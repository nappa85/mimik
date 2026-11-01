//! Deterministic browser harness, absent without `dev-endpoint`. No PDF parser.
use super::{Worker, worker};
use std::{
    path::Path,
    time::{Duration, Instant},
};

/// The only input accepted by the synthetic worker (three empty 100 × 200 pt pages).
pub const PDF: &[u8] = include_bytes!("../../tools/fixtures/upload-three-pages.pdf.txt");
pub(crate) struct Synthetic {
    pub delay: Duration,
}
impl Synthetic {
    fn wait(&self, cancel: &worker::Cancel) -> Result<(), worker::Reason> {
        let end = Instant::now() + self.delay;
        loop {
            if cancel.cancelled() {
                return Err(worker::Reason::Cancelled);
            }
            let now = Instant::now();
            if now >= end {
                return Ok(());
            }
            std::thread::sleep((end - now).min(Duration::from_millis(10)));
        }
    }
}
impl Worker for Synthetic {
    fn availability(&self) -> worker::Availability {
        worker::Availability::Available
    }
    fn inspect(
        &self,
        path: &Path,
        cancel: &worker::Cancel,
    ) -> Result<worker::Inspection, worker::Reason> {
        let bytes = crate::local_file::read_path(path, PDF.len())
            .map_err(|_| worker::Reason::Document(worker::Error::Unsupported))?;
        if bytes != PDF {
            return Err(worker::Reason::Document(worker::Error::Unsupported));
        }
        self.wait(cancel)?;
        Ok(worker::Inspection {
            document: worker::records::Document {
                pages: 3,
                encrypted: false,
                mupdf: worker::MUPDF_VERSION.into(),
            },
            pages: (1..=3)
                .map(|n| worker::Page {
                    n,
                    width: 100.0,
                    height: 200.0,
                    rotate: 0,
                    user_unit: 1.0,
                    transform: [1.0, 0.0, 0.0, -1.0, 0.0, 200.0],
                    visible: [0.0, 0.0, 100.0, 200.0],
                    tagged: false,
                })
                .collect(),
        })
    }
    fn render(
        &self,
        _: &Path,
        pages: &[worker::Page],
        cancel: &worker::Cancel,
        publish: &mut (dyn FnMut(worker::RenderedPage) -> Result<(), worker::Error> + Send),
    ) -> Result<(), worker::Reason> {
        for page in pages {
            self.wait(cancel)?;
            let (w, h) =
                worker::pixel_size(page.width, page.height).map_err(worker::Reason::Document)?;
            let mut png = Vec::new();
            {
                let mut encoder = png::Encoder::new(&mut png, w, h);
                encoder.set_color(png::ColorType::Rgb);
                encoder.set_depth(png::BitDepth::Eight);
                let mut writer = encoder
                    .write_header()
                    .map_err(|_| worker::Reason::Document(worker::Error::Unreadable))?;
                let pixels = vec![255; (w * h * 3) as usize];
                writer
                    .write_image_data(&pixels)
                    .map_err(|_| worker::Reason::Document(worker::Error::Unreadable))?;
            }
            publish(worker::RenderedPage {
                n: page.n,
                png,
                width_px: w,
                height_px: h,
                dpi: 150,
                stats: worker::records::Stats {
                    n: page.n,
                    chars: 26,
                    unmapped: 0,
                    image_area: 0.0,
                    fonts_substituted: vec![],
                    fonts_skipped: 0,
                    incomplete: false,
                    dropped_lines: 0,
                },
                lines: vec![worker::records::TextLine {
                    text: "PRIVATE-TEXT-MARKER page 123".into(),
                    bounds: [5., 5., 95., 15.],
                }],
            })
            .map_err(worker::Reason::Document)?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn synthetic_upload_worker_accepts_only_fixed_pdf_and_cancels_delay() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("synthetic.pdf");
        std::fs::write(&path, PDF).unwrap();
        let worker = Synthetic {
            delay: Duration::ZERO,
        };
        let inspection = worker.inspect(&path, &worker::Cancel::default()).unwrap();
        assert_eq!(inspection.document.pages, 3);
        let mut rendered = Vec::new();
        worker
            .render(
                &path,
                &inspection.pages,
                &worker::Cancel::default(),
                &mut |p| {
                    rendered.push(p);
                    Ok(())
                },
            )
            .unwrap();
        assert_eq!(rendered.len(), 3);
        assert!(rendered.iter().all(|p| p.png == rendered[0].png));
        std::fs::write(&path, b"%PDF-1.7\nother file").unwrap();
        assert!(worker.inspect(&path, &worker::Cancel::default()).is_err());
        let cancel = worker::Cancel::default();
        cancel.cancel();
        let delayed = Synthetic {
            delay: Duration::from_secs(60),
        };
        assert!(matches!(
            delayed.render(&path, &inspection.pages, &cancel, &mut |_| panic!(
                "cancelled page published"
            )),
            Err(worker::Reason::Cancelled)
        ));
    }
}
