//! Route-A 4a0 preflight measurements (synthetic PDFs only): vendored `mupdf` text API fit,
//! clipping behaviour with the `CLIP` extraction flag off and on, and serialized-size encodings.
//! Every fixture is generated here; no real manual is read.

use lopdf::{Document, Object, Stream, dictionary};
use mupdf::text_page::TextBlockType;
use mupdf::{TextCharFlags, TextPageFlags};
use std::fmt::Write as _;

/// One glyph as the safe crate exposes it, plus the owning line index.
#[derive(Clone, Debug)]
struct Glyph {
    ch: Option<char>,
    quad: [f32; 8],
    origin: [f32; 2],
    size: f32,
    argb: u32,
    flags: TextCharFlags,
    font: Option<String>,
    bidi: u16,
    is_rtl: bool,
    line: usize,
}

#[derive(Clone, Debug)]
struct Line {
    wmode_vertical: bool,
    dir: [f32; 2],
    bounds: [f32; 4],
    block_path: Vec<String>,
}

#[derive(Default, Debug)]
struct Dump {
    glyphs: Vec<Glyph>,
    lines: Vec<Line>,
    /// Block kinds seen at the top level of `TextPage::blocks()`.
    top_kinds: Vec<TextBlockType>,
    /// Struct blocks with their standard type and raw role, found anywhere.
    structs: Vec<(String, Option<String>, usize)>,
}

fn walk(blocks: mupdf::text_page::TextBlockIter<'_>, path: &mut Vec<String>, dump: &mut Dump) {
    for block in blocks {
        match block.r#type() {
            TextBlockType::Struct => {
                let node = block.structure().expect("struct block without node");
                let role = node.raw().map(str::to_owned);
                dump.structs
                    .push((node.standard().name().to_owned(), role, path.len()));
                path.push(node.standard().name().to_owned());
                walk(node.blocks(), path, dump);
                path.pop();
            }
            TextBlockType::Text => {
                for line in block.lines() {
                    let index = dump.lines.len();
                    let b = line.bounds();
                    let d = line.dir();
                    dump.lines.push(Line {
                        wmode_vertical: line.wmode() == mupdf::WriteMode::Vertical,
                        dir: [d.x, d.y],
                        bounds: [b.x0, b.y0, b.x1, b.y1],
                        block_path: path.clone(),
                    });
                    for c in line.chars() {
                        let q = c.quad();
                        let o = c.origin();
                        dump.glyphs.push(Glyph {
                            ch: c.char(),
                            quad: [
                                q.ul.x, q.ul.y, q.ur.x, q.ur.y, q.ll.x, q.ll.y, q.lr.x, q.lr.y,
                            ],
                            origin: [o.x, o.y],
                            size: c.size(),
                            argb: c.argb(),
                            flags: c.flags(),
                            font: c.font().map(|f| f.name().to_owned()),
                            bidi: c.bidi(),
                            is_rtl: c.is_rtl(),
                            line: index,
                        });
                    }
                }
            }
            _ => {}
        }
    }
}

fn dump_page(page: &mupdf::pdf::PdfPage, flags: TextPageFlags) -> Dump {
    let text = page.to_text_page(flags).unwrap();
    let mut dump = Dump {
        top_kinds: text.blocks().map(|b| b.r#type()).collect(),
        ..Dump::default()
    };
    walk(text.blocks(), &mut Vec::new(), &mut dump);
    dump
}

fn dump_bytes(bytes: &[u8], page: i32, flags: TextPageFlags) -> Dump {
    let doc = mupdf::pdf::PdfDocument::from_bytes(bytes).unwrap();
    dump_page(&doc.load_pdf_page(page).unwrap(), flags)
}

struct Spec<'a> {
    content: &'a str,
    media: [i32; 4],
    crop: Option<[i32; 4]>,
}

/// Build a Helvetica PDF with one page per spec.
fn build(pages: &[Spec<'_>], tagged: bool) -> Vec<u8> {
    assert!(
        !tagged || pages.len() == 1,
        "the ParentTree is valid for one page only"
    );
    let mut doc = Document::with_version("1.7");
    let pages_id = doc.new_object_id();
    let font =
        doc.add_object(dictionary! {"Type"=>"Font","Subtype"=>"Type1","BaseFont"=>"Helvetica"});
    let resources = doc.add_object(dictionary! {"Font"=>dictionary!{"F1"=>font}});
    let struct_root = doc.new_object_id();
    let doc_elem = doc.new_object_id();
    let mut kids = Vec::new();
    let mut elems: Vec<Object> = Vec::new();
    for (n, spec) in pages.iter().enumerate() {
        let content = doc.add_object(Stream::new(
            dictionary! {},
            spec.content.as_bytes().to_vec(),
        ));
        let mut page = dictionary! {"Type"=>"Page","Parent"=>pages_id,"Contents"=>content,
        "Resources"=>resources,
        "MediaBox"=>spec.media.iter().copied().map(Object::from).collect::<Vec<_>>()};
        if let Some(crop) = spec.crop {
            page.set(
                "CropBox",
                crop.into_iter().map(Object::from).collect::<Vec<_>>(),
            );
        }
        let leaf = doc.new_object_id();
        if tagged {
            let h1 = doc.add_object(
                dictionary! {"Type"=>"StructElem","S"=>"H1","P"=>doc_elem,"Pg"=>leaf,"K"=>0},
            );
            let p = doc.add_object(
                dictionary! {"Type"=>"StructElem","S"=>"P","P"=>doc_elem,"Pg"=>leaf,"K"=>1},
            );
            elems.push(h1.into());
            elems.push(p.into());
            page.set("StructParents", i64::try_from(n).unwrap());
        }
        doc.objects.insert(leaf, Object::Dictionary(page));
        kids.push(Object::from(leaf));
    }
    let count = i64::try_from(kids.len()).unwrap();
    doc.objects.insert(
        pages_id,
        Object::Dictionary(dictionary! {"Type"=>"Pages","Count"=>count,"Kids"=>kids}),
    );
    let mut catalog = dictionary! {"Type"=>"Catalog","Pages"=>pages_id};
    if tagged {
        doc.objects.insert(
            doc_elem,
            Object::Dictionary(
                dictionary! {"Type"=>"StructElem","S"=>"Document","P"=>struct_root,"K"=>elems.clone()},
            ),
        );
        doc.objects.insert(
            struct_root,
            Object::Dictionary(
                dictionary! {"Type"=>"StructTreeRoot","K"=>vec![doc_elem.into()],
                "ParentTree"=>dictionary!{"Nums"=>vec![0.into(), elems.into()]}},
            ),
        );
        catalog.set("StructTreeRoot", struct_root);
        catalog.set("MarkInfo", dictionary! {"Marked"=>true});
    }
    let root = doc.add_object(catalog);
    doc.trailer.set("Root", root);
    let mut bytes = Vec::new();
    doc.save_to(&mut bytes).unwrap();
    bytes
}

const A4: [i32; 4] = [0, 0, 595, 842];

fn one(content: &str, media: [i32; 4], crop: Option<[i32; 4]>) -> Vec<u8> {
    build(
        &[Spec {
            content,
            media,
            crop,
        }],
        false,
    )
}

const BASE: TextPageFlags = TextPageFlags::PRESERVE_LIGATURES
    .union(TextPageFlags::PRESERVE_WHITESPACE)
    .union(TextPageFlags::PRESERVE_IMAGES)
    .union(TextPageFlags::USE_CID_FOR_UNKNOWN_UNICODE);

fn text_of(d: &Dump) -> String {
    d.glyphs.iter().filter_map(|g| g.ch).collect()
}

// ---------------------------------------------------------------------------------------------
// Question 1: API fit.
// ---------------------------------------------------------------------------------------------

/// Content: a red 12 pt Helvetica line, and a line rotated by 90 degrees (Tm), no tags.
const API_CONTENT: &str = "BT /F1 12 Tf 1 0 0 rg 50 700 Td (Hello ABC) Tj ET \
    BT /F1 12 Tf 0 0 1 rg 0 1 -1 0 300 400 Tm (Up) Tj ET";

#[test]
fn api_fit_exposes_geometry_style_font_and_direction() {
    let bytes = one(API_CONTENT, A4, None);
    let d = dump_bytes(&bytes, 0, BASE);
    eprintln!("4a0 api: top kinds {:?}", d.top_kinds);
    eprintln!("4a0 api: lines {:#?}", d.lines);
    eprintln!("4a0 api: first glyphs {:#?}", &d.glyphs[..3]);
    assert!(text_of(&d).starts_with("Hello ABC"));
    assert!(text_of(&d).contains("Up"));
    let h = &d.glyphs[0];
    assert!((h.size - 12.0).abs() < 1e-3);
    assert_eq!(h.argb, 0xffff_0000);
    assert_eq!(h.font.as_deref(), Some("Helvetica"));
    assert!(h.flags.contains(TextCharFlags::FILLED));
    assert!(!h.flags.contains(TextCharFlags::CLIPPED));
    assert!(!h.is_rtl);
    assert_eq!(h.bidi, 0);
    // Origin lies on the baseline: y = 842 - 700 in MuPDF page space.
    assert!((h.origin[0] - 50.0).abs() < 1e-3 && (h.origin[1] - 142.0).abs() < 1e-3);
    // Quad: four distinct corners of positive area.
    let q = h.quad;
    assert!(q[2] > q[0] && q[5] > q[1]);
    // Horizontal line has dir (1, 0); the rotated line differs.
    assert!(d.lines.iter().all(|l| !l.wmode_vertical));
    assert!((d.lines[0].dir[0] - 1.0).abs() < 1e-3 && d.lines[0].dir[1].abs() < 1e-3);
    let rotated = d.lines.last().unwrap();
    assert!(rotated.dir[0].abs() < 1e-3 && (rotated.dir[1] + 1.0).abs() < 1e-3);
    // The space is a real glyph with PRESERVE_WHITESPACE.
    assert!(d.glyphs.iter().any(|g| g.ch == Some(' ')));
}

#[test]
fn api_fit_structure_traversal_needs_collect_structure() {
    let content = "/H1 <</MCID 0>> BDC BT /F1 18 Tf 50 780 Td (Title) Tj ET EMC \
        /P <</MCID 1>> BDC BT /F1 12 Tf 50 700 Td (Body text) Tj ET EMC";
    let bytes = build(
        &[Spec {
            content,
            media: A4,
            crop: None,
        }],
        true,
    );
    let plain = dump_bytes(&bytes, 0, BASE);
    let tagged = dump_bytes(&bytes, 0, BASE | TextPageFlags::COLLECT_STRUCTURE);
    eprintln!(
        "4a0 struct: without flag top={:?} structs={:?} text={:?}",
        plain.top_kinds,
        plain.structs,
        text_of(&plain)
    );
    eprintln!(
        "4a0 struct: with COLLECT_STRUCTURE top={:?} structs={:?} text={:?} line paths={:?}",
        tagged.top_kinds,
        tagged.structs,
        text_of(&tagged),
        tagged
            .lines
            .iter()
            .map(|l| l.block_path.clone())
            .collect::<Vec<_>>()
    );
    // Without the flag no structure is collected.
    assert_eq!(plain.structs.len(), 0);
    assert_eq!(text_of(&plain), "TitleBody text");
    // With the flag: top-level blocks() yields only Struct blocks; text is reachable only
    // through structure().blocks(), and every glyph is still seen exactly once.
    assert!(tagged.top_kinds.iter().all(|k| *k == TextBlockType::Struct));
    assert_ne!(tagged.structs.len(), 0);
    assert_eq!(text_of(&tagged), "TitleBody text");
    assert!(tagged.structs.iter().any(|(s, _, _)| s == "H1"));
    assert!(tagged.structs.iter().any(|(s, _, _)| s == "P"));
    assert_eq!(tagged.lines.len(), 2);
    assert_eq!(tagged.lines[0].block_path, ["Document", "H1"]);
    assert_eq!(tagged.lines[1].block_path, ["Document", "P"]);
    // The flat top-level iteration alone sees no text lines.
    let doc = mupdf::pdf::PdfDocument::from_bytes(&bytes).unwrap();
    let page = doc.load_pdf_page(0).unwrap();
    let text = page
        .to_text_page(BASE | TextPageFlags::COLLECT_STRUCTURE)
        .unwrap();
    let top_lines: usize = text.blocks().map(|b| b.lines().count()).sum();
    assert_eq!(top_lines, 0, "top-level blocks() hides nested text");
    // The ruled-out wrapper maps Struct blocks to Other and so drops the text entirely.
    let structured_chars: usize = text
        .structured()
        .blocks
        .iter()
        .map(|b| match &b.content {
            mupdf::text_page::TextBlockContent::Text { lines } => {
                lines.iter().map(|l| l.chars.len()).sum()
            }
            _ => 0,
        })
        .sum();
    eprintln!("4a0 struct: structured() chars with COLLECT_STRUCTURE = {structured_chars}");
    assert_eq!(structured_chars, 0);
}

// ---------------------------------------------------------------------------------------------
// Question 2: clipping.
// ---------------------------------------------------------------------------------------------

fn summarize(label: &str, d: &Dump) -> String {
    let mut s = format!("{label}: {} glyphs", d.glyphs.len());
    let _ = write!(s, " text={:?}", text_of(d));
    let mut clipped = 0;
    let mut filled = 0;
    let mut stroked = 0;
    for g in &d.glyphs {
        clipped += usize::from(g.flags.contains(TextCharFlags::CLIPPED));
        filled += usize::from(g.flags.contains(TextCharFlags::FILLED));
        stroked += usize::from(g.flags.contains(TextCharFlags::STROKED));
    }
    let _ = write!(s, " CLIPPED={clipped} FILLED={filled} STROKED={stroked}");
    s
}

/// Case (a1): scissored by an in-page clip. Clip rectangle x 0..30, y 0..400 (PDF space) over
/// 20 pt Helvetica: "AB" at x=10 (A is inside the clip, B straddles x=30), "CD" at x=100 (wholly
/// outside), "EF" at x=2 (both inside), then the clip is released and "UNCLIPPED" is drawn.
const SCISSOR: &str = "q 0 0 30 400 re W n BT /F1 20 Tf 10 300 Td (AB) Tj ET \
    BT /F1 20 Tf 100 250 Td (CD) Tj ET BT /F1 20 Tf 2 200 Td (EF) Tj ET Q \
    BT /F1 20 Tf 10 100 Td (UNCLIPPED) Tj ET";

#[test]
fn clip_flag_drops_in_page_scissored_glyphs() {
    let bytes = one(SCISSOR, [0, 0, 300, 400], None);
    let off = dump_bytes(&bytes, 0, BASE);
    let on = dump_bytes(&bytes, 0, BASE | TextPageFlags::CLIP);
    eprintln!("4a0 clip-a1 {}", summarize("CLIP off", &off));
    eprintln!("4a0 clip-a1 {}", summarize("CLIP on ", &on));
    assert_eq!(text_of(&off), "ABCDEFUNCLIPPED");
    assert_eq!(text_of(&on), "ABEFUNCLIPPED");
    // The scissored-away glyphs carry no CLIPPED flag: MuPDF cannot tell us about them.
    assert!(
        off.glyphs
            .iter()
            .all(|g| !g.flags.contains(TextCharFlags::CLIPPED))
    );
    assert!(on.glyphs.len() < off.glyphs.len());
}

/// Case (a2): glyphs outside the displayed page. `MediaBox` 0 0 200 300, text at x=190 (E partly
/// out, F out), x=-30 (partly/wholly left of the page), y=310 (above) and one inside.
const OFF_PAGE: &str = "BT /F1 20 Tf 190 150 Td (EF) Tj ET BT /F1 20 Tf -22 100 Td (GH) Tj ET \
    BT /F1 20 Tf 10 310 Td (UP) Tj ET BT /F1 20 Tf 10 50 Td (IN) Tj ET";

#[test]
fn clip_flag_and_glyphs_outside_the_page_box() {
    for (label, crop) in [
        ("MediaBox only", None),
        ("CropBox 20..100", Some([20, 20, 100, 280])),
    ] {
        let bytes = one(OFF_PAGE, [0, 0, 200, 300], crop);
        let off = dump_bytes(&bytes, 0, BASE);
        let on = dump_bytes(&bytes, 0, BASE | TextPageFlags::CLIP);
        eprintln!("4a0 clip-a2 [{label}] {}", summarize("CLIP off", &off));
        eprintln!("4a0 clip-a2 [{label}] {}", summarize("CLIP on ", &on));
        for g in &off.glyphs {
            eprintln!(
                "4a0 clip-a2 [{label}] off {:?} x={:.1}..{:.1} y={:.1}..{:.1}",
                g.ch,
                g.quad[0].min(g.quad[4]),
                g.quad[2].max(g.quad[6]),
                g.quad[1].min(g.quad[3]),
                g.quad[5].max(g.quad[7])
            );
        }
        assert_eq!(text_of(&off), "EFGHUPIN");
        let expected_on = if crop.is_some() { "N" } else { "EHIN" };
        assert_eq!(text_of(&on), expected_on);
        assert!(text_of(&off).contains("IN"));
        assert!(
            off.glyphs
                .iter()
                .all(|g| !g.flags.contains(TextCharFlags::CLIPPED))
        );
    }
}

/// Case (b): text as clipping path, render modes 4..7. "AB" is drawn with mode `m` inside a
/// q..Q, then plain filled text "CD" is drawn at another place (outside the glyph outlines of
/// AB, so it is wholly clipped away on screen) before Q.
fn clip_mode_pdf(mode: u8) -> Vec<u8> {
    let content = format!(
        "q BT /F1 24 Tf {mode} Tr 10 100 Td (AB) Tj ET \
         BT /F1 12 Tf 0 Tr 10 200 Td (CD) Tj ET Q"
    );
    one(&content, [0, 0, 200, 300], None)
}

#[test]
fn text_render_modes_4_to_7_with_and_without_clip_flag() {
    for mode in [0u8, 3, 4, 5, 6, 7] {
        let bytes = clip_mode_pdf(mode);
        let off = dump_bytes(&bytes, 0, BASE);
        let on = dump_bytes(&bytes, 0, BASE | TextPageFlags::CLIP);
        eprintln!("4a0 clip-b mode {mode} {}", summarize("CLIP off", &off));
        eprintln!("4a0 clip-b mode {mode} {}", summarize("CLIP on ", &on));
        for g in off.glyphs.iter().take(2) {
            eprintln!(
                "4a0 clip-b mode {mode} off glyph {:?} flags {:?}",
                g.ch, g.flags
            );
        }
        // Raw extraction never loses glyphs: all four are present with CLIP off.
        assert_eq!(text_of(&off), "ABCD");
        let clipped = |d: &Dump| {
            d.glyphs
                .iter()
                .filter(|g| g.flags.contains(TextCharFlags::CLIPPED))
                .count()
        };
        // Only mode 7 sets CLIPPED, on the clip-mode glyphs "AB" (together with FILLED);
        // "CD" (drawn later under that clip) is not flagged.
        assert_eq!(clipped(&off), if mode == 7 { 2 } else { 0 }, "mode {mode}");
        assert_eq!(clipped(&on), clipped(&off), "mode {mode}");
        if mode == 3 {
            assert!(off.glyphs[..2].iter().all(|g| {
                !g.flags
                    .intersects(TextCharFlags::FILLED | TextCharFlags::STROKED)
            }));
        }
        if mode == 7 {
            assert_eq!(
                off.glyphs[0].flags,
                TextCharFlags::FILLED | TextCharFlags::CLIPPED
            );
            assert!(!off.glyphs[2].flags.contains(TextCharFlags::CLIPPED));
        }
        // Modes 4-7: the clip-path glyphs "AB" survive CLIP, the later "CD" (outside the
        // glyph outlines) is dropped by CLIP. Modes 0 and 3 install no text clip.
        let expected_on = if (4..=7).contains(&mode) {
            "AB"
        } else {
            "ABCD"
        };
        assert_eq!(text_of(&on), expected_on, "mode {mode}");
    }
}

// ---------------------------------------------------------------------------------------------
// Question 3: serialized size.
// ---------------------------------------------------------------------------------------------

/// Deterministic dense text: `lines` lines of ~100 characters of lowercase pseudo-words at 8 pt.
fn dense_content(seed: u32, lines: usize) -> String {
    let mut state = seed.wrapping_mul(2_654_435_761).wrapping_add(12345);
    let mut next = move || {
        state = state.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
        state >> 16
    };
    let mut content = String::from("BT /F1 8 Tf 9 TL 20 830 Td\n");
    for _ in 0..lines {
        let mut line = String::new();
        while line.len() < 100 {
            let length = 2 + next() % 8;
            for _ in 0..length {
                let letter = u8::try_from(next() % 26).unwrap();
                line.push(char::from(b'a' + letter));
            }
            line.push(' ');
        }
        line.truncate(100);
        let _ = writeln!(content, "({line}) '");
    }
    content.push_str("ET");
    content
}

const SCALE: f64 = 1024.0;

#[derive(Debug, PartialEq, Eq)]
enum EncodeError {
    NonFinite,
    Overflow,
}

/// Round a displayed-pt coordinate to integer 1/1024 pt, refusing non-finite and out-of-range
/// values. The rounded value is integral, so its decimal rendering parses exactly into `i32`
/// when (and only when) it is in range; this avoids a lossy `as` cast.
fn to_fixed(v: f32) -> Result<i32, EncodeError> {
    let scaled = (f64::from(v) * SCALE).round();
    if !scaled.is_finite() {
        return Err(EncodeError::NonFinite);
    }
    format!("{scaled}")
        .parse::<i32>()
        .map_err(|_| EncodeError::Overflow)
}

fn from_fixed(v: i32) -> f64 {
    f64::from(v) / SCALE
}

/// Verbose per-glyph JSON records.
fn verbose_json(glyphs: &[Glyph]) -> String {
    let records: Vec<_> = glyphs
        .iter()
        .map(|g| {
            serde_json::json!({
                "char": g.ch.map_or_else(|| "\u{fffd}".to_owned(), |c| c.to_string()),
                "quad": g.quad,
                "origin": g.origin,
                "size": g.size,
                "font": g.font,
                "flags": g.flags.bits(),
                "bidi": g.bidi,
                "argb": g.argb,
                "line": g.line,
            })
        })
        .collect();
    serde_json::to_string(&serde_json::json!({ "glyphs": records })).unwrap()
}

/// Columnar parallel arrays: shared font table, integer 1/1024 pt coordinates.
fn columnar_json(glyphs: &[Glyph], lines: &[Line]) -> Result<String, EncodeError> {
    let mut fonts: Vec<String> = Vec::new();
    let mut font_index = Vec::new();
    for g in glyphs {
        let name = g.font.clone().unwrap_or_default();
        let at = fonts.iter().position(|f| *f == name).unwrap_or_else(|| {
            fonts.push(name);
            fonts.len() - 1
        });
        font_index.push(at);
    }
    let mut quad = Vec::with_capacity(glyphs.len() * 8);
    let mut origin = Vec::with_capacity(glyphs.len() * 2);
    let mut size = Vec::with_capacity(glyphs.len());
    for g in glyphs {
        for v in g.quad {
            quad.push(to_fixed(v)?);
        }
        for v in g.origin {
            origin.push(to_fixed(v)?);
        }
        size.push(to_fixed(g.size)?);
    }
    let line_bounds = lines
        .iter()
        .flat_map(|l| l.bounds)
        .map(to_fixed)
        .collect::<Result<Vec<_>, _>>()?;
    Ok(serde_json::to_string(&serde_json::json!({
        "scale": 1024,
        "fonts": fonts,
        "lines": { "bounds": line_bounds },
        "glyphs": {
            "cp": glyphs.iter().map(|g| g.ch.map_or(0xFFFD, u32::from)).collect::<Vec<_>>(),
            "line": glyphs.iter().map(|g| g.line).collect::<Vec<_>>(),
            "font": font_index,
            "flags": glyphs.iter().map(|g| g.flags.bits()).collect::<Vec<_>>(),
            "bidi": glyphs.iter().map(|g| g.bidi).collect::<Vec<_>>(),
            "argb": glyphs.iter().map(|g| g.argb).collect::<Vec<_>>(),
            "size": size,
            "origin": origin,
            "quad": quad,
        },
    }))
    .unwrap())
}

fn gzip_len(text: &str) -> usize {
    use std::io::Write as _;
    let mut encoder = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
    encoder.write_all(text.as_bytes()).unwrap();
    encoder.finish().unwrap().len()
}

#[test]
fn fixed_point_encoding_bounds_and_overflow_refusal() {
    // Documentation only: round-to-nearest at 1/1024 pt bounds the error by 1/2048 pt by
    // construction (the overflow-refusal assertions below are the substantive ones).
    let mut worst: f64 = 0.0;
    for n in 0..200_000u32 {
        let v = f32::from(u16::try_from(n % 20_000).unwrap()) / 7.0 - 700.0
            + f32::from(u16::try_from(n % 997).unwrap()) * 1e-3;
        let back = from_fixed(to_fixed(v).unwrap());
        worst = worst.max((back - f64::from(v)).abs());
    }
    eprintln!(
        "4a0 fixed: max abs round-trip error {worst:.8} pt (bound 1/2048 = {:.8})",
        0.5 / SCALE
    );
    assert!(worst <= 0.5 / SCALE + 1e-9);
    // Largest coordinate representable in i32 at 1/1024 pt.
    let max_pt = f64::from(i32::MAX) / SCALE;
    eprintln!(
        "4a0 fixed: largest representable {max_pt:.6} pt, smallest {:.6} pt",
        f64::from(i32::MIN) / SCALE
    );
    assert_eq!(to_fixed(2_097_151.0), Ok(2_147_482_624));
    assert_eq!(to_fixed(2_097_152.0), Err(EncodeError::Overflow));
    assert_eq!(to_fixed(-2_097_152.0), Ok(i32::MIN));
    assert_eq!(to_fixed(-2_097_153.0), Err(EncodeError::Overflow));
    assert_eq!(to_fixed(f32::NAN), Err(EncodeError::NonFinite));
    assert_eq!(to_fixed(f32::INFINITY), Err(EncodeError::NonFinite));
    assert_eq!(to_fixed(f32::MAX), Err(EncodeError::Overflow));
    assert_eq!(to_fixed(-0.0), Ok(0));
}

#[test]
fn serialized_size_columnar_vs_verbose_on_dense_pages() {
    let one_page = build(
        &[Spec {
            content: &dense_content(1, 90),
            media: A4,
            crop: None,
        }],
        false,
    );
    let d = dump_bytes(&one_page, 0, BASE);
    let verbose = verbose_json(&d.glyphs);
    let columnar = columnar_json(&d.glyphs, &d.lines).unwrap();
    let n = d.glyphs.len();
    eprintln!(
        "4a0 size 1 page: {n} glyphs, {} lines; verbose {} B ({:.1} B/glyph, gzip {}), columnar {} B ({:.1} B/glyph, gzip {})",
        d.lines.len(),
        verbose.len(),
        per_glyph(verbose.len(), n),
        gzip_len(&verbose),
        columnar.len(),
        per_glyph(columnar.len(), n),
        gzip_len(&columnar),
    );
    assert!(n >= 6000, "{n}");
    assert!(columnar.len() < verbose.len());
    assert!(columnar.len() < 8 * 1024 * 1024);
    // Geometric round trip over every glyph coordinate of the real page.
    let mut worst: f64 = 0.0;
    for g in &d.glyphs {
        for v in g.quad.iter().chain(&g.origin).copied() {
            worst = worst.max((from_fixed(to_fixed(v).unwrap()) - f64::from(v)).abs());
        }
    }
    eprintln!("4a0 size: dense page max coordinate round-trip error {worst:.8} pt");
    assert!(worst <= 0.5 / SCALE + 1e-9);
}

/// 64 distinct dense pages (576,000 glyphs); about 80 s in a debug build, hence ignored.
#[test]
#[ignore = "4a0 measurement"]
fn serialized_size_columnar_vs_verbose_on_64_pages() {
    let contents: Vec<String> = (0..64).map(|s| dense_content(s + 10, 90)).collect();
    let specs: Vec<Spec<'_>> = contents
        .iter()
        .map(|c| Spec {
            content: c,
            media: A4,
            crop: None,
        })
        .collect();
    let doc_bytes = build(&specs, false);
    let doc = mupdf::pdf::PdfDocument::from_bytes(&doc_bytes).unwrap();
    let (mut verbose_total, mut columnar_total, mut glyph_total) = (0usize, 0usize, 0usize);
    let (mut verbose_gz, mut columnar_gz) = (0usize, 0usize);
    let (mut worst_page_verbose, mut worst_page_columnar) = (0usize, 0usize);
    for page in 0..64 {
        let d = dump_page(&doc.load_pdf_page(page).unwrap(), BASE);
        let v = verbose_json(&d.glyphs);
        let c = columnar_json(&d.glyphs, &d.lines).unwrap();
        glyph_total += d.glyphs.len();
        verbose_total += v.len();
        columnar_total += c.len();
        verbose_gz += gzip_len(&v);
        columnar_gz += gzip_len(&c);
        worst_page_verbose = worst_page_verbose.max(v.len());
        worst_page_columnar = worst_page_columnar.max(c.len());
    }
    eprintln!(
        "4a0 size 64 pages: {glyph_total} glyphs; verbose {verbose_total} B ({:.1} B/glyph, gzip {verbose_gz}, largest page {worst_page_verbose}); columnar {columnar_total} B ({:.1} B/glyph, gzip {columnar_gz}, largest page {worst_page_columnar})",
        per_glyph(verbose_total, glyph_total),
        per_glyph(columnar_total, glyph_total),
    );
    assert!(columnar_total < verbose_total);
    assert!(worst_page_columnar < 8 * 1024 * 1024);
}

fn per_glyph(bytes: usize, glyphs: usize) -> f64 {
    f64::from(u32::try_from(bytes).unwrap()) / f64::from(u32::try_from(glyphs).unwrap())
}

// ---------------------------------------------------------------------------------------------
// Additional 4a0 facts (review round).
// ---------------------------------------------------------------------------------------------

/// Pins the crate's raw block-type mapping: exactly the five kinds of the C library (0..=4)
/// convert; any other raw value is an error. `TextBlock::r#type()` unwraps this conversion and
/// the safe API offers no raw-type pre-check, so a panic from it must be treated as a contained
/// worker failure. A real page cannot produce an unknown value, so this test only documents the
/// mapping and does not exercise `r#type()`.
#[test]
fn block_type_conversion_covers_exactly_the_five_mupdf_kinds() {
    for known in 0..=4 {
        assert!(TextBlockType::try_from(known).is_ok(), "{known}");
    }
    for unknown in [5, 99, -1] {
        assert!(TextBlockType::try_from(unknown).is_err(), "{unknown}");
    }
}

fn top_kinds(bytes: &[u8], flags: TextPageFlags) -> Vec<TextBlockType> {
    dump_bytes(bytes, 0, BASE | flags).top_kinds
}

#[test]
fn segment_and_table_hunt_flags_separately_produce_struct_blocks() {
    let bytes = one(
        "0 0 m 100 100 l S BT /F1 12 Tf 50 700 Td (x) Tj ET",
        A4,
        None,
    );
    for (label, flag) in [
        ("none", TextPageFlags::empty()),
        ("SEGMENT", TextPageFlags::SEGMENT),
        ("TABLE_HUNT", TextPageFlags::TABLE_HUNT),
        ("COLLECT_VECTORS", TextPageFlags::COLLECT_VECTORS),
    ] {
        eprintln!("4a0 blocktype: {label} -> {:?}", top_kinds(&bytes, flag));
    }
    assert_eq!(
        top_kinds(&bytes, TextPageFlags::empty()),
        [TextBlockType::Text]
    );
    assert!(top_kinds(&bytes, TextPageFlags::SEGMENT).contains(&TextBlockType::Struct));
    for flag in [TextPageFlags::TABLE_HUNT, TextPageFlags::COLLECT_VECTORS] {
        assert!(!top_kinds(&bytes, flag).contains(&TextBlockType::Struct));
    }
}

/// Edit page 1 and its resources dictionary of a generated PDF.
fn edit_first_page(
    bytes: &[u8],
    edit: impl FnOnce(&mut Document, lopdf::ObjectId, lopdf::ObjectId),
) -> Vec<u8> {
    let mut doc = Document::load_mem(bytes).unwrap();
    let page = doc.get_pages()[&1];
    let resources = doc
        .get_dictionary(page)
        .unwrap()
        .get(b"Resources")
        .unwrap()
        .as_reference()
        .unwrap();
    edit(&mut doc, page, resources);
    let mut out = Vec::new();
    doc.save_to(&mut out).unwrap();
    out
}

/// A red-to-blue axial shading pattern named `P0` in the page resources.
fn with_shading_pattern(bytes: &[u8]) -> Vec<u8> {
    edit_first_page(bytes, |doc, _, resources| {
        let function = dictionary! {"FunctionType"=>2,"Domain"=>vec![0.into(),1.into()],
        "C0"=>vec![1.into(),0.into(),0.into()],"C1"=>vec![0.into(),0.into(),1.into()],"N"=>1};
        let shading = dictionary! {"ShadingType"=>2,"ColorSpace"=>"DeviceRGB",
        "Coords"=>vec![0.into(),0.into(),200.into(),0.into()],"Function"=>function};
        let pattern = doc.add_object(dictionary! {"PatternType"=>2,"Shading"=>shading});
        doc.get_dictionary_mut(resources)
            .unwrap()
            .set("Pattern", dictionary! {"P0"=>pattern});
    })
}

/// Visible text painted with a shading pattern: mode 0 (fill) "AB", mode 1 (stroke) "CD",
/// and plain black fill "EF" afterwards.
const PATTERN_TEXT: &str = "q /Pattern cs /P0 scn BT /F1 24 Tf 10 200 Td (AB) Tj ET Q \
    q /Pattern CS /P0 SCN 1 w BT /F1 24 Tf 1 Tr 10 150 Td (CD) Tj ET Q \
    0 g BT /F1 24 Tf 0 Tr 10 100 Td (EF) Tj ET";

#[test]
fn shading_pattern_text_flags_with_and_without_clip() {
    let bytes = with_shading_pattern(&one(PATTERN_TEXT, [0, 0, 200, 300], None));
    let off = dump_bytes(&bytes, 0, BASE);
    let on = dump_bytes(&bytes, 0, BASE | TextPageFlags::CLIP);
    eprintln!("4a0 pattern {}", summarize("CLIP off", &off));
    eprintln!("4a0 pattern {}", summarize("CLIP on ", &on));
    for g in &off.glyphs {
        eprintln!(
            "4a0 pattern off {:?} flags {:?} argb {:#010x}",
            g.ch, g.flags, g.argb
        );
    }
    for g in &on.glyphs {
        eprintln!(
            "4a0 pattern on  {:?} flags {:?} argb {:#010x}",
            g.ch, g.flags, g.argb
        );
    }
    assert_eq!(text_of(&off), "ABCDEF");
    // Pattern-painted text goes through clip-text: FILLED/STROKED plus CLIPPED, argb 0; CLIP
    // does not change the result (the glyphs lie inside the page).
    let key = |d: &Dump| {
        d.glyphs
            .iter()
            .map(|g| (g.flags.bits(), g.argb))
            .collect::<Vec<_>>()
    };
    assert_eq!(key(&on), key(&off));
    let by = |c: char| off.glyphs.iter().find(|g| g.ch == Some(c)).unwrap();
    assert_eq!(
        by('A').flags,
        TextCharFlags::FILLED | TextCharFlags::CLIPPED
    );
    assert_eq!(
        by('C').flags,
        TextCharFlags::STROKED | TextCharFlags::CLIPPED
    );
    assert_eq!((by('A').argb, by('C').argb), (0, 0));
    assert_eq!(
        (by('E').flags, by('E').argb),
        (TextCharFlags::FILLED, 0xff00_0000)
    );
}

#[test]
fn page_box_coordinates_rotation_and_user_unit() {
    // CropBox [20 20 100 280] on a 200x300 MediaBox: coordinates are relative to the crop origin.
    let bytes = one(OFF_PAGE, [0, 0, 200, 300], Some([20, 20, 100, 280]));
    let d = dump_bytes(&bytes, 0, BASE);
    let i = d.glyphs.iter().find(|g| g.ch == Some('I')).unwrap();
    assert!((i.quad[0] - (10.0 - 20.0)).abs() < 0.01, "{:?}", i.quad);
    // Rotate 90: the page bounds swap and glyph geometry is already rotated.
    let plain = one("BT /F1 20 Tf 50 100 Td (A) Tj ET", [0, 0, 200, 300], None);
    let rotated = edit_first_page(&plain, |doc, page, _| {
        doc.get_dictionary_mut(page).unwrap().set("Rotate", 90);
    });
    for (label, pdf) in [("Rotate 0", &plain), ("Rotate 90", &rotated)] {
        let doc = mupdf::pdf::PdfDocument::from_bytes(pdf).unwrap();
        let page = doc.load_pdf_page(0).unwrap();
        let b = page.bounds().unwrap();
        let g = dump_page(&page, BASE).glyphs[0].clone();
        eprintln!(
            "4a0 rotate [{label}] bounds {}x{} glyph origin {:?} quad {:?}",
            b.x1 - b.x0,
            b.y1 - b.y0,
            g.origin,
            g.quad
        );
    }
    // Rotate 90: bounds 300x200; PDF (x, y) = (50, 100) maps to page space (y, x) = (100, 50),
    // and the quad corners are rotated with the glyph (ul keeps the glyph's own corner).
    let doc = mupdf::pdf::PdfDocument::from_bytes(&rotated).unwrap();
    let page = doc.load_pdf_page(0).unwrap();
    let b = page.bounds().unwrap();
    assert_eq!((b.x1 - b.x0, b.y1 - b.y0), (300.0, 200.0));
    let g = dump_page(&page, BASE).glyphs[0].clone();
    assert_eq!(g.origin, [100.0, 50.0]);
    assert!((g.quad[0] - 121.5).abs() < 0.1 && (g.quad[1] - 50.0).abs() < 0.01);
    assert!((g.quad[4] - 94.0).abs() < 0.1 && (g.quad[5] - 50.0).abs() < 0.01);
    let user_unit = edit_first_page(&plain, |doc, page, _| {
        doc.get_dictionary_mut(page).unwrap().set("UserUnit", 2.0);
    });
    let doc = mupdf::pdf::PdfDocument::from_bytes(&user_unit).unwrap();
    let page = doc.load_pdf_page(0).unwrap();
    let b = page.bounds().unwrap();
    let g = dump_page(&page, BASE).glyphs[0].clone();
    eprintln!(
        "4a0 userunit 2 bounds {}x{} glyph origin {:?}",
        b.x1 - b.x0,
        b.y1 - b.y0,
        g.origin
    );
    // UserUnit 2 scales page space by 2 (bounds 400x600, origin (50, 200) -> (100, 400)).
    assert_eq!((b.x1 - b.x0, b.y1 - b.y0), (400.0, 600.0));
    assert_eq!(g.origin, [100.0, 400.0]);
}

#[test]
fn clip_flag_with_non_rectangular_clip_path() {
    // Triangle clip (0,0)-(100,0)-(0,100). "IN" at (5,5) lies inside it; "OUT" at (70,70) is
    // inside the triangle's bounding box but outside the triangle.
    let content = "q 0 0 m 100 0 l 0 100 l h W n \
        BT /F1 12 Tf 5 5 Td (IN) Tj 1 0 0 1 70 70 Tm (OUT) Tj ET Q";
    let bytes = one(content, [0, 0, 200, 200], None);
    let off = dump_bytes(&bytes, 0, BASE);
    let on = dump_bytes(&bytes, 0, BASE | TextPageFlags::CLIP);
    eprintln!("4a0 triangle {}", summarize("CLIP off", &off));
    eprintln!("4a0 triangle {}", summarize("CLIP on ", &on));
    assert_eq!(text_of(&off), "INOUT");
    // CLIP prunes by the clip bounding box only: "OUT" (outside the triangle, inside its box) stays.
    assert_eq!(text_of(&on), "INOUT");
}
