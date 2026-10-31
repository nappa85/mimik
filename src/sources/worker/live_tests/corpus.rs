//! All inputs are generated; no manuals, binary fixtures or decompressed bombs are retained.
use super::*;
use std::io::Write;

const HOSTILE: &[&str] = &[
    "deep-arrays",
    "deep-dictionaries",
    "flate-gib",
    "nested-filters",
    "image-bomb",
    "tiny-images",
    "prev-loop",
    "object-stream-cycle",
    "xref-repair",
    "huge-object-count",
    "page-cycle",
    "huge-page-count",
    "operators-50m",
    "type3-recursion",
    "shading-pattern",
    "huge-mediabox",
    "extreme-userunit",
    "bad-jbig2",
    "bad-jpx",
    "bad-font",
    "inert-actions",
    "truncated",
    "not-pdf",
    "password",
];
const VALID: &[&str] = &[
    "text-300",
    "full-image",
    "many-fonts",
    "rotate-90",
    "rotate-180",
    "rotate-270",
];

fn budget() -> Duration {
    static START: std::sync::OnceLock<Instant> = std::sync::OnceLock::new();
    // Shared by all corpus tests in the confinement process. Leave 30 seconds for
    // bounded generation/cleanup/joins, and fail closed rather than overrun CI's 10 min.
    Duration::from_secs(570)
        .checked_sub(START.get_or_init(Instant::now).elapsed())
        .expect("corpus exhausted its ten-minute CI budget")
}

fn bound_wall(opts: &mut Options, valid: bool, pages: usize) {
    let normal = if valid { 30 * pages.max(1) as u64 } else { 5 };
    opts.wall = Some(Duration::from_secs(normal).min(budget()));
}

fn compressed(block: &[u8], count: usize) -> Vec<u8> {
    let mut encoder = flate2::write::ZlibEncoder::new(Vec::new(), flate2::Compression::fast());
    for _ in 0..count {
        encoder.write_all(block).unwrap();
    }
    encoder.finish().unwrap()
}

fn zero_bomb() -> Vec<u8> {
    // Full flush emits byte-aligned, non-final blocks and resets the dictionary. Such
    // independent blocks can be repeated without compressing a gigabyte on the test host.
    // flate2 produces every DEFLATE byte; only the RFC1950 checksum reflects repetition.
    let block = vec![0; 1 << 20];
    let mut compressor = flate2::Compress::new(flate2::Compression::fast(), true);
    let mut encoded = Vec::with_capacity(2 << 20);
    compressor
        .compress_vec(&block, &mut encoded, flate2::FlushCompress::Full)
        .unwrap();
    assert_eq!(compressor.total_in(), block.len() as u64);
    let mut output = Vec::with_capacity(6 << 20);
    output.extend_from_slice(&encoded[..2]); // zlib header appears exactly once
    for _ in 0..1024 {
        output.extend_from_slice(&encoded[2..]);
    }
    encoded.clear();
    assert_eq!(
        compressor
            .compress_vec(&[], &mut encoded, flate2::FlushCompress::Finish)
            .unwrap(),
        flate2::Status::StreamEnd
    );
    output.extend_from_slice(&encoded[..encoded.len() - 4]);
    // Adler32 of N zero bytes: s1=1, s2=N mod 65521.
    let checksum = (((1_u32 << 30) % 65521) << 16) | 1;
    output.extend_from_slice(&checksum.to_be_bytes());
    output
}

fn stream(dict: &str, bytes: &[u8]) -> Vec<u8> {
    let mut out = format!("<< /Length {} {dict} >>\nstream\n", bytes.len()).into_bytes();
    out.extend_from_slice(bytes);
    out.extend_from_slice(b"\nendstream");
    out
}

// Small raw writer deliberately permits malformed syntax that lopdf normalizes away.
fn raw(objects: &[Vec<u8>], trailer: &str, repair: bool) -> Vec<u8> {
    let mut out = b"%PDF-1.7\n".to_vec();
    let mut offsets = vec![0];
    for (n, object) in objects.iter().enumerate() {
        offsets.push(out.len());
        writeln!(out, "{} 0 obj", n + 1).unwrap();
        out.extend_from_slice(object);
        out.extend_from_slice(b"\nendobj\n");
    }
    let xref = out.len();
    write!(out, "xref\n0 {}\n0000000000 65535 f \n", objects.len() + 1).unwrap();
    for offset in offsets.iter().skip(1) {
        writeln!(out, "{offset:010} 00000 n ").unwrap();
    }
    write!(
        out,
        "trailer\n<< /Root 1 0 R /Size {} {} >>\nstartxref\n{}\n%%EOF\n",
        objects.len() + 1,
        trailer.replace("SELF", &xref.to_string()),
        if repair { 1 } else { xref }
    )
    .unwrap();
    out
}

fn generated(name: &str) -> Vec<u8> {
    let mut catalog = "<< /Type /Catalog /Pages 2 0 R >>".to_owned();
    let mut tree = "<< /Type /Pages /Count 1 /Kids [3 0 R] >>".to_owned();
    let mut page =
        "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 600 800] /Resources 5 0 R /Contents 4 0 R >>"
            .to_owned();
    let mut content = b"BT /F1 12 Tf 40 700 Td (Synthetic difficult page) Tj ET".to_vec();
    let mut filters = "";
    let mut resources = "<< /Font << /F1 6 0 R >> >>".to_owned();
    let mut font = b"<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica >>".to_vec();
    let mut extra = Vec::new();
    let mut trailer = "";
    match name {
        "deep-arrays" | "deep-dictionaries" => {
            let (open, close) = if name == "deep-arrays" {
                ("[", "]")
            } else {
                ("<< /X ", ">>")
            };
            catalog = format!(
                "<< /Type /Catalog /Pages 2 0 R /Deep {}0{} >>",
                open.repeat(100_000),
                close.repeat(100_000)
            );
        }
        "flate-gib" => {
            content = zero_bomb();
            filters = "/Filter /FlateDecode";
        }
        "nested-filters" => {
            content = compressed(&vec![b' '; 65536], 64);
            for _ in 0..7 {
                content = compressed(&content, 1);
            }
            filters = "/Filter [/FlateDecode /FlateDecode /FlateDecode /FlateDecode /FlateDecode /FlateDecode /FlateDecode /FlateDecode]";
        }
        "operators-50m" => {
            content = compressed(&b"q Q\n".repeat(8000), 3125);
            filters = "/Filter /FlateDecode";
        }
        "page-cycle" => tree = "<< /Type /Pages /Count 1 /Kids [2 0 R 3 0 R] >>".into(),
        "huge-page-count" => tree = "<< /Type /Pages /Count 2147483648 /Kids [3 0 R] >>".into(),
        "huge-mediabox" => page = page.replace("600 800", "1000000000 1000000000"),
        "extreme-userunit" => {
            page = page.replace("/Type /Page", "/UserUnit 1000000000 /Type /Page");
        }
        "prev-loop" => trailer = "/Prev SELF",
        "huge-object-count" => trailer = "/Size 2147483648",
        "object-stream-cycle" => {
            extra.push(stream(
                "/Type /ObjStm /N 1 /First 4 /Extends 8 0 R",
                b"8 0 8 0 R",
            ));
            extra.push(stream(
                "/Type /ObjStm /N 1 /First 4 /Extends 7 0 R",
                b"7 0 7 0 R",
            ));
            catalog = "<< /Type /Catalog /Pages 2 0 R /Loop 7 0 R >>".into();
        }
        "xref-repair" => {
            extra.push(b"\n999 0 obj << /Broken true >> endobj\n".repeat(170_000));
        }
        "image-bomb" | "tiny-images" | "full-image" | "bad-jbig2" | "bad-jpx" => {
            extra.push(image(name));
            resources = "<< /XObject << /Im 7 0 R >> >>".into();
            content = if name == "tiny-images" {
                b"q 1 0 0 1 0 0 cm /Im Do Q\n".repeat(50_000)
            } else {
                b"q 600 0 0 800 0 0 cm /Im Do Q".to_vec()
            };
        }
        "inert-actions" => {
            catalog = "<< /Type /Catalog /Pages 2 0 R /OpenAction 7 0 R /Names << /EmbeddedFiles << /Names [(payload) 8 0 R] >> >> >>".into();
            extra.push(b"<< /S /JavaScript /JS (app.launchURL('http://127.0.0.1:9/');) /Next << /S /Launch /F (/tmp/mimik-must-not-exist) >> >>".to_vec());
            extra.push(b"<< /Type /Filespec /F (payload) /EF << /F 9 0 R >> >>".to_vec());
            extra.push(stream("/Type /EmbeddedFile", b"synthetic inert payload"));
            page = page.replace(
                "/Type /Page",
                "/AA << /O << /S /URI /URI (http://127.0.0.1:9/) >> >> /Type /Page",
            );
        }
        "bad-font" | "type3-recursion" | "shading-pattern" | "many-fonts" => {
            resource_case(name, &mut font, &mut extra, &mut content, &mut resources);
        }
        "text-300" | "password" => {}
        _ => panic!("unknown corpus case {name}"),
    }
    let objects = vec![
        catalog.into_bytes(),
        tree.into_bytes(),
        page.into_bytes(),
        stream(filters, &content),
        resources.into_bytes(),
        font,
    ];
    finish(
        objects.into_iter().chain(extra).collect(),
        &content,
        name,
        trailer,
    )
}

fn image(name: &str) -> Vec<u8> {
    let (w, h) = match name {
        "image-bomb" => (100_000, 100_000),
        "full-image" => (3000, 4000),
        _ => (1, 1),
    };
    let (filter, bytes) = match name {
        "bad-jbig2" => ("/JBIG2Decode", vec![0xff; 1024]),
        "bad-jpx" => ("/JPXDecode", vec![0xff; 1024]),
        "full-image" => ("/FlateDecode", compressed(&[127; 3000], 4000)),
        _ => ("/FlateDecode", compressed(&[127], 1)),
    };
    stream(
        &format!(
            "/Type /XObject /Subtype /Image /Width {w} /Height {h} /ColorSpace /DeviceGray /BitsPerComponent 8 /Filter {filter}"
        ),
        &bytes,
    )
}

fn resource_case(
    name: &str,
    font: &mut Vec<u8>,
    extra: &mut Vec<Vec<u8>>,
    content: &mut Vec<u8>,
    resources: &mut String,
) {
    match name {
        "bad-font" => {
            *font = b"<< /Type /Font /Subtype /TrueType /BaseFont /Broken /FontDescriptor 7 0 R >>"
                .to_vec();
            extra.push(b"<< /Type /FontDescriptor /FontName /Broken /Flags 4 /FontBBox [0 0 1000 1000] /ItalicAngle 0 /Ascent 800 /Descent -200 /CapHeight 700 /StemV 80 /FontFile2 8 0 R >>".to_vec());
            extra.push(stream("", &[0xff; 4096]));
        }
        "type3-recursion" => {
            *font = b"<< /Type /Font /Subtype /Type3 /FontBBox [0 0 100 100] /FontMatrix [.01 0 0 .01 0 0] /FirstChar 65 /LastChar 65 /Widths [100] /Encoding << /Type /Encoding /Differences [65 /A] >> /CharProcs << /A 7 0 R >> /Resources << /Font << /F1 6 0 R >> >> >>".to_vec();
            extra.push(stream("", b"100 0 d0 BT /F1 10 Tf (A) Tj ET"));
            *content = b"BT /F1 12 Tf 40 700 Td (A) Tj ET".to_vec();
        }
        "shading-pattern" => {
            *resources = "<< /Pattern << /P 7 0 R >> /Shading << /S 8 0 R >> >>".into();
            extra.push(stream("/Type /Pattern /PatternType 1 /PaintType 1 /TilingType 1 /BBox [0 0 1 1] /XStep .01 /YStep .01 /Resources << >>", b"0 0 .5 .5 re f"));
            extra.push(b"<< /ShadingType 2 /ColorSpace /DeviceRGB /Coords [0 0 600 800] /Function << /FunctionType 2 /Domain [0 1] /C0 [0 0 0] /C1 [1 1 1] /N 1 >> /Extend [true true] >>".to_vec());
            *content = b"/Pattern cs /P scn 0 0 600 800 re f /S sh\n".repeat(10_000);
        }
        "many-fonts" => {
            let mut names = Vec::new();
            content.clear();
            for n in 0..256 {
                write!(names, "/F{n} {} 0 R ", n + 7).unwrap();
                writeln!(
                    content,
                    "BT /F{n} 8 Tf 40 {} Td (Font {n}) Tj ET",
                    20 + n * 2
                )
                .unwrap();
            }
            *resources = format!("<< /Font << {} >> >>", String::from_utf8(names).unwrap());
            extra.extend((0..256).map(|_| font.clone()));
        }
        _ => unreachable!(),
    }
}

fn finish(mut objects: Vec<Vec<u8>>, content: &[u8], name: &str, trailer: &str) -> Vec<u8> {
    if name == "text-300" {
        let mut kids = Vec::new();
        for n in 3..=302 {
            write!(kids, "{n} 0 R ").unwrap();
        }
        let kids = String::from_utf8(kids).unwrap();
        let page = b"<< /Type /Page /Parent 2 0 R /MediaBox [0 0 600 800] /Resources 304 0 R /Contents 303 0 R >>".to_vec();
        objects = vec![
            objects[0].clone(),
            format!("<< /Type /Pages /Count 300 /Kids [{kids}] >>").into_bytes(),
        ];
        objects.extend(std::iter::repeat_n(page, 300));
        objects.push(stream("", content));
        objects.push(b"<< /Font << /F1 305 0 R >> >>".to_vec());
        objects.push(b"<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica >>".to_vec());
    }
    let bytes = raw(&objects, trailer, name == "xref-repair");
    if name == "password" {
        let mut doc = lopdf::Document::load_mem(&bytes).unwrap();
        doc.trailer.set(
            "ID",
            lopdf::Object::Array(vec![
                lopdf::Object::string_literal("synthetic-corpus-1"),
                lopdf::Object::string_literal("synthetic-corpus-1"),
            ]),
        );
        let encryption = lopdf::EncryptionState::try_from(lopdf::EncryptionVersion::V2 {
            document: &doc,
            owner_password: "owner",
            user_password: "required",
            key_length: 128,
            permissions: lopdf::Permissions::default(),
        })
        .unwrap();
        doc.encrypt(&encryption).unwrap();
        let mut out = Vec::new();
        doc.save_to(&mut out).unwrap();
        out
    } else {
        bytes
    }
}

fn generate(name: &str) -> Vec<u8> {
    let _ = budget();
    let started = Instant::now();
    let bytes = if let Some(angle) = name.strip_prefix("rotate-") {
        super::super::tests::fixture(angle.parse().unwrap(), [10, 20, 190, 280], 1.0, true)
    } else {
        match name {
            "not-pdf" => b"not a PDF".to_vec(),
            "truncated" => b"%PDF-1.7\n1 0 obj << /Type /Catalog".to_vec(),
            _ => generated(name),
        }
    };
    assert!(bytes.len() <= 8 << 20, "{name}: output limit");
    assert!(
        started.elapsed() < Duration::from_secs(5),
        "{name}: generation took {:?}",
        started.elapsed()
    );
    bytes
}

fn run_case(name: &str, valid: bool) {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("corpus.pdf");
    std::fs::write(&path, generate(name)).unwrap();
    let mut opts = production_options();
    bound_wall(&mut opts, valid, 1);
    let start = Instant::now();
    let report = inspection(&path, &opts, &Cancel::default());
    assert!(
        start.elapsed() < opts.wall.unwrap() + Duration::from_secs(5),
        "{name}: inspect deadline"
    );
    let report = match report {
        Ok(report) => report,
        Err(Reason::Document(reason)) if !valid => {
            println!("corpus {name}: inspect {reason}");
            return;
        }
        other => panic!("{name}: {other:?}"),
    };
    check_rss(name, "inspect", &opts);
    if name == "text-300" {
        assert_eq!(report.document.pages, 300);
    }
    let pages = if valid {
        report.pages.clone()
    } else {
        report.pages[..1].to_vec()
    };
    for batch in pages.chunks(8) {
        bound_wall(&mut opts, valid, batch.len());
        let spec = Spec {
            operation: records::Operation::Render,
            pages: batch.iter().map(|p| p.n).collect(),
            settings: RenderSettings::default(),
            input: "/in/document.pdf".into(),
        };
        let start = Instant::now();
        let result = execute(&path, &spec, &Cancel::default(), &opts, |pipe| {
            let mut reader = records::RenderReader::new(pipe, batch.to_vec()).unwrap();
            let result = (|| {
                while reader.next_page()?.is_some() {}
                Ok(())
            })();
            (result, reader.terminal_error())
        });
        assert!(
            start.elapsed() < opts.wall.unwrap() + Duration::from_secs(5),
            "{name}: render deadline"
        );
        if valid {
            result.unwrap_or_else(|error| panic!("{name}: {error}"));
        } else {
            assert!(
                matches!(result, Ok(()) | Err(Reason::Document(_))),
                "{name}: {result:?}"
            );
        }
        check_rss(name, "render", &opts);
    }
    // No staging output is permitted; execute returns only after all pinned processes die.
    assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 1);
    dir.close().unwrap();
}

fn check_rss(name: &str, operation: &str, opts: &Options) {
    let rss = opts.peak_rss_kib.load(std::sync::atomic::Ordering::Acquire);
    println!(
        "corpus {name} {operation}: peak RSS {rss} KiB (wait4 reaped runner and waited descendants; conservative worker upper bound; RLIMIT_AS=2097152 KiB; Vec and MuPDF buffer both present)"
    );
    assert!(
        rss > 0 && rss < 1_572_864,
        "{name}: RSS must be below 75% of RLIMIT_AS"
    );
}

#[test]
fn corpus_generators_are_bounded() {
    for name in HOSTILE.iter().chain(VALID) {
        drop(generate(name));
    }
    let compressed = zero_bomb();
    let mut decoder = flate2::read::ZlibDecoder::new(compressed.as_slice());
    assert_eq!(
        std::io::copy(&mut decoder, &mut std::io::sink()).unwrap(),
        1 << 30
    );
}

#[test]
#[ignore = "host confinement: MIMIK_LIVE_RUNNER opt-in"]
fn live_hostile_corpus() {
    if !enabled() {
        return;
    }
    let started = Instant::now();
    for name in HOSTILE {
        run_case(name, false);
    }
    assert!(started.elapsed() < Duration::from_secs(600));
}

#[test]
#[ignore = "host confinement: MIMIK_LIVE_RUNNER opt-in"]
fn live_difficult_valid_corpus_and_peak_rss() {
    if !enabled() {
        return;
    }
    let started = Instant::now();
    for name in VALID {
        run_case(name, true);
    }
    assert!(started.elapsed() < Duration::from_secs(600));
}

#[test]
#[ignore = "host confinement: MIMIK_LIVE_RUNNER opt-in"]
fn live_parent_serves_inspect_during_hostile_render() {
    if !enabled() {
        return;
    }
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("hostile.pdf");
    std::fs::write(&path, generate("operators-50m")).unwrap();
    let opts = production_options();
    let report = inspection(&path, &opts, &Cancel::default()).unwrap();
    let (tx, rx) = std::sync::mpsc::channel();
    let handle = std::thread::spawn(move || {
        let spec = Spec {
            operation: records::Operation::Render,
            pages: vec![1],
            settings: RenderSettings::default(),
            input: "/in/document.pdf".into(),
        };
        execute(&path, &spec, &Cancel::default(), &opts, |pipe| {
            // Watchdog is active and owns the process tree before this notification.
            tx.send(()).unwrap();
            let mut reader = records::RenderReader::new(pipe, report.pages).unwrap();
            let result = reader.next_page().map(|_| ());
            (result, reader.terminal_error())
        })
    });
    rx.recv_timeout(Duration::from_secs(5)).unwrap();
    let start = Instant::now();
    launch::probe_with(&production_options()).unwrap();
    assert!(start.elapsed() < Duration::from_secs(5));
    assert!(
        !handle.is_finished(),
        "hostile render must overlap the second request"
    );
    assert!(matches!(
        handle.join().unwrap(),
        Ok(()) | Err(Reason::Document(_))
    ));
}
