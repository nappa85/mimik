//! Bounded wire types and parent-side validation. This module does not parse PDFs.
use super::{Error, geometry, pixel_size};

#[cfg(test)]
#[path = "record_tests.rs"]
mod tests;
use serde::{Deserialize, Serialize};
use std::io::{BufReader, Read, Write};

pub const HEADER_LIMIT: usize = 4096;
pub const IMAGE_LIMIT: usize = 12 * 1024 * 1024;
pub const TEXT_LIMIT: usize = 8 * 1024 * 1024;
pub const INSPECT_LIMIT: usize = 9 * 1024 * 1024;
const SMALL_LIMIT: usize = 4096;
// Even JSON's worst-case six-byte escapes fit a complete 4 KiB stats frame.
pub(super) const FONT_LIMIT: usize = 8;

#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Operation {
    Inspect,
    Render,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Document {
    pub pages: u32,
    pub encrypted: bool,
    pub mupdf: String,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Page {
    pub n: u32,
    pub width: f64,
    pub height: f64,
    pub rotate: u16,
    pub user_unit: f64,
    pub transform: [f64; 6],
    /// Clipped visible box in original PDF coordinates, for independent matrix validation.
    pub visible: [f64; 4],
    pub tagged: bool,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Stats {
    pub n: u32,
    pub chars: u32,
    pub unmapped: u32,
    pub image_area: f64,
    #[serde(deserialize_with = "font_names")]
    pub fonts_substituted: Vec<String>,
    pub fonts_skipped: u32,
    pub incomplete: bool,
    pub dropped_lines: u32,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct TextLine {
    #[serde(deserialize_with = "line_text")]
    pub text: String,
    #[serde(rename = "box")]
    pub bounds: [f64; 4],
}

fn line_text<'de, D: serde::Deserializer<'de>>(d: D) -> Result<String, D::Error> {
    bounded_string::<D, 4096>(d)
}

fn font_names<'de, D: serde::Deserializer<'de>>(d: D) -> Result<Vec<String>, D::Error> {
    #[derive(Deserialize)]
    struct Name(#[serde(deserialize_with = "bounded_string::<_, 64>")] String);
    Ok(BoundedVec::<Name, FONT_LIMIT>::deserialize(d)?
        .0
        .into_iter()
        .map(|n| n.0)
        .collect())
}

fn bounded_string<'de, D: serde::Deserializer<'de>, const N: usize>(
    d: D,
) -> Result<String, D::Error> {
    struct Visitor<const N: usize>;
    impl<const N: usize> serde::de::Visitor<'_> for Visitor<N> {
        type Value = String;
        fn expecting(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            f.write_str("bounded string")
        }
        fn visit_str<E: serde::de::Error>(self, s: &str) -> Result<String, E> {
            // The deserializer presents decoded UTF-8, including JSON escapes. Validate
            // borrowed/scratch bytes before allocating the owned application value.
            if s.is_empty() || s.len() > N {
                return Err(E::custom("invalid string length"));
            }
            Ok(s.to_owned())
        }
    }
    d.deserialize_str(Visitor::<N>)
}

struct BoundedVec<T, const N: usize>(Vec<T>);
impl<'de, T: Deserialize<'de>, const N: usize> Deserialize<'de> for BoundedVec<T, N> {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        struct Visitor<T, const N: usize>(std::marker::PhantomData<T>);
        impl<'de, T: Deserialize<'de>, const N: usize> serde::de::Visitor<'de> for Visitor<T, N> {
            type Value = BoundedVec<T, N>;
            fn expecting(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                f.write_str("bounded array")
            }
            fn visit_seq<A: serde::de::SeqAccess<'de>>(
                self,
                mut seq: A,
            ) -> Result<Self::Value, A::Error> {
                struct Refuse;
                impl<'de> serde::de::DeserializeSeed<'de> for Refuse {
                    type Value = ();
                    fn deserialize<D: serde::Deserializer<'de>>(
                        self,
                        _: D,
                    ) -> Result<(), D::Error> {
                        Err(serde::de::Error::custom("too many elements"))
                    }
                }
                let mut values = Vec::new();
                while values.len() < N {
                    let Some(value) = seq.next_element()? else {
                        return Ok(BoundedVec(values));
                    };
                    values.push(value);
                }
                seq.next_element_seed(Refuse)?;
                Ok(BoundedVec(values))
            }
        }
        d.deserialize_seq(Visitor::<T, N>(std::marker::PhantomData))
    }
}

#[derive(Clone, Copy, Debug, Default, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct Counts {
    pub document: u32,
    pub page: u32,
    pub image: u32,
    pub stats: u32,
    pub text: u32,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct Done {
    operation: Operation,
    counts: Counts,
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
enum Header {
    Document {
        length: usize,
    },
    Page {
        length: usize,
    },
    Image {
        n: u32,
        length: usize,
        width_px: u32,
        height_px: u32,
        dpi: u16,
    },
    Stats {
        length: usize,
    },
    Text {
        n: u32,
        length: usize,
    },
    Done {
        length: usize,
    },
    Error {
        length: usize,
    },
}
impl Header {
    fn length(&self) -> usize {
        match *self {
            Self::Document { length }
            | Self::Page { length }
            | Self::Image { length, .. }
            | Self::Stats { length }
            | Self::Text { length, .. }
            | Self::Done { length }
            | Self::Error { length } => length,
        }
    }
    fn cap(&self) -> usize {
        match self {
            Self::Image { .. } => IMAGE_LIMIT,
            Self::Text { .. } => TEXT_LIMIT,
            _ => SMALL_LIMIT,
        }
    }
}

#[derive(Clone, Debug)]
pub struct Inspection {
    pub document: Document,
    pub pages: Vec<Page>,
}

#[derive(Debug)]
pub struct RenderedPage {
    pub n: u32,
    pub png: Vec<u8>,
    pub width_px: u32,
    pub height_px: u32,
    pub dpi: u16,
    pub stats: Stats,
    pub lines: Vec<TextLine>,
}

pub(super) struct LimitedBytes {
    pub bytes: Vec<u8>,
    pub limit: usize,
}
impl Write for LimitedBytes {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        if bytes.len() > self.limit.saturating_sub(self.bytes.len()) {
            return Err(std::io::ErrorKind::FileTooLarge.into());
        }
        self.bytes
            .try_reserve_exact(bytes.len())
            .map_err(|_| std::io::Error::from(std::io::ErrorKind::OutOfMemory))?;
        self.bytes.extend_from_slice(bytes);
        Ok(bytes.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

pub(crate) fn json<T: Serialize>(value: &T) -> Result<Vec<u8>, Error> {
    let mut output = LimitedBytes {
        bytes: Vec::new(),
        limit: TEXT_LIMIT,
    };
    serde_json::to_writer(&mut output, value).map_err(|_| Error::TooLarge)?;
    Ok(output.bytes)
}
fn parse<T: serde::de::DeserializeOwned>(bytes: &[u8]) -> Result<T, Error> {
    serde_json::from_slice(bytes).map_err(|_| Error::Unreadable)
}

pub(super) fn validate_stats(s: &Stats, n: u32) -> Result<(), Error> {
    if s.n != n
        || s.chars > 1_000_000
        || s.unmapped > s.chars
        || !(0.0..=1.0).contains(&s.image_area)
        || s.fonts_substituted.len() > FONT_LIMIT
        || s.fonts_skipped > 10000
        || s.dropped_lines > 1_000_000
        || s.fonts_substituted
            .iter()
            .any(|s| s.is_empty() || s.len() > 64)
    {
        return Err(Error::Unreadable);
    }
    Ok(())
}

pub(crate) fn validate_lines(lines: &[TextLine], page: &Page) -> Result<(), Error> {
    if lines.len() > 20_000 {
        return Err(Error::TooLarge);
    }
    for line in lines {
        let b = line.bounds;
        if line.text.is_empty()
            || line.text.len() > 4096
            || b.iter().any(|v| !v.is_finite())
            || b[0] < 0.0
            || b[1] < 0.0
            || b[2] > page.width
            || b[3] > page.height
            || b[0] >= b[2]
            || b[1] >= b[3]
        {
            return Err(Error::Unreadable);
        }
    }
    Ok(())
}

fn validate_png(bytes: &[u8], page: &Page, dimensions: (u32, u32)) -> Result<(), Error> {
    let actual = super::super::import::validate_png(bytes, page.width, page.height, true)
        .map_err(|_| Error::Unreadable)?;
    if actual != dimensions {
        return Err(Error::Unreadable);
    }
    Ok(())
}

/// Wire byte budget, including all headers and newlines.
#[must_use]
pub fn render_budget(pages: usize) -> usize {
    (IMAGE_LIMIT + TEXT_LIMIT + 16 * 1024) * pages + SMALL_LIMIT
}

struct Frames<R> {
    input: R,
    remaining: usize,
    terminal: Option<Error>,
}
impl<R: Read> Frames<R> {
    fn read(&mut self) -> Result<(Header, Vec<u8>), Error> {
        let mut line = Vec::with_capacity(HEADER_LIMIT);
        loop {
            if line.len() == HEADER_LIMIT || self.remaining == 0 {
                return Err(Error::TooLarge);
            }
            let mut byte = [0];
            self.input
                .read_exact(&mut byte)
                .map_err(|_| Error::Unreadable)?;
            self.remaining -= 1;
            line.push(byte[0]);
            if byte[0] == b'\n' {
                break;
            }
        }
        let header: Header = parse(&line)?;
        let length = header.length();
        if length > header.cap()
            || length > self.remaining
            || matches!(header, Header::Page { .. }) && length + line.len() > 2048
            || matches!(header, Header::Done { .. }) && length + line.len() > SMALL_LIMIT
        {
            return Err(Error::TooLarge);
        }
        if matches!(header, Header::Image { .. }) && length == 0 {
            return Err(Error::Unreadable);
        }
        self.remaining -= length;
        let mut bytes = vec![0; length];
        self.input
            .read_exact(&mut bytes)
            .map_err(|_| Error::Unreadable)?;
        if matches!(header, Header::Error { .. }) {
            let reason: Error = parse(&bytes)?;
            self.eof()?;
            self.terminal = Some(reason);
            return Err(if reason == Error::Timeout {
                Error::Unreadable
            } else {
                reason
            });
        }
        Ok((header, bytes))
    }
    fn done(&mut self, operation: Operation, counts: Counts) -> Result<(), Error> {
        let (header, bytes) = self.read()?;
        if !matches!(header, Header::Done { .. }) {
            return Err(Error::Unreadable);
        }
        let done: Done = parse(&bytes)?;
        if done.operation != operation || done.counts != counts {
            return Err(Error::Unreadable);
        }
        self.eof()
    }
    fn eof(&mut self) -> Result<(), Error> {
        let mut byte = [0];
        if self.input.read(&mut byte).map_err(|_| Error::Unreadable)? != 0 {
            return Err(Error::Unreadable);
        }
        Ok(())
    }
}

/// Read and validate the entire bounded inspection report.
///
/// # Errors
/// Refuses malformed, incomplete, out-of-order or over-budget output.
pub fn read_inspection<R: Read>(input: R) -> Result<Inspection, Error> {
    read_inspection_tracked(input).0
}

pub(super) fn read_inspection_tracked<R: Read>(
    input: R,
) -> (Result<Inspection, Error>, Option<Error>) {
    let mut frames = Frames {
        input: BufReader::new(input),
        remaining: INSPECT_LIMIT,
        terminal: None,
    };
    let result = inspection_frames(&mut frames);
    (result, frames.terminal)
}

fn inspection_frames<R: Read>(frames: &mut Frames<R>) -> Result<Inspection, Error> {
    let (header, bytes) = frames.read()?;
    if !matches!(header, Header::Document { .. }) {
        return Err(Error::Unreadable);
    }
    let document: Document = parse(&bytes)?;
    if document.pages > 2000 {
        return Err(Error::TooManyPages);
    }
    if document.pages == 0 || document.mupdf.is_empty() || document.mupdf.len() > 32 {
        return Err(Error::Unreadable);
    }
    let mut pages = Vec::new();
    for n in 1..=document.pages {
        let (header, bytes) = frames.read()?;
        if !matches!(header, Header::Page { .. }) {
            return Err(Error::Unreadable);
        }
        let page: Page = parse(&bytes)?;
        if page.n != n {
            return Err(Error::Unreadable);
        }
        geometry::validate_page(&page)?;
        pages.push(page);
    }
    frames.done(
        Operation::Inspect,
        Counts {
            document: 1,
            page: document.pages,
            ..Counts::default()
        },
    )?;
    Ok(Inspection { document, pages })
}

/// Stateful parent reader; retains only one page's payloads at a time. EOF is checked
/// before the final page is yielded. Discard the reader after any error.
/// JSON allocation bounds are per frame: `serde_json` may decode an escaped string into
/// its scratch buffer up to the frame's length before a field's own limit refuses it.
/// This is accepted; JSON payload frames are at most 8 MiB (PNG frames are separate).
pub struct RenderReader<R> {
    frames: Frames<BufReader<R>>,
    pages: Vec<Page>,
    index: usize,
    failed: bool,
}
impl<R: Read> RenderReader<R> {
    /// # Errors
    /// Refuses empty, oversized or duplicate page requests and invalid geometry.
    pub fn new(input: R, pages: Vec<Page>) -> Result<Self, Error> {
        if pages.is_empty() || pages.len() > 8 {
            return Err(Error::Unsupported);
        }
        let mut seen = std::collections::BTreeSet::new();
        for p in &pages {
            geometry::validate_page(p)?;
            if !seen.insert(p.n) {
                return Err(Error::Unsupported);
            }
        }
        Ok(Self {
            frames: Frames {
                input: BufReader::new(input),
                remaining: render_budget(pages.len()),
                terminal: None,
            },
            pages,
            index: 0,
            failed: false,
        })
    }
    /// # Errors
    /// Refuses any framing, ordering, completeness, image, statistics or text violation.
    pub fn next_page(&mut self) -> Result<Option<RenderedPage>, Error> {
        if self.failed {
            return Err(Error::Unreadable);
        }
        self.failed = true;
        let result = self.read_page();
        if result.is_ok() {
            self.failed = false;
        }
        result
    }
    pub(super) fn terminal_error(&self) -> Option<Error> {
        self.frames.terminal
    }
    fn read_page(&mut self) -> Result<Option<RenderedPage>, Error> {
        let Some(page) = self.pages.get(self.index) else {
            return Ok(None);
        };
        let (header, png) = self.frames.read()?;
        let Header::Image {
            n,
            width_px,
            height_px,
            dpi,
            ..
        } = header
        else {
            return Err(Error::Unreadable);
        };
        let dimensions = pixel_size(page.width, page.height)?;
        if n != page.n || dpi != 150 || (width_px, height_px) != dimensions {
            return Err(Error::Unreadable);
        }
        validate_png(&png, page, dimensions)?;
        let (header, bytes) = self.frames.read()?;
        if !matches!(header, Header::Stats { .. }) {
            return Err(Error::Unreadable);
        }
        let stats: Stats = parse(&bytes)?;
        validate_stats(&stats, n)?;
        let (header, bytes) = self.frames.read()?;
        if !matches!(header, Header::Text{n:nn,..} if nn == n) {
            return Err(Error::Unreadable);
        }
        let lines = parse::<BoundedVec<TextLine, 20_000>>(&bytes)?.0;
        validate_lines(&lines, page)?;
        self.index += 1;
        if self.index == self.pages.len() {
            let count = u32::try_from(self.index).map_err(|_| Error::TooLarge)?;
            self.frames.done(
                Operation::Render,
                Counts {
                    image: count,
                    stats: count,
                    text: count,
                    ..Counts::default()
                },
            )?;
        }
        Ok(Some(RenderedPage {
            n,
            png,
            width_px,
            height_px,
            dpi,
            stats,
            lines,
        }))
    }
}

/// Worker-side framed writer. The operation controls record order; the parent rechecks it.
pub(super) struct RecordWriter<W> {
    output: W,
    operation: Operation,
    counts: Counts,
    remaining: usize,
}
impl<W: Write> RecordWriter<W> {
    pub(super) fn error(mut self, reason: Error) -> Result<(), Error> {
        let bytes = json(&reason)?;
        self.frame(
            Header::Error {
                length: bytes.len(),
            },
            &bytes,
        )?;
        self.output.flush().map_err(|_| Error::Unreadable)
    }
    #[must_use]
    pub fn new(output: W, operation: Operation, pages: usize) -> Self {
        Self {
            output,
            operation,
            counts: Counts::default(),
            remaining: if operation == Operation::Inspect {
                INSPECT_LIMIT
            } else {
                render_budget(pages.min(8))
            },
        }
    }
    fn frame(&mut self, header: Header, bytes: &[u8]) -> Result<(), Error> {
        let mut line = json(&header)?;
        line.push(b'\n');
        let size = line.len() + bytes.len();
        if line.len() > HEADER_LIMIT
            || bytes.len() > header.cap()
            || size > self.remaining
            || matches!(header, Header::Page { .. }) && size > 2048
            || matches!(header, Header::Done { .. }) && size > SMALL_LIMIT
        {
            return Err(Error::TooLarge);
        }
        self.remaining -= size;
        self.output
            .write_all(&line)
            .and_then(|()| self.output.write_all(bytes))
            .map_err(|_| Error::Unreadable)
    }
    /// # Errors
    /// Refuses a record over a wire budget or an output failure.
    pub fn inspection(&mut self, inspection: &Inspection) -> Result<(), Error> {
        let bytes = json(&inspection.document)?;
        self.frame(
            Header::Document {
                length: bytes.len(),
            },
            &bytes,
        )?;
        self.counts.document += 1;
        for page in &inspection.pages {
            let bytes = json(page)?;
            self.frame(
                Header::Page {
                    length: bytes.len(),
                },
                &bytes,
            )?;
            self.counts.page += 1;
        }
        Ok(())
    }
    /// # Errors
    /// Refuses a record over a wire budget or an output failure.
    pub fn rendered(&mut self, page: &RenderedPage) -> Result<(), Error> {
        self.frame(
            Header::Image {
                n: page.n,
                length: page.png.len(),
                width_px: page.width_px,
                height_px: page.height_px,
                dpi: page.dpi,
            },
            &page.png,
        )?;
        self.counts.image += 1;
        let bytes = json(&page.stats)?;
        self.frame(
            Header::Stats {
                length: bytes.len(),
            },
            &bytes,
        )?;
        self.counts.stats += 1;
        let bytes = json(&page.lines)?;
        self.frame(
            Header::Text {
                n: page.n,
                length: bytes.len(),
            },
            &bytes,
        )?;
        self.counts.text += 1;
        Ok(())
    }
    /// # Errors
    /// Refuses an output failure or exceeded wire budget.
    pub fn finish(mut self) -> Result<(), Error> {
        let bytes = json(&Done {
            operation: self.operation,
            counts: self.counts,
        })?;
        self.frame(
            Header::Done {
                length: bytes.len(),
            },
            &bytes,
        )?;
        self.output.flush().map_err(|_| Error::Unreadable)
    }
}
