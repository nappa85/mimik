//! Independent test-only decoding of rendered browser QR screenshots.
use std::{fs, io::Cursor, path::PathBuf};
fn main() -> Result<(), Box<dyn std::error::Error>> {
    let root = PathBuf::from(
        std::env::args_os()
            .nth(1)
            .ok_or("Verification directory required")?,
    );
    for name in ["desktop", "mobile", "proxy-player"] {
        let expected = fs::read_to_string(root.join(format!("{name}.link")))?;
        if expected.len() > 512 {
            return Err("QR expectation exceeds the sharing-link bound".into());
        }
        let path = root.join(format!("{name}.png"));
        if fs::metadata(&path)?.len() > 1_048_576 {
            return Err("QR screenshot exceeds the image bound".into());
        }
        let data = fs::read(path)?;
        let mut decoder = png::Decoder::new(Cursor::new(data));
        decoder.set_transformations(png::Transformations::EXPAND | png::Transformations::STRIP_16);
        let mut reader = decoder.read_info()?;
        if reader.info().width > 512 || reader.info().height > 512 {
            return Err("QR screenshot dimensions exceed the bound".into());
        }
        let mut pixels = vec![
            0;
            reader
                .output_buffer_size()
                .ok_or("PNG dimensions unavailable")?
        ];
        let info = reader.next_frame(&mut pixels)?;
        let channels = match info.color_type {
            png::ColorType::Rgb => 3,
            png::ColorType::Rgba => 4,
            png::ColorType::Grayscale => 1,
            png::ColorType::GrayscaleAlpha => 2,
            png::ColorType::Indexed => return Err("Unsupported screenshot color".into()),
        };
        let width = usize::try_from(info.width)?;
        let height = usize::try_from(info.height)?;
        let mut image = rqrr::PreparedImage::prepare_from_greyscale(width, height, |x, y| {
            pixels[(y * width + x) * channels]
        });
        let grids = image.detect_grids();
        if grids.len() != 1 {
            return Err("Expected one readable rendered QR code".into());
        }
        let (_, payload) = grids[0].decode().map_err(|_| "Rendered QR is unreadable")?;
        if payload != expected {
            return Err("Rendered QR payload does not match the sharing link".into());
        }
    }
    println!("Independent rendered QR screenshot decoding passed at both viewports.");
    Ok(())
}
