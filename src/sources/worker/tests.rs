use super::operations::{inspect, render};
use super::*;
use lopdf::{Document, Object, Stream, dictionary};

fn saved(doc: &mut Document) -> Vec<u8> {
    let mut bytes = Vec::new();
    doc.save_to(&mut bytes).unwrap();
    bytes
}

fn font_fixture(kind: &str) -> Vec<u8> {
    let mut doc = Document::load_mem(&fixture(0, [0, 0, 200, 300], 1.0, false)).unwrap();
    let page = doc.get_pages()[&1];
    let resources = doc
        .get_dictionary(page)
        .unwrap()
        .get(b"Resources")
        .unwrap()
        .as_reference()
        .unwrap();
    match kind {
        "cycle" | "shared" => {
            let form = doc.add_object(Stream::new(dictionary!{"Type"=>"XObject", "Subtype"=>"Form", "BBox"=>vec![0.into(),0.into(),10.into(),10.into()], "Resources"=>resources}, Vec::new()));
            doc.get_dictionary_mut(resources)
                .unwrap()
                .set("XObject", dictionary! {"X1"=>form});
            if kind == "shared" {
                let mut objects = lopdf::Dictionary::new();
                for n in 0..10001 {
                    objects.set(format!("X{n}"), form);
                }
                doc.get_dictionary_mut(resources)
                    .unwrap()
                    .set("XObject", objects);
            }
        }
        "pattern" | "appearance" => {
            let font = doc.add_object(
                dictionary! {"Type"=>"Font", "Subtype"=>"Type1", "BaseFont"=>"Courier"},
            );
            let nested = dictionary! {"Font"=>dictionary!{"F2"=>font}};
            let stream = doc.add_object(Stream::new(dictionary!{"Type"=>"XObject", "Subtype"=>"Form", "PatternType"=>1, "BBox"=>vec![0.into(),0.into(),10.into(),10.into()], "Resources"=>nested}, Vec::new()));
            if kind == "pattern" {
                doc.get_dictionary_mut(resources)
                    .unwrap()
                    .set("Pattern", dictionary! {"P1"=>stream});
            } else {
                let annot = doc.add_object(dictionary!{"Type"=>"Annot", "Subtype"=>"Widget", "Rect"=>vec![0.into(),0.into(),10.into(),10.into()], "AP"=>dictionary!{"N"=>stream}});
                doc.get_dictionary_mut(page)
                    .unwrap()
                    .set("Annots", vec![annot.into()]);
            }
        }
        "bad-fonts" => {
            doc.get_dictionary_mut(resources).unwrap().set("Font", 42);
        }
        _ => {
            let mut font = dictionary! {"Type"=>"Font", "Subtype"=>"Type1"};
            if kind == "sjis" {
                font.set("BaseFont", Object::Name(vec![0x82, 0xa0]));
            }
            if kind == "type0" {
                font.set("Subtype", "Type0");
            }
            if kind == "budget" {
                let mut fonts = lopdf::Dictionary::new();
                for n in 0..10001 {
                    fonts.set(format!("F{n}"), font.clone());
                }
                doc.get_dictionary_mut(resources)
                    .unwrap()
                    .set("Font", fonts);
            } else {
                doc.get_dictionary_mut(resources)
                    .unwrap()
                    .set("Font", dictionary! {"F1"=>font});
            }
        }
    }
    saved(&mut doc)
}

fn diagnostic_fonts(kind: &str) {
    let bytes = font_fixture(kind);
    let native = mupdf::pdf::PdfDocument::from_bytes(&bytes)
        .unwrap()
        .load_pdf_page(0)
        .unwrap();
    native
        .to_pixmap(
            &mupdf::Matrix::IDENTITY,
            &mupdf::Colorspace::device_rgb(),
            false,
            false,
        )
        .unwrap();
    let output = render(Input::Bytes(&bytes), &[1]);
    assert!(output.is_ok(), "{kind}: {output:?}");
    let stats = serde_json::to_value(&output.unwrap()[0].stats).unwrap();
    if kind == "sjis" {
        assert_eq!(stats["fonts_substituted"][0], "#82#a0");
    }
    if kind == "missing" {
        assert_eq!(stats["fonts_substituted"][0], "Unknown");
    }
    if kind == "type0" || kind == "bad-fonts" {
        assert!(stats["fonts_skipped"].as_u64().unwrap_or(0) > 0);
        assert_eq!(stats["incomplete"], true);
    }
    if kind == "pattern" || kind == "appearance" {
        assert!(
            stats["fonts_substituted"]
                .as_array()
                .unwrap()
                .contains(&serde_json::json!("Courier"))
        );
    }
    if kind == "budget" {
        assert_eq!(stats["incomplete"], true);
    }
    if kind == "cycle" || kind == "shared" {
        assert_eq!(stats["incomplete"], false);
    }
}

#[test]
fn font_cycle() {
    diagnostic_fonts("cycle");
}
#[test]
fn font_shared() {
    diagnostic_fonts("shared");
}

#[test]
fn system_and_malloc_errors_are_too_large() {
    let error = mupdf::Error::MuPdf(mupdf::error::MuPdfError {
        code: 2,
        message: "dummy".into(),
    });
    assert_eq!(Error::from(error), Error::TooLarge);
}
#[test]
fn font_sjis() {
    diagnostic_fonts("sjis");
}
#[test]
fn font_missing() {
    diagnostic_fonts("missing");
}
#[test]
fn font_type0() {
    diagnostic_fonts("type0");
}
#[test]
fn font_bad_dictionary() {
    diagnostic_fonts("bad-fonts");
}
#[test]
fn font_pattern() {
    diagnostic_fonts("pattern");
}
#[test]
fn font_appearance() {
    diagnostic_fonts("appearance");
}
#[test]
fn font_budget() {
    diagnostic_fonts("budget");
}

#[test]
fn eof_tolerates_trailing_bytes() {
    let mut bytes = fixture(0, [0, 0, 200, 300], 1.0, false);
    bytes.extend_from_slice(b"\0\0 trailing garbage");
    assert!(inspect(Input::Bytes(&bytes)).is_ok());
    bytes.extend_from_slice(&[b'x'; 1024]);
    assert_eq!(
        inspect(Input::Bytes(&bytes)).unwrap_err(),
        Error::Unreadable
    );
}

#[test]
fn reversed_box_corners_are_normalized() {
    let bytes = fixture(0, [200, 300, 0, 0], 1.0, false);
    let page = inspect(Input::Bytes(&bytes)).unwrap().pages.remove(0);
    assert_eq!((page.width, page.height), (200.0, 300.0));
}

#[test]
fn crop_edge_text_counts_only_visible_characters() {
    // Courier is 6 pt wide at 10 pt. The CropBox intersects C, but D starts at x=53.
    let mut doc = Document::load_mem(&fixture(0, [0, 0, 52, 300], 1.0, false)).unwrap();
    let page = doc.get_pages()[&1];
    let resources = doc
        .get_dictionary(page)
        .unwrap()
        .get(b"Resources")
        .unwrap()
        .as_reference()
        .unwrap();
    // Make the off-page F explicitly unmapped, so the unmapped count is exercised too.
    let unicode = doc.add_object(Stream::new(
        dictionary! {},
        br"/CIDInit /ProcSet findresource begin
12 dict begin begincmap
/CIDSystemInfo << /Registry (Adobe) /Ordering (UCS) /Supplement 0 >> def
/CMapName /Synthetic def /CMapType 2 def
1 begincodespacerange <00> <FF> endcodespacerange
6 beginbfchar
<41> <0041> <42> <0042> <43> <0043>
<44> <0044> <45> <0045> <46> <FFFD>
endbfchar
endcmap CMapName currentdict /CMap defineresource pop end end"
            .to_vec(),
    ));
    let font = doc.add_object(dictionary! {"Type"=>"Font", "Subtype"=>"Type1", "BaseFont"=>"Courier", "ToUnicode"=>unicode});
    doc.get_dictionary_mut(resources)
        .unwrap()
        .set("Font", dictionary! {"F1"=>font});
    let content = doc.add_object(Stream::new(
        dictionary! {},
        b"BT /F1 10 Tf 35 60 Td (ABCDEF) Tj ET".to_vec(),
    ));
    doc.get_dictionary_mut(page)
        .unwrap()
        .set("Contents", content);
    let bytes = saved(&mut doc);
    let output = render(Input::Bytes(&bytes), &[1]).unwrap();
    assert_eq!(output[0].lines.len(), 1);
    assert_eq!(output[0].lines[0].text, "ABC");
    assert_eq!(output[0].stats.chars, 3);
    assert_eq!(output[0].stats.unmapped, 0);
    doc.get_dictionary_mut(page)
        .unwrap()
        .set("CropBox", vec![0.into(), 0.into(), 200.into(), 300.into()]);
    let bytes = saved(&mut doc);
    let full = render(Input::Bytes(&bytes), &[1]).unwrap();
    assert_eq!(full[0].lines[0].text, "ABCDE\u{fffd}");
    assert_eq!(full[0].stats.chars, 6);
    assert_eq!(full[0].stats.unmapped, 1);
}

#[test]
fn hidden_zero_size_text_is_dropped() {
    let mut doc = Document::load_mem(&fixture(0, [0, 0, 200, 300], 1.0, false)).unwrap();
    let p = doc.get_pages()[&1];
    let content = doc.add_object(Stream::new(
        dictionary! {},
        b"BT /F1 0 Tf 35 60 Td (Hidden) Tj ET".to_vec(),
    ));
    doc.get_dictionary_mut(p).unwrap().set("Contents", content);
    let bytes = saved(&mut doc);
    let output = render(Input::Bytes(&bytes), &[1]).unwrap();
    assert!(output[0].lines.is_empty());
    assert!(
        serde_json::to_value(&output[0].stats).unwrap()["dropped_lines"]
            .as_u64()
            .unwrap_or(0)
            > 0
    );
}

#[test]
fn tagged_requires_catalog_marking_and_structure() {
    for marked in [false, true] {
        let mut doc = Document::load_mem(&fixture(0, [0, 0, 200, 300], 1.0, false)).unwrap();
        let root = doc.trailer.get(b"Root").unwrap().as_reference().unwrap();
        let catalog = doc.get_dictionary_mut(root).unwrap();
        catalog.set("MarkInfo", dictionary! {"Marked"=>marked});
        catalog.set("StructTreeRoot", dictionary! {"Type"=>"StructTreeRoot"});
        let bytes = saved(&mut doc);
        assert_eq!(
            inspect(Input::Bytes(&bytes)).unwrap().pages[0].tagged,
            marked
        );
    }
}

#[test]
fn reported_version_matches_bundled_header() {
    let output = std::process::Command::new(env!("CARGO"))
        .args(["metadata", "--offline", "--locked", "--format-version", "1"])
        .current_dir(env!("CARGO_MANIFEST_DIR"))
        .output()
        .unwrap();
    assert!(output.status.success(), "cargo metadata failed");
    let metadata: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    let manifest = metadata["packages"]
        .as_array()
        .unwrap()
        .iter()
        .find(|p| p["name"] == "mupdf-sys")
        .unwrap()["manifest_path"]
        .as_str()
        .unwrap();
    let path = std::path::Path::new(manifest)
        .parent()
        .unwrap()
        .join("mupdf/include/mupdf/fitz/version.h");
    let header = std::fs::read_to_string(path).unwrap();
    let version = header
        .lines()
        .find_map(|line| {
            line.strip_prefix("#define FZ_VERSION \"")
                .and_then(|s| s.strip_suffix('"'))
        })
        .unwrap();
    let bytes = fixture(0, [0, 0, 200, 300], 1.0, false);
    assert_eq!(
        inspect(Input::Bytes(&bytes)).unwrap().document.mupdf,
        version
    );
}

pub(super) fn fixture(rotate: i32, crop: [i32; 4], unit: f32, inherited: bool) -> Vec<u8> {
    let mut doc = Document::with_version("1.7");
    let pages = doc.new_object_id();
    let font =
        doc.add_object(dictionary! {"Type"=>"Font","Subtype"=>"Type1","BaseFont"=>"Helvetica"});
    let resources = doc.add_object(dictionary! {"Font"=>dictionary!{"F1"=>font}});
    let content = doc.add_object(Stream::new(
        dictionary! {},
        b"0 0 0 rg 35 45 10 10 re f BT /F1 10 Tf 35 60 Td (Corner) Tj ET".to_vec(),
    ));
    let mut page = dictionary! {"Type"=>"Page","Parent"=>pages,"Contents"=>content,"Resources"=>resources,"UserUnit"=>unit};
    let crop: Vec<Object> = crop.into_iter().map(Object::from).collect();
    let mut parent = dictionary! {"Type"=>"Pages","Count"=>1,"MediaBox"=>vec![0.into(),0.into(),200.into(),300.into()]};
    if inherited {
        parent.set("Rotate", rotate);
        parent.set("CropBox", crop);
    } else {
        page.set("Rotate", rotate);
        page.set("CropBox", crop);
    }
    let page_id = doc.add_object(page);
    parent.set("Kids", vec![page_id.into()]);
    doc.objects.insert(pages, Object::Dictionary(parent));
    let root = doc.add_object(dictionary! {"Type"=>"Catalog","Pages"=>pages});
    doc.trailer.set("Root", root);
    let mut bytes = Vec::new();
    doc.save_to(&mut bytes).unwrap();
    bytes
}

#[test]
fn geometry_landmarks() {
    for (rotate, crop, unit, inherited, size, transform, centre) in [
        (
            0,
            [0, 0, 200, 300],
            1.0,
            false,
            (200.0, 300.0),
            [1.0, 0.0, 0.0, -1.0, 0.0, 300.0],
            (40.0, 250.0),
        ),
        (
            90,
            [0, 0, 200, 300],
            1.0,
            false,
            (300.0, 200.0),
            [0.0, 1.0, 1.0, 0.0, 0.0, 0.0],
            (50.0, 40.0),
        ),
        (
            180,
            [0, 0, 200, 300],
            1.0,
            false,
            (200.0, 300.0),
            [-1.0, 0.0, 0.0, 1.0, 200.0, 0.0],
            (160.0, 50.0),
        ),
        (
            270,
            [0, 0, 200, 300],
            1.0,
            false,
            (300.0, 200.0),
            [0.0, -1.0, -1.0, 0.0, 300.0, 200.0],
            (250.0, 160.0),
        ),
        (
            0,
            [20, 30, 180, 280],
            1.0,
            false,
            (160.0, 250.0),
            [1.0, 0.0, 0.0, -1.0, -20.0, 280.0],
            (20.0, 230.0),
        ),
        (
            0,
            [-100, -100, 400, 500],
            1.0,
            false,
            (200.0, 300.0),
            [1.0, 0.0, 0.0, -1.0, 0.0, 300.0],
            (40.0, 250.0),
        ),
        (
            0,
            [0, 0, 200, 300],
            2.0,
            false,
            (400.0, 600.0),
            [2.0, 0.0, 0.0, -2.0, 0.0, 600.0],
            (80.0, 500.0),
        ),
        (
            90,
            [20, 30, 180, 280],
            2.0,
            true,
            (500.0, 320.0),
            [0.0, 2.0, 2.0, 0.0, -60.0, -40.0],
            (40.0, 40.0),
        ),
    ] {
        let bytes = fixture(rotate, crop, unit, inherited);
        check_geometry(&bytes, size, transform, centre);
    }
}

fn check_geometry(bytes: &[u8], size: (f64, f64), transform: [f64; 6], centre: (f64, f64)) {
    let inspection = inspect(Input::Bytes(bytes)).unwrap();
    let info = &inspection.pages[0];
    assert_eq!((info.width, info.height), size);
    assert_eq!(info.transform, transform);
    let native = mupdf::pdf::PdfDocument::from_bytes(bytes)
        .unwrap()
        .load_pdf_page(0)
        .unwrap();
    let ctm = native.ctm().unwrap();
    assert_eq!(
        [ctm.a, ctm.b, ctm.c, ctm.d, ctm.e, ctm.f].map(f64::from),
        transform
    );
    let bounds = native.bounds().unwrap();
    assert_eq!(
        (f64::from(bounds.width()), f64::from(bounds.height())),
        size
    );
    let page = render(Input::Bytes(bytes), &[1]).unwrap().remove(0);
    assert!(page.lines.iter().any(|l| l.text == "Corner"));
    let b = geometry::transformed_box([35.0, 45.0, 45.0, 55.0], info.transform);
    assert_eq!((b[0].midpoint(b[2]), b[1].midpoint(b[3])), centre);
    let x = (centre.0 / size.0 * f64::from(page.width_px)).floor();
    let y = (centre.1 / size.1 * f64::from(page.height_px)).floor();
    let mut decoder = png::Decoder::new(std::io::Cursor::new(&page.png))
        .read_info()
        .unwrap();
    let mut pixels = vec![0; decoder.output_buffer_size().unwrap()];
    decoder.next_frame(&mut pixels).unwrap();
    let x: usize = x.to_string().parse().unwrap();
    let y: usize = y.to_string().parse().unwrap();
    let offset = (y * usize::try_from(page.width_px).unwrap() + x) * 3;
    assert!(pixels[offset..offset + 3].iter().all(|&c| c < 20));
    let text = &page.lines[0].bounds;
    let square_corner = ((b[0] + b[2]) < info.width, (b[1] + b[3]) < info.height);
    assert_eq!(
        square_corner,
        (
            (text[0] + text[2]) < info.width,
            (text[1] + text[3]) < info.height
        )
    );
    let mut wire = Vec::new();
    let mut writer = records::RecordWriter::new(&mut wire, records::Operation::Render, 1);
    writer.rendered(&page).unwrap();
    writer.finish().unwrap();
    let mut reader = records::RenderReader::new(&wire[..], vec![info.clone()]).unwrap();
    assert_eq!(reader.next_page().unwrap().unwrap().lines[0].text, "Corner");
    assert!(reader.next_page().unwrap().is_none());
}

#[test]
fn refusals_and_inspection_wire() {
    assert_eq!(
        inspect(Input::Bytes(b"not a PDF")).unwrap_err(),
        Error::Unreadable
    );
    let mut bytes = fixture(0, [0, 0, 200, 300], 1.0, false);
    bytes.truncate(bytes.len() / 2);
    assert_eq!(
        inspect(Input::Bytes(&bytes)).unwrap_err(),
        Error::Unreadable
    );
    assert_eq!(
        inspect(Input::Bytes(&fixture(
            0,
            [0, 0, 200, 300],
            100_000.0,
            false
        )))
        .unwrap_err(),
        Error::TooLarge
    );
    let bytes = fixture(0, [0, 0, 200, 300], 1.0, false);
    let inspection = inspect(Input::Bytes(&bytes)).unwrap();
    let mut wire = Vec::new();
    let mut writer = records::RecordWriter::new(&mut wire, records::Operation::Inspect, 1);
    writer.inspection(&inspection).unwrap();
    writer.finish().unwrap();
    assert_eq!(records::read_inspection(&wire[..]).unwrap().pages.len(), 1);
    wire.push(b'x');
    assert!(records::read_inspection(&wire[..]).is_err());
    assert!(records::read_inspection(&vec![b'x'; 4097][..]).is_err());
}

#[test]
fn password_and_claimed_page_count() {
    let bytes = fixture(0, [0, 0, 200, 300], 1.0, false);
    for password in ["dummy-user", ""] {
        let mut doc = Document::load_mem(&bytes).unwrap();
        doc.trailer.set(
            "ID",
            Object::Array(vec![
                Object::string_literal("synthetic-id-0001"),
                Object::string_literal("synthetic-id-0001"),
            ]),
        );
        let encryption = lopdf::EncryptionState::try_from(lopdf::EncryptionVersion::V2 {
            document: &doc,
            owner_password: "dummy-owner",
            user_password: password,
            key_length: 128,
            permissions: lopdf::Permissions::all(),
        })
        .unwrap();
        doc.encrypt(&encryption).unwrap();
        let mut encrypted = Vec::new();
        doc.save_to(&mut encrypted).unwrap();
        let result = inspect(Input::Bytes(&encrypted));
        if password.is_empty() {
            assert!(result.unwrap().document.encrypted);
            assert!(render(Input::Bytes(&encrypted), &[1]).is_ok());
        } else {
            assert_eq!(result.unwrap_err(), Error::PasswordProtected);
        }
    }
    let mut doc = Document::load_mem(&bytes).unwrap();
    let root = doc.trailer.get(b"Root").unwrap().as_reference().unwrap();
    let pages = doc
        .get_dictionary(root)
        .unwrap()
        .get(b"Pages")
        .unwrap()
        .as_reference()
        .unwrap();
    doc.get_dictionary_mut(pages).unwrap().set("Count", 2001);
    let mut many = Vec::new();
    doc.save_to(&mut many).unwrap();
    assert_eq!(
        inspect(Input::Bytes(&many)).unwrap_err(),
        Error::TooManyPages
    );
}

#[test]
fn long_normal_page_renders() {
    let mut doc = Document::load_mem(&fixture(0, [0, 0, 4000, 100], 1.0, false)).unwrap();
    let page = doc.get_pages()[&1];
    doc.get_dictionary_mut(page).unwrap().set(
        "MediaBox",
        Object::Array(vec![0.into(), 0.into(), 4000.into(), 100.into()]),
    );
    let mut bytes = Vec::new();
    doc.save_to(&mut bytes).unwrap();
    let result = render(Input::Bytes(&bytes), &[1]).unwrap();
    assert_eq!(
        (result[0].width_px, result[0].height_px),
        pixel_size(4000.0, 100.0).unwrap()
    );
}

#[test]
fn overlapping_images_count_once() {
    let mut doc = Document::load_mem(&fixture(0, [0, 0, 200, 300], 1.0, false)).unwrap();
    let page = doc.get_pages()[&1];
    let image=doc.add_object(Stream::new(dictionary!{"Type"=>"XObject","Subtype"=>"Image","Width"=>1,"Height"=>1,"ColorSpace"=>"DeviceRGB","BitsPerComponent"=>8},vec![255,0,0]));
    let resources = doc
        .get_dictionary(page)
        .unwrap()
        .get(b"Resources")
        .unwrap()
        .as_reference()
        .unwrap();
    doc.get_dictionary_mut(resources)
        .unwrap()
        .set("XObject", dictionary! {"I1"=>image});
    let content = doc.add_object(Stream::new(
        dictionary! {},
        b"q 100 0 0 100 0 0 cm /I1 Do /I1 Do Q".to_vec(),
    ));
    doc.get_dictionary_mut(page)
        .unwrap()
        .set("Contents", content);
    let mut bytes = Vec::new();
    doc.save_to(&mut bytes).unwrap();
    let page = render(Input::Bytes(&bytes), &[1]).unwrap().remove(0);
    assert!((page.stats.image_area - 1.0 / 6.0).abs() < 1e-6);
}

#[test]
fn pixel_bounds_and_extreme_aspects() {
    assert_eq!(pixel_size(3999.5, 1.0), Err(Error::Unsupported));
    for (w, h) in [
        (4000.0, 100.0),
        (1.0, 4000.0),
        (4000.0, 4000.0),
        (612.0, 792.0),
        (1.0, 1.0),
    ] {
        let (x, y) = pixel_size(w, h).unwrap();
        let mut info = png::Info::default();
        info.width = x;
        info.height = y;
        super::super::import::validate_image_header(&info, w, h).unwrap();
    }
}

#[test]
fn mupdf_stays_in_worker() {
    fn visit(path: &std::path::Path) {
        let native = regex::Regex::new(r"\bmupdf\b").unwrap();
        let operations = operations_reference();
        for entry in std::fs::read_dir(path).unwrap() {
            let path = entry.unwrap().path();
            if path.ends_with("sources/worker") {
                continue;
            }
            if path.is_dir() {
                visit(&path);
            } else {
                let content = std::fs::read_to_string(&path).unwrap();
                assert!(!native.is_match(&content), "{}", path.display());
                assert!(!operations.is_match(&content), "{}", path.display());
            }
        }
    }
    visit(&std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src"));
}

fn operations_reference() -> regex::Regex {
    regex::Regex::new(r"(?s)\b(?:(?:worker|operations)\s*::\s*(?:operations\b|(?:Input|inspect|render|render_each)\b|\{[^;]*\b(?:Input|inspect|render|render_each)\b)|(?:inspect|render|render_each)\s*\(\s*(?:Input|operations\s*::\s*Input)\s*::)").unwrap()
}

#[test]
fn boundary_matchers_cover_aliases_and_imports() {
    let native = regex::Regex::new(r"\bmupdf\b").unwrap();
    for text in ["use mupdf as m;", "extern crate mupdf;", "mupdf::Context"] {
        assert!(native.is_match(text));
    }
    assert!(!native.is_match("mupdf_error"));
    let operations = operations_reference();
    for text in [
        "worker::inspect(input)",
        "worker::{Input, render as r}",
        "operations::render_each(input)",
        "worker::operations as op",
        "render(Input::Bytes(bytes), &[1])",
    ] {
        assert!(operations.is_match(text), "{text}");
    }
    assert!(!operations.is_match("worker::{Spec, records::read_inspection}"));
}
