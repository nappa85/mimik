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
    repeated_flate(&vec![0; 1 << 20], 1024)
}

fn repeated_flate(block: &[u8], count: usize) -> Vec<u8> {
    // Full flush emits byte-aligned, non-final blocks and resets the dictionary. Such
    // independent blocks can be repeated without compressing a gigabyte on the test host.
    // flate2 produces every DEFLATE byte; only the RFC1950 checksum reflects repetition.
    let mut compressor = flate2::Compress::new(flate2::Compression::fast(), true);
    let mut encoded = Vec::with_capacity(2 << 20);
    compressor
        .compress_vec(block, &mut encoded, flate2::FlushCompress::Full)
        .unwrap();
    assert_eq!(compressor.total_in(), block.len() as u64);
    let mut output = Vec::with_capacity(6 << 20);
    output.extend_from_slice(&encoded[..2]); // zlib header appears exactly once
    for _ in 0..count {
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
    // Combine Adler32 for repeated independent blocks, without traversing expanded data.
    let (mut a, mut b) = (1_u64, 0_u64);
    for byte in block {
        a = (a + u64::from(*byte)) % 65521;
        b = (b + a) % 65521;
    }
    let (mut total_a, mut total_b) = (1_u64, 0_u64);
    for _ in 0..count {
        total_b = (total_b + b + (block.len() as u64 % 65521) * (total_a + 65520)) % 65521;
        total_a = (total_a + a + 65520) % 65521;
    }
    let checksum = u32::try_from((total_b << 16) | total_a).unwrap();
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
        "trailer\n<< /Root 1 0 R {} {} >>\nstartxref\n{}\n%%EOF\n",
        if trailer.contains("/Size ") {
            String::new()
        } else {
            format!("/Size {}", objects.len() + 1)
        },
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
            content = repeated_flate(&b"q Q\n".repeat(8000), 3125);
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
            page = page.replace("/Resources 5 0 R", "/Resources 9 0 R");
            // Object 9 (page resources) requires stream B. Its indirect Length 10
            // requires stream A, whose indirect Length is object 9 in B again.
            extra.push(
                b"<< /Type /ObjStm /N 1 /First 5 /Length 9 0 R >>\nstream\n10 0 29\nendstream"
                    .to_vec(),
            );
            extra.push(b"<< /Type /ObjStm /N 1 /First 4 /Length 10 0 R >>\nstream\n9 0 << /Font << /F1 6 0 R >> >>\nendstream".to_vec());
        }
        "xref-repair" => {
            extra.push(b"\n999 0 obj << /Broken true >> endobj\n".repeat(170_000));
        }
        "image-bomb" | "tiny-images" | "full-image" | "bad-jbig2" | "bad-jpx" => {
            extra.push(image(name));
            resources = "<< /XObject << /Im 7 0 R >> >>".into();
            content = if name == "tiny-images" {
                // Stay below operations::text_and_stats' 20,000 image-block cap:
                // this case exercises rendering, rather than a declared size refusal.
                b"q 1 0 0 1 0 0 cm /Im Do Q\n".repeat(19_999)
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
    if name == "object-stream-cycle" {
        return cyclic_xref(&objects);
    }
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

fn cyclic_xref(objects: &[Vec<u8>]) -> Vec<u8> {
    let mut out = b"%PDF-1.7\n".to_vec();
    let mut entries = vec![(0_u8, 0_u32, 65535_u16)];
    for (n, object) in objects.iter().enumerate() {
        entries.push((1, u32::try_from(out.len()).unwrap(), 0));
        writeln!(out, "{} 0 obj", n + 1).unwrap();
        out.extend_from_slice(object);
        out.extend_from_slice(b"\nendobj\n");
    }
    assert_eq!(objects.len(), 8);
    entries.extend([(2, 8, 0), (2, 7, 0)]); // 9 in B, 10 in A: neither has a physical fallback
    let xref = out.len();
    entries.push((1, u32::try_from(xref).unwrap(), 0));
    let mut data = Vec::new();
    for (kind, offset, generation) in entries {
        data.push(kind);
        data.extend_from_slice(&offset.to_be_bytes());
        data.extend_from_slice(&generation.to_be_bytes());
    }
    out.extend_from_slice(b"11 0 obj\n");
    out.extend(stream("/Type /XRef /Root 1 0 R /Size 12 /W [1 4 2]", &data));
    write!(out, "\nendobj\nstartxref\n{xref}\n%%EOF\n").unwrap();
    out
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

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Expected {
    Inspect(Error),
    RenderOk,
    RenderTimeout,
}

fn expected(name: &str) -> Expected {
    match name {
        "password" => Expected::Inspect(Error::PasswordProtected),
        "not-pdf" | "truncated" | "deep-arrays" | "deep-dictionaries" | "page-cycle" => {
            Expected::Inspect(Error::Unreadable)
        }
        "huge-page-count" => Expected::Inspect(Error::TooManyPages),
        "huge-mediabox" | "extreme-userunit" => Expected::Inspect(Error::TooLarge),
        "flate-gib" | "operators-50m" | "shading-pattern" => Expected::RenderTimeout,
        "nested-filters"
        | "image-bomb"
        | "tiny-images"
        | "prev-loop"
        | "object-stream-cycle"
        | "xref-repair"
        | "huge-object-count"
        | "type3-recursion"
        | "bad-jbig2"
        | "bad-jpx"
        | "bad-font"
        | "inert-actions"
        | "text-300"
        | "full-image"
        | "many-fonts"
        | "rotate-90"
        | "rotate-180"
        | "rotate-270" => Expected::RenderOk,
        _ => panic!("case lacks an expected outcome: {name}"),
    }
}

fn run_case(name: &str, valid: bool) {
    let expected = expected(name);
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("corpus.pdf");
    std::fs::write(&path, generate(name)).unwrap();
    let canary = Path::new("/tmp/mimik-must-not-exist");
    if name == "inert-actions" {
        assert!(!canary.exists(), "host canary must start absent");
    }
    let mut opts = production_options();
    bound_wall(&mut opts, valid, 1);
    let start = Instant::now();
    let report = inspection(&path, &opts, &Cancel::default());
    println!(
        "corpus {name}: inspect {}",
        report
            .as_ref()
            .map_or_else(ToString::to_string, |_| "Ok".into())
    );
    check_rss(name, "inspect", &opts);
    assert!(
        start.elapsed() < opts.wall.unwrap() + Duration::from_secs(5),
        "{name}: inspect deadline"
    );
    if let Expected::Inspect(error) = expected {
        assert_eq!(
            report.unwrap_err(),
            Reason::Document(error),
            "{name}: inspect class"
        );
        println!("corpus {name}: render not reached (declared inspect refusal)");
        return;
    }
    let report =
        report.unwrap_or_else(|e| panic!("{name}: must reach render, inspect returned {e}"));
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
        println!("corpus {name}: render {result:?}");
        check_rss(name, "render", &opts);
        let expected_result = match expected {
            Expected::RenderOk => Ok(()),
            Expected::RenderTimeout => Err(Error::Timeout.into()),
            Expected::Inspect(_) => unreachable!(),
        };
        assert_eq!(result, expected_result, "{name}: render class");
    }
    words_case(name, valid, expected, &path, &pages, &mut opts);
    if name == "inert-actions" {
        assert!(!canary.exists(), "inert PDF actions created a host canary");
    }
    // No staging output is permitted; execute returns only after all pinned processes die.
    assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 1);
    dir.close().unwrap();
}

/// Words for the first batch: bounded, validated, within the deadline, like the render.
fn words_case(
    name: &str,
    valid: bool,
    expected: Expected,
    path: &Path,
    pages: &[Page],
    opts: &mut Options,
) {
    let batch = &pages[..pages.len().min(8)];
    bound_wall(opts, valid, batch.len());
    let spec = Spec {
        operation: records::Operation::Words,
        pages: batch.iter().map(|p| p.n).collect(),
        settings: RenderSettings::default(),
        input: "/in/document.pdf".into(),
    };
    let start = Instant::now();
    let result = execute(path, &spec, &Cancel::default(), opts, |pipe| {
        let mut reader = records::WordsReader::new(pipe, batch.to_vec()).unwrap();
        let result = (|| {
            while reader.next_page()?.is_some() {}
            Ok(())
        })();
        (result, reader.terminal_error())
    });
    assert!(
        start.elapsed() < opts.wall.unwrap() + Duration::from_secs(5),
        "{name}: words deadline"
    );
    println!("corpus {name}: words {result:?}");
    check_rss(name, "words", opts);
    match expected {
        Expected::RenderOk => assert_eq!(result, Ok(()), "{name}: words class"),
        // Extracting text may finish where painting does not; it may never hang or crash.
        Expected::RenderTimeout => assert!(
            matches!(result, Ok(()) | Err(Reason::Document(Error::Timeout))),
            "{name}: words class {result:?}"
        ),
        Expected::Inspect(_) => unreachable!(),
    }
}

fn check_rss(name: &str, operation: &str, opts: &Options) {
    use std::sync::atomic::Ordering;
    let rss = opts.peak_rss_kib.load(std::sync::atomic::Ordering::Acquire);
    let worker = opts.worker_peak_rss_kib.load(Ordering::Acquire);
    let killed = opts.parent_killed.load(Ordering::Acquire);
    let exit = opts.observed_exit.load(Ordering::Acquire);
    println!(
        "corpus {name} {operation}: exit={exit}, parent_killed={killed}, wait4 RSS={rss} KiB (worker upper bound only for natural exits), pinned worker VmHWM={worker} KiB (sampled before parent kill; RLIMIT_AS=2097152 KiB; Vec and MuPDF buffer both present)"
    );
    assert!(
        rss_within_bound(opts),
        "{name}: RSS must be below 75% of RLIMIT_AS"
    );
    if killed {
        return;
    }
    assert!(
        (0..128).contains(&exit),
        "{name} {operation}: unexpected signal death/crash {exit}"
    );
}

#[test]
#[should_panic(expected = "RSS must be below 75% of RLIMIT_AS")]
fn parent_killed_run_cannot_skip_rss_gate() {
    let opts = Options::production().unwrap();
    opts.parent_killed
        .store(true, std::sync::atomic::Ordering::Release);
    check_rss("missing-killed-worker-measurement", "render", &opts);
}

#[test]
#[ignore = "host confinement: MIMIK_LIVE_RUNNER opt-in"]
fn live_peak_rss_positive_control() {
    if !enabled() {
        return;
    }
    let (_dir, path) = pdf();
    let opts = options("rss-control");
    inspection(&path, &opts, &Cancel::default()).unwrap();
    check_rss("256-MiB-positive-control", "inspect", &opts);
    assert!(
        !opts
            .parent_killed
            .load(std::sync::atomic::Ordering::Acquire)
    );
    assert!(opts.peak_rss_kib.load(std::sync::atomic::Ordering::Acquire) >= 256 * 1024);
}

#[test]
#[ignore = "host confinement: MIMIK_LIVE_RUNNER opt-in"]
fn live_parent_killed_peak_rss_positive_control() {
    if !enabled() {
        return;
    }
    let (_dir, path) = pdf();
    let mut opts = options("rss-control-killed");
    opts.wall = Some(Duration::from_secs(2));
    assert_eq!(
        inspection(&path, &opts, &Cancel::default()).unwrap_err(),
        Error::Timeout.into()
    );
    check_rss("256-MiB-killed-positive-control", "inspect", &opts);
    assert!(
        opts.parent_killed
            .load(std::sync::atomic::Ordering::Acquire)
    );
    assert!(
        opts.worker_peak_rss_kib
            .load(std::sync::atomic::Ordering::Acquire)
            >= 256 * 1024
    );
}

/// One page of known text: `Item Sword` at (50, 300) and `Cost` at (200, 300), PDF points from
/// the bottom left of a 300 x 400 `MediaBox`.
fn words_pdf(extra: &str) -> Vec<u8> {
    let content = b"BT /F1 12 Tf 50 300 Td (Item Sword) Tj 150 0 Td (Cost) Tj ET";
    raw(
        &[
            b"<< /Type /Catalog /Pages 2 0 R >>".to_vec(),
            b"<< /Type /Pages /Count 1 /Kids [3 0 R] >>".to_vec(),
            format!("<< /Type /Page /Parent 2 0 R /MediaBox [0 0 300 400] {extra} /Resources 5 0 R /Contents 4 0 R >>").into_bytes(),
            stream("", content),
            b"<< /Font << /F1 6 0 R >> >>".to_vec(),
            b"<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica >>".to_vec(),
        ],
        "",
        false,
    )
}

fn run_words(extra: &str) -> (Page, Vec<records::Word>) {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("words.pdf");
    std::fs::write(&path, words_pdf(extra)).unwrap();
    let opts = production_options();
    let report = inspection(&path, &opts, &Cancel::default()).unwrap();
    let page = report.pages[0].clone();
    let spec = Spec {
        operation: records::Operation::Words,
        pages: vec![1],
        settings: RenderSettings::default(),
        input: "/in/document.pdf".into(),
    };
    let words = execute(&path, &spec, &Cancel::default(), &opts, |pipe| {
        let mut reader = records::WordsReader::new(pipe, vec![page.clone()]).unwrap();
        let result = (|| {
            let words = reader.next_page()?.unwrap().words;
            assert!(reader.next_page()?.is_none());
            Ok(words)
        })();
        (result, reader.terminal_error())
    })
    .unwrap();
    (page, words)
}

#[test]
#[ignore = "host confinement: MIMIK_LIVE_RUNNER opt-in"]
fn live_words_land_inside_their_unit_polygons() {
    if !enabled() {
        return;
    }
    let (page, words) = run_words("");
    assert_eq!((page.width, page.height), (300.0, 400.0));
    let texts: Vec<&str> = words.iter().map(|w| w.text.as_str()).collect();
    assert_eq!(texts, ["Item", "Sword", "Cost"]);
    // A unit polygon an extractor would draw round each text, in the same page space.
    let inside = |word: &records::Word, polygon: [f64; 4]| {
        let (x, y) = (
            f64::midpoint(word.bounds[0], word.bounds[2]),
            f64::midpoint(word.bounds[1], word.bounds[3]),
        );
        polygon[0] <= x && x <= polygon[2] && polygon[1] <= y && y <= polygon[3]
    };
    assert!(inside(&words[0], [40.0, 80.0, 100.0, 110.0]));
    assert!(inside(&words[1], [70.0, 80.0, 130.0, 110.0]));
    assert!(inside(&words[2], [190.0, 80.0, 250.0, 110.0]));
    assert!(words[0].bounds[2] < words[1].bounds[0]);
}

#[test]
#[ignore = "host confinement: MIMIK_LIVE_RUNNER opt-in"]
fn live_words_of_a_rotated_page_stay_inside_the_corrected_page() {
    if !enabled() {
        return;
    }
    let (page, words) = run_words("/Rotate 90");
    assert_eq!((page.width, page.height), (400.0, 300.0));
    assert_eq!(words.len(), 3);
    for word in &words {
        assert!(word.bounds[2] <= page.width && word.bounds[3] <= page.height);
    }
}

#[test]
#[ignore = "host confinement: MIMIK_LIVE_RUNNER opt-in"]
fn live_words_of_an_offset_crop_box_are_relative_to_the_visible_page() {
    if !enabled() {
        return;
    }
    // Visible area x 10..290, y 20..380 of the 300 x 400 MediaBox: the corrected page is
    // 280 x 360 and `Item` (50, 300 from the bottom) sits at x 40, 80 from the top.
    let (page, words) = run_words("/CropBox [10 20 290 380]");
    assert_eq!((page.width, page.height), (280.0, 360.0));
    let item = words.iter().find(|w| w.text == "Item").unwrap();
    assert!((item.bounds[0] - 40.0).abs() < 1.0, "{:?}", item.bounds);
    assert!(
        item.bounds[1] > 62.0 && item.bounds[3] < 90.0,
        "{:?}",
        item.bounds
    );
    for word in &words {
        assert!(word.bounds[2] <= page.width && word.bounds[3] <= page.height);
    }
}

#[test]
#[ignore = "host confinement: MIMIK_LIVE_RUNNER opt-in"]
fn live_words_of_a_user_unit_page_are_scaled() {
    if !enabled() {
        return;
    }
    // UserUnit 2 doubles the page in points: 600 x 800, and `Item` moves to x 100.
    let (page, words) = run_words("/UserUnit 2");
    assert_eq!((page.width, page.height), (600.0, 800.0));
    let item = words.iter().find(|w| w.text == "Item").unwrap();
    assert!((item.bounds[0] - 100.0).abs() < 2.0, "{:?}", item.bounds);
    for word in &words {
        assert!(word.bounds[2] <= page.width && word.bounds[3] <= page.height);
    }
}

#[test]
fn cyclic_object_streams_are_required_by_page_resources() {
    let bytes = generated("object-stream-cycle");
    for required in [
        b"/Type /XRef".as_slice(),
        b"/Resources 9 0 R",
        b"/Length 10 0 R",
        b"/Length 9 0 R",
        &[2, 0, 0, 0, 8, 0, 0],
        &[2, 0, 0, 0, 7, 0, 0],
    ] {
        assert!(
            bytes.windows(required.len()).any(|part| part == required),
            "missing required cycle/xref edge: {required:?}"
        );
    }
}

#[test]
fn huge_object_count_has_one_size_key() {
    let bytes = generated("huge-object-count");
    assert_eq!(bytes.windows(5).filter(|part| *part == b"/Size").count(), 1);
}

#[test]
fn repeated_flate_has_valid_nonzero_payload_and_checksum() {
    let encoded = repeated_flate(b"q Q\n", 7);
    let mut decoded = Vec::new();
    flate2::read::ZlibDecoder::new(encoded.as_slice())
        .read_to_end(&mut decoded)
        .unwrap();
    assert_eq!(decoded, b"q Q\n".repeat(7));
}

#[test]
fn corpus_generators_are_bounded() {
    for name in HOSTILE.iter().chain(VALID) {
        // Fresh exec per case, measuring its own VmHWM. wait4 also includes pre-exec
        // fork RSS inherited from this parallel test process, so cannot enforce this cap.
        let output = std::process::Command::new(std::env::current_exe().unwrap())
            .args([
                "sources::worker::live_tests::corpus::generator_helper",
                "--exact",
                "--test-threads=1",
            ])
            .env("MIMIK_GENERATE_CASE", name)
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{name}: generator child failed: {} {}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
    }
    let compressed = zero_bomb();
    let mut decoder = flate2::read::ZlibDecoder::new(compressed.as_slice());
    assert_eq!(
        std::io::copy(&mut decoder, &mut std::io::sink()).unwrap(),
        1 << 30
    );
}

#[test]
fn generator_helper() {
    if let Ok(name) = std::env::var("MIMIK_GENERATE_CASE") {
        drop(generate(&name));
        let status = std::fs::read_to_string("/proc/self/status").unwrap();
        let rss = status
            .lines()
            .find_map(|line| {
                line.strip_prefix("VmHWM:")
                    .and_then(|value| value.split_whitespace().next()?.parse::<u64>().ok())
            })
            .unwrap();
        assert!(
            rss <= 64 * 1024,
            "{name}: generator peak {rss} KiB exceeds 64 MiB"
        );
    }
}

#[test]
#[ignore = "host confinement: MIMIK_LIVE_RUNNER opt-in"]
fn live_hostile_corpus() {
    if !enabled() {
        return;
    }
    let started = Instant::now();
    let mut failed = Vec::new();
    for name in HOSTILE {
        if std::panic::catch_unwind(|| run_case(name, false)).is_err() {
            failed.push(*name);
        }
    }
    assert!(
        failed.is_empty(),
        "hostile cases failed their declared contracts: {failed:?}"
    );
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
    // The real hostile render either consumes the deadline itself or waits after its
    // first image in the test-only mid-render hook. Either way the deadline is mandatory.
    let mut opts = options("mid-render");
    opts.wall = Some(Duration::from_secs(8));
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
            let mut reader =
                records::RenderReader::new(super::reader(pipe, true).unwrap(), report.pages)
                    .unwrap();
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
    assert_eq!(handle.join().unwrap(), Err(Error::Timeout.into()));
}
