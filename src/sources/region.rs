//! Derived bounding-region previews; original evidence is never modified.
use crate::AppError;
use serde::{
    Deserialize,
    de::{SeqAccess, Visitor},
};
use std::{
    fmt,
    io::{Cursor, Write},
    sync::{Arc, LazyLock},
};
use tokio::sync::{OwnedSemaphorePermit, Semaphore};

pub(super) const INPUT_LIMIT: usize = 24 * 1024 * 1024;
const OUTPUT_LIMIT: usize = 4 * 1024 * 1024;
static JOBS: LazyLock<Arc<Semaphore>> = LazyLock::new(|| Arc::new(Semaphore::new(1)));

pub(crate) struct RegionInput {
    pub image: Vec<u8>,
    pub prepared: Vec<u8>,
    pub pdf_page: i64,
    pub polygon: String,
    pub permit: OwnedSemaphorePermit,
}

pub(crate) fn admit() -> Option<OwnedSemaphorePermit> {
    JOBS.clone().try_acquire_owned().ok()
}

fn invalid() -> AppError {
    AppError::Invalid(
        "Region preview unavailable: invalid geometry/image metadata or preview size limit exceeded. Inspect the full-page image.",
    )
}

// Project only page geometry: Serde skips all unit/provenance values, avoiding
// expansion of a potentially large prepared bundle into owned strings/vectors.
#[derive(Deserialize)]
struct Geometry {
    pdf_page: i64,
    width_points: f64,
    height_points: f64,
}

#[derive(Deserialize)]
struct GeometryBundle {
    #[serde(deserialize_with = "geometries")]
    pages: Vec<Geometry>,
}

fn geometries<'de, D: serde::Deserializer<'de>>(decoder: D) -> Result<Vec<Geometry>, D::Error> {
    struct Bounded;
    impl<'de> Visitor<'de> for Bounded {
        type Value = Vec<Geometry>;
        fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
            formatter.write_str("at most 10000 page geometries")
        }
        fn visit_seq<A: SeqAccess<'de>>(self, mut seq: A) -> Result<Self::Value, A::Error> {
            let mut pages = Vec::new();
            while let Some(page) = seq.next_element()? {
                if pages.len() == 10_000 {
                    return Err(serde::de::Error::custom("Too many pages"));
                }
                pages.push(page);
            }
            Ok(pages)
        }
    }
    decoder.deserialize_seq(Bounded)
}

struct LimitedOutput(Vec<u8>);
impl Write for LimitedOutput {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        if bytes.len() > OUTPUT_LIMIT.saturating_sub(self.0.len()) {
            return Err(std::io::Error::other("Region PNG output limit exceeded"));
        }
        self.0.extend_from_slice(bytes);
        Ok(bytes.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

// Float conversion is used only after validation and outward rounding, clamped
// to the PNG dimension (at most 8192); no negative/overflowing conversion occurs.
#[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
fn pixel(value: f64, points: f64, dimension: u32, upper: bool) -> u32 {
    let scaled = value / points * f64::from(dimension);
    let rounded = if upper { scaled.ceil() } else { scaled.floor() };
    rounded.clamp(0.0, f64::from(dimension)) as u32
}

fn encoding_info(
    original: &png::Info<'_>,
    width: u32,
    height: u32,
    color: png::ColorType,
) -> Result<png::Info<'static>, AppError> {
    if original.coding_independent_code_points.is_some()
        || original.mastering_display_color_volume.is_some()
        || original.content_light_level.is_some()
    {
        return Err(AppError::Invalid(
            "Unsupported HDR/color metadata for region preview; inspect the original full-page image.",
        ));
    }
    let mut encoding = png::Info::with_size(width, height);
    encoding.color_type = color;
    encoding.bit_depth = png::BitDepth::Eight;
    // png0.18 decodes these chunks into gama_chunk/chrm_chunk; the encoder
    // writes source_gamma/source_chromaticities rather than those raw fields.
    encoding.source_gamma = original.gama_chunk.or(original.source_gamma);
    encoding.source_chromaticities = original.chrm_chunk.or(original.source_chromaticities);
    encoding.srgb = original.srgb;
    encoding.icc_profile = original
        .icc_profile
        .as_ref()
        .map(|profile| std::borrow::Cow::Owned(profile.to_vec()));
    Ok(encoding)
}

pub(crate) fn crop(
    image: &[u8],
    prepared: &[u8],
    pdf_page: i64,
    polygon: &str,
) -> Result<Vec<u8>, AppError> {
    if image.len() > INPUT_LIMIT || prepared.len() > INPUT_LIMIT || polygon.len() > 1024 {
        return Err(invalid());
    }
    let bundle: GeometryBundle = serde_json::from_slice(prepared).map_err(|_| invalid())?;
    let mut matches = bundle.pages.iter().filter(|page| page.pdf_page == pdf_page);
    let page = matches.next().ok_or_else(invalid)?;
    if matches.next().is_some() {
        return Err(invalid());
    }
    let points: [[f64; 2]; 4] = serde_json::from_str(polygon).map_err(|_| invalid())?;
    if points.iter().any(|[x, y]| {
        !x.is_finite()
            || !y.is_finite()
            || *x < 0.0
            || *y < 0.0
            || *x > page.width_points
            || *y > page.height_points
    }) {
        return Err(invalid());
    }
    let min_x = points.iter().map(|p| p[0]).fold(f64::INFINITY, f64::min);
    let min_y = points.iter().map(|p| p[1]).fold(f64::INFINITY, f64::min);
    let max_x = points
        .iter()
        .map(|p| p[0])
        .fold(f64::NEG_INFINITY, f64::max);
    let max_y = points
        .iter()
        .map(|p| p[1])
        .fold(f64::NEG_INFINITY, f64::max);
    if min_x >= max_x || min_y >= max_y {
        return Err(invalid());
    }
    let mut decoder = png::Decoder::new(Cursor::new(image));
    decoder.set_limits(png::Limits {
        bytes: 64 * 1024 * 1024,
    });
    decoder.set_transformations(png::Transformations::EXPAND | png::Transformations::STRIP_16);
    let mut reader = decoder.read_info().map_err(|_| invalid())?;
    super::import::validate_image_header(reader.info(), page.width_points, page.height_points)
        .map_err(|_| invalid())?;
    let size = reader
        .output_buffer_size()
        .filter(|size| *size <= 48 * 1024 * 1024)
        .ok_or_else(invalid)?;
    let mut pixels = vec![0; size];
    let info = reader.next_frame(&mut pixels).map_err(|_| invalid())?;
    let x0 = pixel(min_x, page.width_points, info.width, false);
    let x1 = pixel(max_x, page.width_points, info.width, true);
    let y0 = pixel(min_y, page.height_points, info.height, false);
    let y1 = pixel(max_y, page.height_points, info.height, true);
    let width = x1
        .checked_sub(x0)
        .filter(|width| *width > 0)
        .ok_or_else(invalid)?;
    let height = y1
        .checked_sub(y0)
        .filter(|height| *height > 0)
        .ok_or_else(invalid)?;
    let channels = info.color_type.samples();
    let row_size = usize::try_from(width)
        .map_err(|_| invalid())?
        .checked_mul(channels)
        .ok_or_else(invalid)?;
    let crop_size = row_size
        .checked_mul(usize::try_from(height).map_err(|_| invalid())?)
        .filter(|size| *size <= 48 * 1024 * 1024)
        .ok_or_else(invalid)?;
    let mut cropped = Vec::with_capacity(crop_size);
    for y in y0..y1 {
        let start = usize::try_from(y)
            .map_err(|_| invalid())?
            .checked_mul(info.line_size)
            .and_then(|offset| {
                usize::try_from(x0)
                    .ok()?
                    .checked_mul(channels)?
                    .checked_add(offset)
            })
            .ok_or_else(invalid)?;
        let end = start.checked_add(row_size).ok_or_else(invalid)?;
        cropped.extend_from_slice(pixels.get(start..end).ok_or_else(invalid)?);
    }
    let encoding = encoding_info(reader.info(), width, height, info.color_type)?;
    drop(reader);
    drop(pixels);
    let mut output = LimitedOutput(Vec::new());
    {
        let encoder = png::Encoder::with_info(&mut output, encoding).map_err(|_| invalid())?;
        let mut writer = encoder.write_header().map_err(|_| invalid())?;
        writer.write_image_data(&cropped).map_err(|_| invalid())?;
        writer.finish().map_err(|_| invalid())?;
    }
    Ok(output.0)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[tokio::test]
    async fn admission_never_queues_and_recovers_after_release() {
        use axum::{
            body::Body,
            http::{Request, StatusCode},
        };
        use tower::ServiceExt;
        let permit = admit().unwrap();
        assert!(admit().is_none());
        let dir = tempfile::tempdir().unwrap();
        let router = crate::app(crate::Store::open(&dir.path().join("test.sqlite3")).unwrap());
        let mut requests = tokio::task::JoinSet::new();
        for _ in 0..16 {
            let router = router.clone();
            requests.spawn(async move {
                let response = router
                    .oneshot(
                        Request::builder()
                            .uri("/sources/pages/1/units/1/image")
                            .header("host", "localhost:3000")
                            .body(Body::empty())
                            .unwrap(),
                    )
                    .await
                    .unwrap();
                assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
                assert_eq!(response.headers()["retry-after"], "1");
            });
        }
        while let Some(result) = requests.join_next().await {
            result.unwrap();
        }
        drop(permit);
        assert!(admit().is_some());
    }
    #[test]
    fn output_limit_errors_instead_of_truncating_successfully() {
        let mut output = LimitedOutput(vec![0; OUTPUT_LIMIT - 1]);
        assert!(output.write_all(&[1, 2]).is_err());
        assert_eq!(output.0.len(), OUTPUT_LIMIT - 1);
    }

    fn png_bytes(color: png::ColorType, pixels: &[u8], palette: bool) -> Vec<u8> {
        let mut bytes = Vec::new();
        {
            let mut encoder = png::Encoder::new(&mut bytes, 2, 2);
            encoder.set_color(color);
            encoder.set_depth(png::BitDepth::Eight);
            if palette {
                encoder.set_palette(vec![12, 34, 56, 78, 90, 12]);
                encoder.set_trns(vec![0, 255]);
            }
            encoder
                .write_header()
                .unwrap()
                .write_image_data(pixels)
                .unwrap();
        }
        bytes
    }

    #[test]
    fn crops_supported_channels_alpha_and_exact_page_metadata() {
        let metadata = br#"{"pages":[{"pdf_page":9,"width_points":400,"height_points":200,"units":[]},{"pdf_page":2,"width_points":20,"height_points":20}]}"#;
        for (color, pixels, expected, palette) in [
            (
                png::ColorType::Rgb,
                vec![1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12],
                vec![1, 2, 3],
                false,
            ),
            (
                png::ColorType::Rgba,
                vec![1, 2, 3, 0, 4, 5, 6, 255, 7, 8, 9, 128, 10, 11, 12, 255],
                vec![1, 2, 3, 0],
                false,
            ),
            (png::ColorType::Grayscale, vec![1, 2, 3, 4], vec![1], false),
            (
                png::ColorType::GrayscaleAlpha,
                vec![1, 0, 2, 255, 3, 128, 4, 255],
                vec![1, 0],
                false,
            ),
            (
                png::ColorType::Indexed,
                vec![0, 1, 1, 0],
                vec![12, 34, 56, 0],
                true,
            ),
        ] {
            let image = png_bytes(color, &pixels, palette);
            let bytes = crop(&image, metadata, 2, "[[0,0],[10,0],[10,10],[0,10]]").unwrap();
            let mut reader = png::Decoder::new(Cursor::new(bytes)).read_info().unwrap();
            let mut decoded = vec![0; reader.output_buffer_size().unwrap()];
            let info = reader.next_frame(&mut decoded).unwrap();
            assert_eq!((info.width, info.height), (1, 1));
            assert_eq!(decoded, expected);
            // An outward-rounded fractional region touching the right/bottom
            // boundary includes both pixels and never leaves the image bounds.
            let full = crop(&image, metadata, 2, "[[0.1,0.1],[20,0.1],[20,20],[0.1,20]]").unwrap();
            let reader = png::Decoder::new(Cursor::new(full)).read_info().unwrap();
            assert_eq!((reader.info().width, reader.info().height), (2, 2));
        }
    }

    #[test]
    fn invalid_geometry_metadata_and_corrupt_png_are_explicit_errors() {
        let image = png_bytes(png::ColorType::Grayscale, &[1, 2, 3, 4], false);
        let metadata = br#"{"pages":[{"pdf_page":2,"width_points":20,"height_points":20}]}"#;
        for polygon in [
            "[[0,0],[0,0],[0,10],[0,10]]",
            "[[0,0],[10,0],[10,0],[0,0]]",
            "[[0,0],[21,0],[21,10],[0,10]]",
            "[[0,0],[1e999,0],[10,10],[0,10]]",
            "[[0,0],[-1,0],[10,10],[0,10]]",
        ] {
            assert!(crop(&image, metadata, 2, polygon).is_err());
        }
        let polygon = "[[0,0],[10,0],[10,10],[0,10]]";
        for metadata in [br#"{"pages":[]}"#.as_slice(), br#"{"pages":[{"pdf_page":2}]}"#, br#"{"pages":[{"pdf_page":2,"width_points":20,"height_points":20},{"pdf_page":2,"width_points":20,"height_points":20}]}"#] {
            assert!(crop(&image, metadata, 2, polygon).is_err());
        }
        assert!(crop(b"not a PNG", metadata, 2, polygon).is_err());
    }

    #[test]
    fn incompressible_region_exceeding_output_cap_is_not_a_partial_png_success() {
        let mut pixels = vec![0; 1400 * 1400 * 3];
        let mut state = 123_456_789_u32;
        for byte in &mut pixels {
            state ^= state << 13;
            state ^= state >> 17;
            state ^= state << 5;
            *byte = state.to_le_bytes()[0];
        }
        let mut image = Vec::new();
        {
            let mut encoder = png::Encoder::new(&mut image, 1400, 1400);
            encoder.set_color(png::ColorType::Rgb);
            encoder.set_depth(png::BitDepth::Eight);
            encoder
                .write_header()
                .unwrap()
                .write_image_data(&pixels)
                .unwrap();
        }
        assert!(
            crop(
                &image,
                br#"{"pages":[{"pdf_page":1,"width_points":1400,"height_points":1400}]}"#,
                1,
                "[[0,0],[1400,0],[1400,1400],[0,1400]]"
            )
            .is_err()
        );
    }

    #[test]
    fn color_interpretation_survives_preview_encoding() {
        for kind in 0..3 {
            let mut source = png::Info::with_size(2, 2);
            source.color_type = png::ColorType::Rgba;
            source.bit_depth = png::BitDepth::Eight;
            if kind == 0 {
                source.source_gamma = Some(png::ScaledFloat::new(0.7));
                source.source_chromaticities = Some(png::SourceChromaticities::new(
                    (0.3127, 0.3290),
                    (0.64, 0.33),
                    (0.3, 0.6),
                    (0.15, 0.06),
                ));
            } else if kind == 1 {
                source.srgb = Some(png::SrgbRenderingIntent::Perceptual);
            } else {
                source.icc_profile = Some(std::borrow::Cow::Owned(vec![0; 128]));
            }
            let mut image = Vec::new();
            {
                png::Encoder::with_info(&mut image, source)
                    .unwrap()
                    .write_header()
                    .unwrap()
                    .write_image_data(&[1, 2, 3, 0, 4, 5, 6, 255, 7, 8, 9, 128, 10, 11, 12, 255])
                    .unwrap();
            }
            let original = png::Decoder::new(Cursor::new(&image)).read_info().unwrap();
            let bytes = crop(
                &image,
                br#"{"pages":[{"pdf_page":1,"width_points":20,"height_points":20}]}"#,
                1,
                "[[0,0],[10,0],[10,10],[0,10]]",
            )
            .unwrap();
            let mut preview = png::Decoder::new(Cursor::new(bytes)).read_info().unwrap();
            assert_eq!(preview.info().source_gamma, original.info().source_gamma);
            assert_eq!(
                preview.info().source_chromaticities,
                original.info().source_chromaticities
            );
            assert_eq!(preview.info().gama_chunk, original.info().gama_chunk);
            assert_eq!(preview.info().chrm_chunk, original.info().chrm_chunk);
            assert_eq!(preview.info().srgb, original.info().srgb);
            assert_eq!(preview.info().icc_profile, original.info().icc_profile);
            let mut pixels = vec![0; preview.output_buffer_size().unwrap()];
            preview.next_frame(&mut pixels).unwrap();
            assert_eq!(pixels, [1, 2, 3, 0]);
        }
    }

    #[test]
    fn unsupported_color_chunks_use_explicit_full_page_fallback() {
        for (chunk, data) in [
            (png::chunk::cICP, vec![1, 13, 0, 1]),
            (png::chunk::mDCV, vec![0; 24]),
            (png::chunk::cLLI, vec![0; 8]),
        ] {
            let mut image = Vec::new();
            {
                let mut encoder = png::Encoder::new(&mut image, 2, 2);
                encoder.set_color(png::ColorType::Rgb);
                encoder.set_depth(png::BitDepth::Eight);
                let mut writer = encoder.write_header().unwrap();
                writer.write_chunk(chunk, &data).unwrap();
                writer.write_image_data(&[1; 12]).unwrap();
            }
            let result = crop(
                &image,
                br#"{"pages":[{"pdf_page":1,"width_points":20,"height_points":20}]}"#,
                1,
                "[[0,0],[10,0],[10,10],[0,10]]",
            );
            assert!(
                matches!(result, Err(AppError::Invalid(message)) if message.contains("Unsupported HDR/color metadata"))
            );
        }
    }
}
