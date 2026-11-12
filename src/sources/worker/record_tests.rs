use super::*;
use std::io::Cursor;

#[test]
fn refuses_extra_element_without_deserializing_it() {
    static COUNT: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
    struct Probe;
    impl<'de> Deserialize<'de> for Probe {
        fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
            COUNT.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            u32::deserialize(d)?;
            Ok(Self)
        }
    }
    assert!(serde_json::from_str::<BoundedVec<Probe, 1>>("[1,2]").is_err());
    assert_eq!(COUNT.load(std::sync::atomic::Ordering::SeqCst), 1);
}

#[test]
fn oversized_decoded_fields_are_refused() {
    for text in ["x".repeat(4097), "\\u0061".repeat(4097), "é".repeat(2049)] {
        let json = format!(r#"{{"text":"{text}","box":[0,0,1,1]}}"#);
        assert!(serde_json::from_str::<TextLine>(&json).is_err());
    }
    let mut stats = serde_json::to_value(rendered(1).stats).unwrap();
    stats["fonts_substituted"] = serde_json::json!(["x".repeat(65)]);
    assert!(serde_json::from_value::<Stats>(stats).is_err());
    let json = format!(r#"{{"text":"{}","box":[0,0,1,1]}}"#, "\\u0061".repeat(4096));
    assert_eq!(
        serde_json::from_str::<TextLine>(&json).unwrap().text.len(),
        4096
    );
}

#[test]
fn maximum_stats_fit_small_frame() {
    let mut p = rendered(2000);
    p.stats.chars = 1_000_000;
    p.stats.unmapped = 1_000_000;
    p.stats.fonts_skipped = 10000;
    p.stats.dropped_lines = 1_000_000;
    p.stats.image_area = 0.123_456_789_123_456_78;
    p.stats.fonts_substituted = vec!["\u{1}".repeat(64); FONT_LIMIT];
    let bytes = json(&p.stats).unwrap();
    assert!(
        bytes.len()
            + json(&Header::Stats {
                length: bytes.len()
            })
            .unwrap()
            .len()
            < SMALL_LIMIT
    );
}

#[test]
fn png_rejects_trailing_bytes_and_wrong_format() {
    let mut p = rendered(1);
    p.png.extend_from_slice(b"trailing");
    assert!(validate_png(&p.png, &page(1), (p.width_px, p.height_px)).is_err());
}

#[test]
fn png_requires_rgb_eight_bit() {
    let p = rendered(1);
    for (color, depth, channels) in [
        (png::ColorType::Rgba, png::BitDepth::Eight, 4),
        (png::ColorType::Rgb, png::BitDepth::Sixteen, 6),
    ] {
        let mut bytes = Vec::new();
        {
            let mut e = png::Encoder::new(&mut bytes, p.width_px, p.height_px);
            e.set_color(color);
            e.set_depth(depth);
            e.write_header()
                .unwrap()
                .write_image_data(&vec![
                    0;
                    usize::try_from(p.width_px * p.height_px * channels)
                        .unwrap()
                ])
                .unwrap();
        }
        assert!(validate_png(&bytes, &page(1), (p.width_px, p.height_px)).is_err());
    }
}

#[test]
fn inspection_buffers_pipe_reads() {
    struct CountReads<'a> {
        input: &'a [u8],
        calls: &'a std::cell::Cell<usize>,
    }
    impl Read for CountReads<'_> {
        fn read(&mut self, out: &mut [u8]) -> std::io::Result<usize> {
            self.calls.set(self.calls.get() + 1);
            self.input.read(out)
        }
    }
    let mut wire = Vec::new();
    let mut writer = RecordWriter::new(&mut wire, Operation::Inspect, 1);
    writer
        .inspection(&Inspection {
            document: Document {
                pages: 1,
                encrypted: false,
                mupdf: "1.27.2".into(),
            },
            pages: vec![page(1)],
        })
        .unwrap();
    writer.finish().unwrap();
    let calls = std::cell::Cell::new(0);
    assert!(
        read_inspection(CountReads {
            input: &wire,
            calls: &calls
        })
        .is_ok()
    );
    assert!(
        calls.get() <= 3,
        "{} pipe reads for {} bytes",
        calls.get(),
        wire.len()
    );
}

#[test]
fn terminal_error_is_returned_at_any_frame_boundary() {
    for reason in [
        Error::Unreadable,
        Error::Timeout,
        Error::TooLarge,
        Error::TooManyPages,
        Error::PasswordProtected,
        Error::Unsupported,
    ] {
        let payload = format!(
            "\"{}\"",
            match reason {
                Error::Unreadable => "unreadable",
                Error::Timeout => "timeout",
                Error::TooLarge => "too_large",
                Error::TooManyPages => "too_many_pages",
                Error::PasswordProtected => "password_protected",
                Error::Unsupported => "unsupported",
            }
        );
        let expected = if reason == Error::Timeout {
            Error::Unreadable
        } else {
            reason
        };
        let terminal = format!(
            "{{\"kind\":\"error\",\"length\":{}}}\n{payload}",
            payload.len()
        );
        assert_eq!(read_inspection(terminal.as_bytes()).unwrap_err(), expected);
        for prefix in 0..=3 {
            let mut wire = join(&split(valid())[..prefix]);
            wire.extend_from_slice(terminal.as_bytes());
            let mut r = RenderReader::new(&wire[..], vec![page(1)]).unwrap();
            assert_eq!(r.next_page().unwrap_err(), expected);
        }
    }
    assert_eq!(Error::from_exit_code(1), None);
    for payload in [
        br#""arbitrary source text""#.as_slice(),
        br#"{"reason":"too_large","text":"source"}"#.as_slice(),
    ] {
        let mut wire =
            format!("{{\"kind\":\"error\",\"length\":{}}}\n", payload.len()).into_bytes();
        wire.extend_from_slice(payload);
        assert_eq!(read_inspection(&wire[..]).unwrap_err(), Error::Unreadable);
    }
}

#[test]
fn worker_exit_status_cannot_claim_parent_timeout() {
    assert_eq!(Error::from_exit_code(21), Some(Error::Unreadable));
    assert_eq!(Error::Timeout.to_string(), "took too long");
    for reason in [
        Error::Unreadable,
        Error::TooLarge,
        Error::TooManyPages,
        Error::PasswordProtected,
        Error::Unsupported,
    ] {
        assert_eq!(Error::from_exit_code(reason.exit_code()), Some(reason));
    }
}

#[test]
fn entry_preserves_original_reason_when_error_frame_cannot_be_written() {
    struct BrokenPipe;
    impl std::io::Write for BrokenPipe {
        fn write(&mut self, _: &[u8]) -> std::io::Result<usize> {
            Err(std::io::Error::from_raw_os_error(libc::EPIPE))
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }
    let mut exhausted = RecordWriter::new(Vec::new(), Operation::Inspect, 0);
    exhausted.remaining = 0;
    let results = [
        super::super::entry::complete(exhausted, Err(Error::PasswordProtected)),
        super::super::entry::complete(
            RecordWriter::new(BrokenPipe, Operation::Inspect, 0),
            Err(Error::TooManyPages),
        ),
    ]
    .map(|result| result.unwrap_err().exit_code());
    assert_eq!(
        results,
        [
            Error::PasswordProtected.exit_code(),
            Error::TooManyPages.exit_code()
        ]
    );
}

fn page(n: u32) -> Page {
    Page {
        n,
        width: 10.0,
        height: 10.0,
        rotate: 0,
        user_unit: 1.0,
        transform: [1.0, 0.0, 0.0, -1.0, 0.0, 10.0],
        visible: [0.0, 0.0, 10.0, 10.0],
        tagged: false,
    }
}
fn rendered(n: u32) -> RenderedPage {
    let (w, h) = pixel_size(10.0, 10.0).unwrap();
    let mut png = Vec::new();
    {
        let mut encoder = png::Encoder::new(&mut png, w, h);
        encoder.set_color(png::ColorType::Rgb);
        encoder.set_depth(png::BitDepth::Eight);
        encoder
            .write_header()
            .unwrap()
            .write_image_data(&vec![255; usize::try_from(w * h * 3).unwrap()])
            .unwrap();
    }
    RenderedPage {
        n,
        png,
        width_px: w,
        height_px: h,
        dpi: 150,
        stats: Stats {
            n,
            chars: 1,
            unmapped: 0,
            image_area: 0.0,
            fonts_substituted: vec![],
            fonts_skipped: 0,
            incomplete: false,
            dropped_lines: 0,
        },
        lines: vec![TextLine {
            text: "X".into(),
            bounds: [1.0, 1.0, 2.0, 2.0],
        }],
    }
}
fn frame(header: Header, payload: &[u8]) -> Vec<u8> {
    let mut wire = serde_json::to_vec(&header).unwrap();
    wire.push(b'\n');
    wire.extend_from_slice(payload);
    wire
}
fn valid() -> Vec<u8> {
    let mut wire = Vec::new();
    let mut w = RecordWriter::new(&mut wire, Operation::Render, 1);
    w.rendered(&rendered(1)).unwrap();
    w.finish().unwrap();
    wire
}
fn split(wire: Vec<u8>) -> Vec<(Header, Vec<u8>)> {
    let mut frames = Frames {
        terminal: None,
        input: Cursor::new(wire),
        remaining: render_budget(1),
    };
    (0..4).map(|_| frames.read().unwrap()).collect()
}
fn join(frames: &[(Header, Vec<u8>)]) -> Vec<u8> {
    frames.iter().flat_map(|(h, p)| frame(*h, p)).collect()
}
fn refused(wire: &[u8]) {
    let mut reader = RenderReader::new(wire, vec![page(1)]).unwrap();
    assert!(reader.next_page().is_err());
    assert!(
        reader.next_page().is_err(),
        "failed reader must stay poisoned"
    );
}

#[test]
fn hostile_framing_order_and_completeness() {
    let good = valid();
    let mut reader = RenderReader::new(&good[..], vec![page(1)]).unwrap();
    assert_eq!(reader.next_page().unwrap().unwrap().n, 1);
    assert!(reader.next_page().unwrap().is_none());
    refused(&vec![b'x'; HEADER_LIMIT + 1]);
    refused(&frame(
        Header::Image {
            n: 1,
            length: IMAGE_LIMIT + 1,
            width_px: 21,
            height_px: 21,
            dpi: 150,
        },
        &[],
    ));
    refused(&good[..good.len() - 1]);
    let mut stray = good.clone();
    stray.extend_from_slice(b"stray");
    refused(&stray);
    let frames = split(good.clone());
    for omitted in 0..4 {
        let mut f = frames.clone();
        f.remove(omitted);
        refused(&join(&f));
    }
    let mut f = frames.clone();
    f.swap(0, 1);
    refused(&join(&f));
    let mut f = frames.clone();
    f.insert(1, f[0].clone());
    refused(&join(&f));
    let mut f = frames.clone();
    if let Header::Image { ref mut n, .. } = f[0].0 {
        *n = 2;
    }
    refused(&join(&f));
    let mut f = frames.clone();
    if let Header::Text { ref mut n, .. } = f[2].0 {
        *n = 2;
    }
    refused(&join(&f));
    let mut f = frames.clone();
    let bytes = serde_json::to_vec(&Done {
        operation: Operation::Render,
        counts: Counts::default(),
    })
    .unwrap();
    f[3] = (
        Header::Done {
            length: bytes.len(),
        },
        bytes,
    );
    refused(&join(&f));
    let mut mixed = frame(frames[0].0, &frames[0].1);
    mixed.extend_from_slice(b"warning: test\n");
    mixed.extend_from_slice(&join(&frames[1..]));
    refused(&mixed);
    let mut unknown=b"{\"kind\":\"image\",\"length\":0,\"n\":1,\"dpi\":150,\"width_px\":21,\"height_px\":21,\"extra\":true}\n".to_vec();
    unknown.extend_from_slice(&good);
    refused(&unknown);
}

#[test]
fn hostile_image_stats_and_text_values() {
    for invalid in 0..10 {
        let mut p = rendered(1);
        match invalid {
            0 => p.width_px += 1,
            1 => {
                p.png[16..20].copy_from_slice(&22_u32.to_be_bytes());
            }
            2 => p.dpi = 151,
            3 => p.stats.unmapped = 2,
            4 => p.stats.image_area = 1.1,
            5 => p.lines[0].bounds[0] = -1.0,
            6 => p.lines[0].bounds[2] = 11.0,
            7 => p.lines[0].bounds[2] = f64::NAN,
            8 => p.lines[0].text = "a".repeat(4097),
            _ => p.lines = vec![p.lines[0].clone(); 20_001],
        }
        let mut wire = Vec::new();
        let mut w = RecordWriter::new(&mut wire, Operation::Render, 1);
        w.rendered(&p).unwrap();
        w.finish().unwrap();
        refused(&wire);
    }
    let mut p = page(1);
    p.transform[0] = 2.0;
    assert!(RenderReader::new(&[][..], vec![p]).is_err());
    let mut p = page(1);
    p.visible[2] = 11.0;
    assert!(RenderReader::new(&[][..], vec![p]).is_err());
    assert!(RenderReader::new(&[][..], vec![page(1), page(1)]).is_err());
}

// The transport maximum tests deliberately use synthetic bytes: semantic PNG/text
// validation is covered separately, without spending CPU compressing 160 MiB of data.
struct RepeatedFrames {
    prefix: Cursor<Vec<u8>>,
    payload: std::io::Take<std::io::Repeat>,
    suffix: Cursor<Vec<u8>>,
}
struct Sequence(std::collections::VecDeque<Box<dyn Read>>);
impl Read for Sequence {
    fn read(&mut self, out: &mut [u8]) -> std::io::Result<usize> {
        while let Some(first) = self.0.front_mut() {
            let n = first.read(out)?;
            if n != 0 {
                return Ok(n);
            }
            self.0.pop_front();
        }
        Ok(0)
    }
}
impl Read for RepeatedFrames {
    fn read(&mut self, out: &mut [u8]) -> std::io::Result<usize> {
        let n = self.prefix.read(out)?;
        if n != 0 {
            return Ok(n);
        }
        let n = self.payload.read(out)?;
        if n != 0 {
            return Ok(n);
        }
        self.suffix.read(out)
    }
}
fn synthetic(header: Header, total: usize) -> RepeatedFrames {
    let mut prefix = serde_json::to_vec(&header).unwrap();
    prefix.resize(total - header.length() - 1, b' ');
    prefix.push(b'\n');
    RepeatedFrames {
        prefix: Cursor::new(prefix),
        payload: std::io::repeat(0).take(u64::try_from(header.length()).unwrap()),
        suffix: Cursor::new(Vec::new()),
    }
}

#[test]
fn maximum_render_transport_and_one_byte_more() {
    let mut sequence = Sequence(std::collections::VecDeque::new());
    for _ in 0..8 {
        for (header, total) in [
            (
                Header::Image {
                    n: 1,
                    length: IMAGE_LIMIT,
                    width_px: 1,
                    height_px: 1,
                    dpi: 150,
                },
                IMAGE_LIMIT + HEADER_LIMIT,
            ),
            (Header::Stats { length: 4096 }, 4096 + HEADER_LIMIT),
            (
                Header::Text {
                    n: 1,
                    length: TEXT_LIMIT,
                },
                TEXT_LIMIT + HEADER_LIMIT,
            ),
        ] {
            let mut frames = Frames {
                terminal: None,
                input: synthetic(header, total),
                remaining: total,
            };
            assert_eq!(frames.read().unwrap().1.len(), header.length());
            assert_eq!(frames.remaining, 0);
            let mut frames = Frames {
                terminal: None,
                input: synthetic(header, total),
                remaining: total - 1,
            };
            assert!(frames.read().is_err());
            sequence.0.push_back(Box::new(synthetic(header, total)));
        }
    }
    let done = serde_json::to_vec(&Done {
        operation: Operation::Render,
        counts: Counts {
            image: 8,
            stats: 8,
            text: 8,
            ..Counts::default()
        },
    })
    .unwrap();
    let mut done_header = serde_json::to_vec(&Header::Done { length: done.len() }).unwrap();
    done_header.resize(4095 - done.len(), b' ');
    done_header.push(b'\n');
    done_header.extend_from_slice(&done);
    sequence.0.push_back(Box::new(Cursor::new(done_header)));
    let mut frames = Frames {
        terminal: None,
        input: sequence,
        remaining: render_budget(8),
    };
    for _ in 0..24 {
        frames.read().unwrap();
    }
    frames
        .done(
            Operation::Render,
            Counts {
                image: 8,
                stats: 8,
                text: 8,
                ..Counts::default()
            },
        )
        .unwrap();
    assert_eq!(frames.remaining, 0);
    assert_eq!(
        render_budget(8),
        (IMAGE_LIMIT + TEXT_LIMIT + 16 * 1024) * 8 + 4096
    );
    let header = Header::Text {
        n: 1,
        length: TEXT_LIMIT + 1,
    };
    assert!(
        Frames {
            terminal: None,
            input: synthetic(header, TEXT_LIMIT + 1 + HEADER_LIMIT),
            remaining: render_budget(1)
        }
        .read()
        .is_err()
    );
}

#[test]
fn maximum_inspect_transport_and_one_byte_more() {
    let header = Header::Page { length: 2000 };
    let mut sequence = Sequence(std::collections::VecDeque::new());
    let document = serde_json::to_vec(&Document {
        pages: 2000,
        encrypted: false,
        mupdf: "1.27.2".into(),
    })
    .unwrap();
    sequence.0.push_back(Box::new(Cursor::new(frame(
        Header::Document {
            length: document.len(),
        },
        &document,
    ))));
    for n in 1..=2000 {
        let mut payload = serde_json::to_vec(&page(n)).unwrap();
        payload.resize(2000, b' ');
        let mut prefix = serde_json::to_vec(&header).unwrap();
        prefix.resize(47, b' ');
        prefix.push(b'\n');
        prefix.extend(payload);
        sequence.0.push_back(Box::new(Cursor::new(prefix)));
    }
    let done = serde_json::to_vec(&Done {
        operation: Operation::Inspect,
        counts: Counts {
            document: 1,
            page: 2000,
            ..Counts::default()
        },
    })
    .unwrap();
    sequence.0.push_back(Box::new(Cursor::new(frame(
        Header::Done { length: done.len() },
        &done,
    ))));
    assert_eq!(read_inspection(sequence).unwrap().pages.len(), 2000);
    assert!(
        Frames {
            terminal: None,
            input: synthetic(header, 2049),
            remaining: INSPECT_LIMIT
        }
        .read()
        .is_err()
    );
}

// Words: one frame per requested page, validated against the page like text lines.
fn word(text: &str, bounds: [f64; 4]) -> Word {
    Word {
        text: text.into(),
        bounds,
    }
}
fn words_wire(pages: &[PageWords]) -> Vec<u8> {
    let mut wire = Vec::new();
    let mut w = RecordWriter::new(&mut wire, Operation::Words, pages.len());
    for page in pages {
        w.words(page).unwrap();
    }
    w.finish().unwrap();
    wire
}
fn page_words(n: u32, words: Vec<Word>) -> PageWords {
    PageWords { n, words }
}
fn words_refused(wire: &[u8], pages: Vec<Page>) {
    let mut reader = WordsReader::new(wire, pages).unwrap();
    let result = (|| {
        while reader.next_page()?.is_some() {}
        Ok::<(), Error>(())
    })();
    assert!(result.is_err());
    assert!(
        reader.next_page().is_err(),
        "failed reader must stay poisoned"
    );
}

#[test]
fn words_round_trip_in_requested_order() {
    let a = page_words(2, vec![word("one", [1.0, 1.0, 4.0, 3.0])]);
    let b = page_words(1, vec![]);
    let wire = words_wire(&[a, b]);
    let mut reader = WordsReader::new(&wire[..], vec![page(2), page(1)]).unwrap();
    let first = reader.next_page().unwrap().unwrap();
    assert_eq!((first.n, first.words[0].text.as_str()), (2, "one"));
    assert_eq!(reader.next_page().unwrap().unwrap().words.len(), 0);
    assert!(reader.next_page().unwrap().is_none());
}

#[test]
fn hostile_word_values_are_refused() {
    let good = || word("w", [1.0, 1.0, 2.0, 2.0]);
    for invalid in 0..12 {
        let mut w = good();
        let mut count = 1;
        match invalid {
            0 => w.bounds[0] = -1.0,
            1 => w.bounds[2] = 11.0,
            2 => w.bounds[3] = 10.5,
            3 => w.bounds[1] = f64::NAN,
            4 => w.bounds = [2.0, 1.0, 1.0, 2.0],
            5 => w.bounds = [1.0, 1.0, 1.0, 2.0],
            6 => w.text = String::new(),
            7 => w.text = "a".repeat(257),
            8 => w.bounds[2] = f64::INFINITY,
            9 => w.text = "a\u{0}".into(),
            10 => w.text = "a\u{9c}b".into(),
            _ => count = MAX_WORDS + 1,
        }
        let mut wire = Vec::new();
        let mut writer = RecordWriter::new(&mut wire, Operation::Words, 1);
        writer.words(&page_words(1, vec![w; count])).unwrap();
        writer.finish().unwrap();
        words_refused(&wire, vec![page(1)]);
    }
    // The bounds themselves are accepted: 256 bytes, 20,000 words, the page edge.
    let edge = Word {
        text: "a".repeat(256),
        bounds: [0.0, 0.0, 10.0, 10.0],
    };
    let wire = words_wire(&[page_words(1, vec![edge; MAX_WORDS])]);
    let mut reader = WordsReader::new(&wire[..], vec![page(1)]).unwrap();
    assert_eq!(reader.next_page().unwrap().unwrap().words.len(), MAX_WORDS);
}

#[test]
fn oversized_decoded_word_text_is_refused() {
    for text in ["x".repeat(257), "\\u0061".repeat(257), "\u{e9}".repeat(129)] {
        let json = format!(r#"{{"text":"{text}","box":[0,0,1,1]}}"#);
        assert!(serde_json::from_str::<Word>(&json).is_err());
    }
    let json = format!(r#"{{"text":"{}","box":[0,0,1,1]}}"#, "\u{e9}".repeat(128));
    assert_eq!(serde_json::from_str::<Word>(&json).unwrap().text.len(), 256);
}

#[test]
fn hostile_words_framing_order_and_completeness() {
    let pages = || vec![page(1), page(2)];
    let wire = words_wire(&[
        page_words(1, vec![word("a", [1.0, 1.0, 2.0, 2.0])]),
        page_words(2, vec![]),
    ]);
    let mut frames = Frames {
        terminal: None,
        input: Cursor::new(wire.clone()),
        remaining: words_budget(2),
    };
    let parts: Vec<_> = (0..3).map(|_| frames.read().unwrap()).collect();
    words_refused(&wire[..wire.len() - 1], pages());
    let mut stray = wire.clone();
    stray.extend_from_slice(b"stray");
    words_refused(&stray, pages());
    for omitted in 0..3 {
        let mut f = parts.clone();
        f.remove(omitted);
        words_refused(&join(&f), pages());
    }
    let mut f = parts.clone();
    f.swap(0, 1);
    words_refused(&join(&f), pages());
    let mut f = parts.clone();
    if let Header::Words { ref mut n, .. } = f[0].0 {
        *n = 2;
    }
    words_refused(&join(&f), pages());
    // A render's text frame is not a words frame, and the done counts must say `words`.
    let mut f = parts.clone();
    if let Header::Words { n, length } = f[0].0 {
        f[0].0 = Header::Text { n, length };
    }
    words_refused(&join(&f), pages());
    let mut f = parts;
    let bytes = serde_json::to_vec(&Done {
        operation: Operation::Words,
        counts: Counts {
            text: 2,
            ..Counts::default()
        },
    })
    .unwrap();
    f[2] = (
        Header::Done {
            length: bytes.len(),
        },
        bytes,
    );
    words_refused(&join(&f), pages());
    // Requests are bounded like render's.
    assert!(WordsReader::new(&[][..], vec![]).is_err());
    assert!(WordsReader::new(&[][..], vec![page(1), page(1)]).is_err());
    assert!(WordsReader::new(&[][..], (1..=9).map(page).collect()).is_err());
}

#[test]
fn words_frames_have_their_own_bound_and_budget() {
    assert_eq!(words_budget(8), (WORDS_LIMIT + 16 * 1024) * 8 + 4096);
    let header = Header::Words {
        n: 1,
        length: WORDS_LIMIT + 1,
    };
    assert!(
        Frames {
            terminal: None,
            input: synthetic(header, WORDS_LIMIT + 1 + HEADER_LIMIT),
            remaining: words_budget(1)
        }
        .read()
        .is_err()
    );
    let header = Header::Words {
        n: 1,
        length: WORDS_LIMIT,
    };
    assert_eq!(
        Frames {
            terminal: None,
            input: synthetic(header, WORDS_LIMIT + HEADER_LIMIT),
            remaining: words_budget(1)
        }
        .read()
        .unwrap()
        .1
        .len(),
        WORDS_LIMIT
    );
}
