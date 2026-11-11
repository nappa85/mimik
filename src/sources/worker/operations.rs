use super::{
    Error, Inspection, Page, RenderedPage, geometry, pixel_size,
    records::{self, Document, PageWords, Stats, TextLine, Word},
};
use mupdf::{
    Colorspace, Device, Matrix, Pixmap, TextCharFlags, TextPageFlags,
    pdf::{PdfDocument, PdfObject},
};
use std::collections::BTreeSet;

/// Input is opened only in the worker; callers must not use this API in the server.
#[derive(Clone, Copy)]
pub(super) enum Input<'a> {
    Bytes(&'a [u8]),
}

fn mupdf_error(error: mupdf::Error) -> Error {
    // MuPDF's numeric FZ_ERROR_SYSTEM/LIMIT/UNSUPPORTED values (fitz/context.h).
    // Never propagate its diagnostic string, which can contain source text or paths.
    match error {
        mupdf::Error::MuPdf(e) if e.code == 2 || e.code == 5 => Error::TooLarge,
        mupdf::Error::MuPdf(e) if e.code == 6 => Error::Unsupported,
        _ => Error::Unreadable,
    }
}
impl From<mupdf::Error> for Error {
    fn from(e: mupdf::Error) -> Self {
        mupdf_error(e)
    }
}

fn open(input: Input<'_>) -> Result<PdfDocument, Error> {
    let Input::Bytes(bytes) = input;
    if bytes.len() > 128 * 1024 * 1024 {
        return Err(Error::TooLarge);
    }
    // MuPDF repairs truncated files. The worker contract rejects a missing final EOF marker.
    if !bytes.windows(5).take(1024).any(|w| w == b"%PDF-")
        || !bytes[bytes.len().saturating_sub(1024)..]
            .windows(5)
            .any(|w| w == b"%%EOF")
    {
        return Err(Error::Unreadable);
    }
    let document = PdfDocument::from_bytes(bytes)?;
    if document.needs_password()? {
        return Err(Error::PasswordProtected);
    }
    // Check the advertised root count before MuPDF tries to reconcile a damaged tree.
    if let Some(tree) = document.catalog()?.get_dict("Pages")?
        && let Some(count) = tree.get_dict("Count")?
        && count.is_number()?
        && count.as_float()? > 2000.0
    {
        return Err(Error::TooManyPages);
    }
    let count = document.page_count()?;
    if count > 2000 {
        return Err(Error::TooManyPages);
    }
    if count < 1 {
        return Err(Error::Unreadable);
    }
    Ok(document)
}

fn box_value(value: &PdfObject) -> Result<[f64; 4], Error> {
    if !value.is_array()? || value.len()? != 4 {
        return Err(Error::Unsupported);
    }
    let mut result = [0.0; 4];
    for (index, slot) in result.iter_mut().enumerate() {
        let obj = value
            .get_array(i32::try_from(index).map_err(|_| Error::Unsupported)?)?
            .ok_or(Error::Unsupported)?;
        if !obj.is_number()? {
            return Err(Error::Unsupported);
        }
        *slot = f64::from(obj.as_float()?);
    }
    if result.iter().any(|n| !n.is_finite()) {
        return Err(Error::Unsupported);
    }
    if result[0] > result[2] {
        result.swap(0, 2);
    }
    if result[1] > result[3] {
        result.swap(1, 3);
    }
    if result[0] >= result[2] || result[1] >= result[3] {
        return Err(Error::Unsupported);
    }
    Ok(result)
}

fn page_info(document: &PdfDocument, n: u32) -> Result<Page, Error> {
    let obj = document.find_page(i32::try_from(n - 1).map_err(|_| Error::Unsupported)?)?;
    let media = box_value(
        &obj.get_dict_inheritable("MediaBox")?
            .ok_or(Error::Unsupported)?,
    )?;
    let crop = obj
        .get_dict_inheritable("CropBox")?
        .map(|o| box_value(&o))
        .transpose()?
        .unwrap_or(media);
    let visible = [
        media[0].max(crop[0]),
        media[1].max(crop[1]),
        media[2].min(crop[2]),
        media[3].min(crop[3]),
    ];
    let user_unit = obj
        .get_dict("UserUnit")?
        .map(|o| o.as_float())
        .transpose()?
        .map_or(1.0, f64::from);
    let rotation = obj
        .get_dict_inheritable("Rotate")?
        .map(|o| o.as_int())
        .transpose()?
        .unwrap_or(0);
    let rotate = u16::try_from(rotation.rem_euclid(360)).map_err(|_| Error::Unsupported)?;
    let coefficients = geometry::linear(rotate, user_unit)?;
    let mut transform = [
        coefficients[0],
        coefficients[1],
        coefficients[2],
        coefficients[3],
        0.0,
        0.0,
    ];
    let bounds = geometry::transformed_box(visible, transform);
    transform[4] = -bounds[0];
    transform[5] = -bounds[1];
    let page = Page {
        n,
        width: bounds[2] - bounds[0],
        height: bounds[3] - bounds[1],
        rotate,
        user_unit,
        transform,
        visible,
        tagged: document.catalog()?.get_dict("StructTreeRoot")?.is_some()
            && document
                .catalog()?
                .get_dict("MarkInfo")?
                .and_then(|o| o.get_dict("Marked").ok().flatten())
                .is_some_and(|o| o.as_bool().unwrap_or(false)),
    };
    if page.width > 4000.0 || page.height > 4000.0 {
        return Err(Error::TooLarge);
    }
    geometry::validate_page(&page)?;
    Ok(page)
}

fn inspection(document: &PdfDocument) -> Result<Inspection, Error> {
    let count = u32::try_from(document.page_count()?).map_err(|_| Error::Unreadable)?;
    let pages = (1..=count)
        .map(|n| page_info(document, n))
        .collect::<Result<Vec<_>, _>>()?;
    Ok(Inspection {
        document: Document {
            pages: count,
            encrypted: document.trailer()?.get_dict("Encrypt")?.is_some(),
            mupdf: super::MUPDF_VERSION.to_owned(),
        },
        pages,
    })
}

/// Inspect page dictionaries without interpreting content streams.
///
/// # Errors
/// Returns only bounded worker reasons for malformed or unsupported input.
pub(super) fn inspect(input: Input<'_>) -> Result<Inspection, Error> {
    inspection(&open(input)?)
}

// Only validated bounded geometry crosses the f64 -> MuPDF f32 boundary.
#[allow(clippy::cast_possible_truncation)]
fn matrix(values: [f64; 6]) -> Matrix {
    Matrix::new(
        values[0] as f32,
        values[1] as f32,
        values[2] as f32,
        values[3] as f32,
        values[4] as f32,
        values[5] as f32,
    )
}
fn values(m: &Matrix) -> [f64; 6] {
    [m.a, m.b, m.c, m.d, m.e, m.f].map(f64::from)
}

#[derive(Default)]
struct FontScan {
    visited: BTreeSet<i32>,
    names: BTreeSet<String>,
    remaining: usize,
    incomplete: bool,
    skipped: u32,
}
impl FontScan {
    fn enter(&mut self, obj: &PdfObject, depth: usize) -> bool {
        let id = obj.as_indirect().unwrap_or(0);
        if id > 0 && !self.visited.insert(id) {
            return false;
        }
        if depth > 32 || self.remaining == 0 {
            self.incomplete = true;
            return false;
        }
        self.remaining -= 1;
        true
    }
    fn font(&mut self, font: &PdfObject) -> Result<(), Error> {
        if !font.is_dict()? {
            return Err(Error::Unreadable);
        }
        let subtype = font
            .get_dict("Subtype")?
            .map(|o| o.as_name())
            .transpose()?
            .unwrap_or_default();
        if subtype == b"Type3" {
            return Ok(());
        }
        let effective = if subtype == b"Type0" {
            font.get_dict("DescendantFonts")?
                .ok_or(Error::Unreadable)?
                .get_array(0)?
                .ok_or(Error::Unreadable)?
        } else {
            font.clone()
        };
        let descriptor = effective.get_dict("FontDescriptor")?;
        let mut embedded = false;
        if let Some(descriptor) = descriptor {
            for key in ["FontFile", "FontFile2", "FontFile3"] {
                embedded |= descriptor.get_dict(key)?.is_some();
            }
        }
        if !embedded {
            let name = font
                .get_dict("BaseFont")?
                .or(effective.get_dict("BaseFont")?)
                .map(|o| o.as_name())
                .transpose()?
                .unwrap_or_default();
            let mut encoded = String::new();
            for byte in name {
                let part = if (33..=126).contains(&byte) && !b"#%()/<>[]{}".contains(&byte) {
                    char::from(byte).to_string()
                } else {
                    format!("#{byte:02x}")
                };
                if encoded.len() + part.len() > 64 {
                    self.incomplete = true;
                    break;
                }
                encoded.push_str(&part);
            }
            if encoded.is_empty() {
                encoded = "Unknown".into();
            }
            if self.names.contains(&encoded) || self.names.len() < records::FONT_LIMIT {
                self.names.insert(encoded);
            } else {
                self.incomplete = true;
            }
        }
        Ok(())
    }
    fn resources(&mut self, resources: &PdfObject, depth: usize) -> Result<(), Error> {
        if !self.enter(resources, depth) {
            return Ok(());
        }
        if let Some(fonts) = resources.get_dict("Font")? {
            if fonts.is_dict()? {
                for pair in fonts.dict_iter()? {
                    let (_, font) = pair?;
                    if self.remaining == 0 {
                        self.incomplete = true;
                        break;
                    }
                    if self.enter(&font, depth) && self.font(&font).is_err() {
                        self.skipped += 1;
                    }
                }
            } else {
                self.skipped += 1;
            }
        }
        for (key, subtype) in [("XObject", Some(b"Form".as_slice())), ("Pattern", None)] {
            if let Some(objects) = resources.get_dict(key)? {
                if !objects.is_dict()? {
                    self.incomplete = true;
                    continue;
                }
                for pair in objects.dict_iter()? {
                    if self.remaining == 0 {
                        self.incomplete = true;
                        break;
                    }
                    let (_, obj) = pair?;
                    if !self.enter(&obj, depth + 1) {
                        continue;
                    }
                    let selected = if let Some(expected) = subtype {
                        obj.get_dict("Subtype")?
                            .is_some_and(|s| s.as_name().is_ok_and(|s| s == expected))
                    } else {
                        obj.get_dict("PatternType")?
                            .is_some_and(|s| s.as_int().ok() == Some(1))
                    };
                    if selected && let Some(nested) = obj.get_dict("Resources")? {
                        self.resources(&nested, depth + 1)?;
                    }
                }
            }
        }
        Ok(())
    }
    fn appearance(&mut self, obj: &PdfObject, depth: usize) -> Result<(), Error> {
        if !self.enter(obj, depth) {
            return Ok(());
        }
        if obj.is_stream()? {
            if let Some(resources) = obj.get_dict("Resources")? {
                self.resources(&resources, depth + 1)?;
            }
        } else if obj.is_dict()? {
            for pair in obj.dict_iter()? {
                if self.remaining == 0 {
                    self.incomplete = true;
                    break;
                }
                self.appearance(&pair?.1, depth + 1)?;
            }
        }
        Ok(())
    }
    fn page(&mut self, page: &PdfObject) -> Result<(), Error> {
        if let Some(resources) = page.get_dict_inheritable("Resources")? {
            self.resources(&resources, 0)?;
        }
        if let Some(annots) = page.get_dict("Annots")? {
            for i in 0..annots.len()? {
                if self.remaining == 0 {
                    self.incomplete = true;
                    break;
                }
                if let Some(annot) =
                    annots.get_array(i32::try_from(i).map_err(|_| Error::TooLarge)?)?
                {
                    if !self.enter(&annot, 0) {
                        continue;
                    }
                    if let Some(ap) = annot.get_dict("AP")?
                        && let Some(normal) = ap.get_dict("N")?
                    {
                        self.appearance(&normal, 0)?;
                    }
                }
            }
        }
        Ok(())
    }
}

fn render_page(document: &PdfDocument, info: &Page) -> Result<RenderedPage, Error> {
    let page =
        document.load_pdf_page(i32::try_from(info.n - 1).map_err(|_| Error::Unsupported)?)?;
    let mut correction = page.ctm()?.invert().ok_or(Error::Unsupported)?;
    correction.concat(matrix(info.transform));
    let (width_px, height_px) = pixel_size(info.width, info.height)?;
    let mut pixels = Pixmap::new_with_w_h(
        &Colorspace::device_rgb(),
        i32::try_from(width_px).map_err(|_| Error::TooLarge)?,
        i32::try_from(height_px).map_err(|_| Error::TooLarge)?,
        false,
    )?;
    pixels.clear_with(255)?;
    let mut render_matrix = correction.clone();
    render_matrix.concat(matrix([
        f64::from(width_px) / info.width,
        0.0,
        0.0,
        f64::from(height_px) / info.height,
        0.0,
        0.0,
    ]));
    let device = Device::from_pixmap(&pixels)?;
    page.run(&device, &render_matrix)?;
    drop(device);
    let mut png = records::LimitedBytes {
        bytes: Vec::new(),
        limit: records::IMAGE_LIMIT,
    };
    {
        let mut encoder = png::Encoder::new(&mut png, width_px, height_px);
        encoder.set_color(png::ColorType::Rgb);
        encoder.set_depth(png::BitDepth::Eight);
        encoder.set_compression(png::Compression::Balanced);
        let mut writer = encoder.write_header().map_err(|_| Error::TooLarge)?;
        writer
            .write_image_data(pixels.samples())
            .map_err(|_| Error::TooLarge)?;
        writer.finish().map_err(|_| Error::TooLarge)?;
    }
    let (stats, lines) = text(&page, info, &correction)?;
    Ok(RenderedPage {
        n: info.n,
        png: png.bytes,
        width_px,
        height_px,
        dpi: 150,
        stats,
        lines,
    })
}

fn clipped(bounds: mupdf::Rect, info: &Page, correction: &Matrix) -> [f64; 4] {
    let b = geometry::transformed_box(
        [bounds.x0, bounds.y0, bounds.x1, bounds.y1].map(f64::from),
        values(correction),
    );
    [
        b[0].max(0.0),
        b[1].max(0.0),
        b[2].min(info.width),
        b[3].min(info.height),
    ]
}
fn text(
    page: &mupdf::pdf::PdfPage,
    info: &Page,
    correction: &Matrix,
) -> Result<(Stats, Vec<TextLine>), Error> {
    let text = page.to_text_page(
        TextPageFlags::CLIP
            | TextPageFlags::PRESERVE_IMAGES
            | TextPageFlags::USE_CID_FOR_UNKNOWN_UNICODE,
    )?;
    let mut stats = Stats {
        n: info.n,
        chars: 0,
        unmapped: 0,
        image_area: 0.0,
        fonts_substituted: Vec::new(),
        fonts_skipped: 0,
        incomplete: false,
        dropped_lines: 0,
    };
    let mut lines = Vec::new();
    let mut images = Vec::new();
    for block in text.blocks() {
        if block.ctm().is_some() {
            let b = clipped(block.bounds(), info, correction);
            if images.len() == 20_000 {
                return Err(Error::TooLarge);
            }
            if b[0] < b[2] && b[1] < b[3] {
                images.push(b);
            }
        }
        for line in block.lines() {
            if lines.len() == 20_000 {
                return Err(Error::TooLarge);
            }
            let mut value = String::new();
            for ch in line.chars() {
                stats.chars += 1;
                if stats.chars > 1_000_000 {
                    return Err(Error::TooLarge);
                }
                let unmapped = ch
                    .flags()
                    .intersects(TextCharFlags::UNICODE_IS_CID | TextCharFlags::UNICODE_IS_GID)
                    || ch.char().is_none()
                    || ch.char() == Some('\u{fffd}');
                stats.unmapped += u32::from(unmapped);
                value.push(if unmapped {
                    '\u{fffd}'
                } else {
                    ch.char().unwrap_or('\u{fffd}')
                });
                if value.len() > 4096 {
                    return Err(Error::TooLarge);
                }
            }
            let bounds = clipped(line.bounds(), info, correction);
            if !value.is_empty() {
                if bounds[0] >= bounds[2] || bounds[1] >= bounds[3] {
                    stats.dropped_lines += 1;
                    continue;
                }
                lines.push(TextLine {
                    text: value,
                    bounds,
                });
            }
        }
    }
    stats.image_area =
        (geometry::image_coverage(&images) / (info.width * info.height)).clamp(0.0, 1.0);
    let mut scan = FontScan {
        remaining: 10000,
        ..FontScan::default()
    };
    if scan.page(&page.object()).is_err() {
        scan.incomplete = true;
    }
    stats.fonts_substituted = scan.names.into_iter().collect();
    stats.fonts_skipped = scan.skipped;
    stats.incomplete = scan.incomplete || scan.skipped > 0;
    records::validate_stats(&stats, info.n)?;
    records::validate_lines(&lines, info)?;
    serde_json::to_writer(
        records::LimitedBytes {
            bytes: Vec::new(),
            limit: records::TEXT_LIMIT,
        },
        &lines,
    )
    .map_err(|_| Error::TooLarge)?;
    Ok((stats, lines))
}

/// A split of a word at a gap wider than this share of the font size, or at whitespace.
const WORD_GAP: f32 = 0.25;

fn push_word(
    words: &mut Vec<Word>,
    text: &mut String,
    bounds: &mut Option<mupdf::Rect>,
    info: &Page,
    correction: &Matrix,
) -> Result<(), Error> {
    let value = std::mem::take(text);
    let Some(rect) = bounds.take() else {
        return Ok(());
    };
    if value.is_empty() {
        return Ok(());
    }
    let b = clipped(rect, info, correction);
    if b[0] >= b[2] || b[1] >= b[3] {
        return Ok(());
    }
    if words.len() == records::MAX_WORDS {
        return Err(Error::TooLarge);
    }
    words.push(Word {
        text: value,
        bounds: b,
    });
    Ok(())
}

/// The words of one page: whitespace and font-size gaps split a line's characters; a run longer
/// than the word bound is cut at a character boundary.
fn page_words(document: &PdfDocument, info: &Page) -> Result<PageWords, Error> {
    let page =
        document.load_pdf_page(i32::try_from(info.n - 1).map_err(|_| Error::Unsupported)?)?;
    let mut correction = page.ctm()?.invert().ok_or(Error::Unsupported)?;
    correction.concat(matrix(info.transform));
    let text = page.to_text_page(
        TextPageFlags::CLIP
            | TextPageFlags::PRESERVE_IMAGES
            | TextPageFlags::USE_CID_FOR_UNKNOWN_UNICODE,
    )?;
    let mut words = Vec::new();
    let mut chars = 0_u32;
    for block in text.blocks() {
        for line in block.lines() {
            let mut value = String::new();
            let mut bounds: Option<mupdf::Rect> = None;
            for ch in line.chars() {
                chars += 1;
                if chars > 1_000_000 {
                    return Err(Error::TooLarge);
                }
                let unmapped = ch
                    .flags()
                    .intersects(TextCharFlags::UNICODE_IS_CID | TextCharFlags::UNICODE_IS_GID)
                    || ch.char().is_none()
                    || ch.char() == Some('\u{fffd}');
                let value_char = if unmapped {
                    '\u{fffd}'
                } else {
                    ch.char().unwrap_or('\u{fffd}')
                };
                if value_char.is_whitespace() {
                    push_word(&mut words, &mut value, &mut bounds, info, &correction)?;
                    continue;
                }
                let rect = mupdf::Rect::from(ch.quad());
                if let Some(previous) = bounds {
                    let gap = (rect.x0 - previous.x1)
                        .max(previous.x0 - rect.x1)
                        .max(rect.y0 - previous.y1)
                        .max(previous.y0 - rect.y1);
                    if gap > WORD_GAP * ch.size()
                        || value.len() + value_char.len_utf8() > records::MAX_WORD_BYTES
                    {
                        push_word(&mut words, &mut value, &mut bounds, info, &correction)?;
                    }
                }
                bounds = Some(match bounds {
                    Some(previous) => previous.r#union(&rect),
                    None => rect,
                });
                value.push(value_char);
            }
            push_word(&mut words, &mut value, &mut bounds, info, &correction)?;
        }
    }
    records::validate_words(&words, info)?;
    Ok(PageWords { n: info.n, words })
}

/// The words of one to eight distinct pages, in requested order.
///
/// # Errors
/// Returns fixed reasons; source content and file names never enter errors.
pub(super) fn words_each(
    input: Input<'_>,
    pages: &[u32],
    mut emit: impl FnMut(PageWords) -> Result<(), Error>,
) -> Result<(), Error> {
    let document = open(input)?;
    if pages.is_empty()
        || pages.len() > 8
        || pages.iter().collect::<BTreeSet<_>>().len() != pages.len()
    {
        return Err(Error::Unsupported);
    }
    let count = u32::try_from(document.page_count()?).map_err(|_| Error::Unreadable)?;
    for &n in pages {
        if n == 0 || n > count {
            return Err(Error::Unsupported);
        }
        emit(page_words(&document, &page_info(&document, n)?)?)?;
    }
    Ok(())
}

/// Render a batch of one to eight distinct pages, in requested order.
///
/// # Errors
/// Returns fixed reasons; source content and file names never enter errors.
#[cfg(test)]
pub(super) fn render(input: Input<'_>, pages: &[u32]) -> Result<Vec<RenderedPage>, Error> {
    let mut output = Vec::new();
    render_each(input, pages, |page| {
        output.push(page);
        Ok(())
    })?;
    Ok(output)
}
pub(super) fn render_each(
    input: Input<'_>,
    pages: &[u32],
    mut emit: impl FnMut(RenderedPage) -> Result<(), Error>,
) -> Result<(), Error> {
    let document = open(input)?;
    if pages.is_empty()
        || pages.len() > 8
        || pages.iter().collect::<BTreeSet<_>>().len() != pages.len()
    {
        return Err(Error::Unsupported);
    }
    mupdf::Context::get().set_aa_level(8);
    let count = u32::try_from(document.page_count()?).map_err(|_| Error::Unreadable)?;
    for &n in pages {
        if n == 0 || n > count {
            return Err(Error::Unsupported);
        }
        emit(render_page(&document, &page_info(&document, n)?)?)?;
    }
    Ok(())
}
