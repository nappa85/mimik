use super::*;

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
                input: synthetic(header, total),
                remaining: total,
            };
            assert_eq!(frames.read().unwrap().1.len(), header.length());
            assert_eq!(frames.remaining, 0);
            let mut frames = Frames {
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
        needs_password: false,
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
            input: synthetic(header, 2049),
            remaining: INSPECT_LIMIT
        }
        .read()
        .is_err()
    );
}
