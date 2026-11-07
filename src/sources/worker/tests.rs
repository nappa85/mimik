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
fn rotated_render_exclusion_round_trip_uses_displayed_points() {
    for rotation in [90, 180, 270] {
        let bytes = fixture(rotation, [10, 20, 180, 270], 2.0, true);
        let inspected = inspect(Input::Bytes(&bytes)).unwrap();
        let image = render(Input::Bytes(&bytes), &[1]).unwrap().remove(0);
        crate::sources::uploads::tests::assert_rendered_exclusion_round_trip(inspected, image);
    }
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
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
    let worker = root.join("src/sources/worker");
    let operations = operations_reference();
    for path in boundary_files(root) {
        let content = std::fs::read_to_string(&path).unwrap();
        if path.file_name().is_some_and(|n| n == "Cargo.toml") {
            assert!(!cargo_alias_reference(&content), "{}", path.display());
        } else if path == root.join("Cargo.lock") {
            // Only include_str! can reach Cargo.lock: Rust references require .rs.
            // It is provenance data, so its native package names are exempt.
            assert!(syn::parse_file(&content).is_err());
        } else if !path.starts_with(&worker) {
            assert!(!native_rust_reference(&content), "{}", path.display());
            assert!(!operations.is_match(&content), "{}", path.display());
        }
    }
}

fn boundary_visit(
    path: &std::path::Path,
    files: &mut Vec<std::path::PathBuf>,
    rules: &[BoundaryIgnore],
) {
    let metadata = match std::fs::symlink_metadata(path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return,
        Err(error) => panic!("{}: {error}", path.display()),
    };
    if boundary_ignored(path, metadata.is_dir(), rules) {
        return;
    }
    assert!(
        !metadata.is_symlink(),
        "symlink in source boundary: {}",
        path.display()
    );
    if metadata.is_file() {
        if path.file_name().is_some_and(|n| n == "Cargo.toml")
            || path.extension().is_some_and(|extension| extension == "rs")
        {
            files.push(path.to_path_buf());
        }
        return;
    }
    let mut rules = rules.to_vec();
    boundary_ignore_rules(path, &mut rules);
    for entry in std::fs::read_dir(path).unwrap() {
        let path = entry.unwrap().path();
        boundary_visit(&path, files, &rules);
    }
}

fn boundary_files(root: &std::path::Path) -> Vec<std::path::PathBuf> {
    for name in [".cargo/config", ".cargo/config.toml"] {
        let path = boundary_target(root, root, name);
        assert!(
            !path.exists(),
            "unsupported Cargo source configuration: {name}"
        );
    }
    let mut files = Vec::new();
    let mut rules = Vec::new();
    boundary_ignore_rules(root, &mut rules);
    for name in [
        "src",
        "tests",
        "examples",
        "benches",
        "tools",
        "build.rs",
        "Cargo.toml",
    ] {
        boundary_visit(&root.join(name), &mut files, &rules);
    }
    // Cargo always reads the checkout's root manifest, even when it is ignored.
    let manifest = boundary_target(root, root, "Cargo.toml");
    assert!(manifest.is_file(), "missing root Cargo.toml");
    if !files.contains(&manifest) {
        files.push(manifest);
    }
    let crate_roots = boundary_crate_roots(root, &mut files);
    boundary_follow_sources(root, &mut files, &crate_roots);
    files.sort();
    files.dedup();
    files
}

const VENDORED_MUPDF: &str = "vendor/mupdf";

fn boundary_crate_roots(
    root: &std::path::Path,
    files: &mut Vec<std::path::PathBuf>,
) -> Vec<std::path::PathBuf> {
    let manifests: Vec<_> = files
        .iter()
        .filter(|p| p.file_name().unwrap() == "Cargo.toml")
        .cloned()
        .collect();
    let mut crate_roots = Vec::new();
    for manifest in manifests {
        for (path, kind) in cargo_source_paths(&manifest) {
            let target = boundary_target(root, manifest.parent().unwrap(), &path);
            assert!(
                !target.starts_with(root.join("src/sources/worker")),
                "Cargo {kind} inside worker: {path}"
            );
            // The single vendored renderer fork is the dependency's own source (DEP-29): it is
            // allowed only as the root manifest's exact patch entry, and is not walked.
            if kind == "vendored-patch" {
                assert!(
                    manifest == root.join("Cargo.toml") && path == VENDORED_MUPDF,
                    "Cargo {kind} outside the allowed form: {path}"
                );
                assert!(
                    target == root.join(VENDORED_MUPDF) && target.join("Cargo.toml").is_file(),
                    "Cargo {kind} missing: {path}"
                );
                continue;
            }
            assert!(
                ["src", "tests", "examples", "benches", "tools"]
                    .iter()
                    .any(|name| target.starts_with(root.join(name)))
                    || (kind == "build" && target == root.join("build.rs")),
                "Cargo {kind} outside walked roots: {path}"
            );
            assert!(
                if kind == "member" || kind == "dependency" {
                    files.contains(&target.join("Cargo.toml"))
                } else {
                    target.is_file()
                },
                "Cargo {kind} not scanned: {}",
                target.display()
            );
            if kind == "target" || kind == "build" {
                crate_roots.push(target);
            }
        }
        crate_roots.extend(boundary_default_roots(root, manifest.parent().unwrap()));
    }
    files.extend(crate_roots.iter().cloned());
    assert!(
        files.contains(&root.join("src/main.rs")),
        "source walk missed src/main.rs"
    );
    crate_roots
}

fn boundary_default_roots(
    root: &std::path::Path,
    base: &std::path::Path,
) -> Vec<std::path::PathBuf> {
    let mut crate_roots = Vec::new();
    for name in ["src/main.rs", "src/lib.rs", "build.rs"] {
        let path = boundary_target(root, base, name);
        if path.exists() {
            crate_roots.push(path);
        }
    }
    for directory in ["src/bin", "tests", "examples", "benches"] {
        let directory = boundary_target(root, base, directory);
        if !directory.exists() {
            continue;
        }
        for entry in std::fs::read_dir(&directory).unwrap() {
            let entry = entry.unwrap();
            let path = entry.path();
            let path = if path.extension().is_some_and(|e| e == "rs") {
                boundary_target(root, &directory, entry.file_name().to_str().unwrap())
            } else if path.is_dir() && path.join("main.rs").exists() {
                boundary_target(
                    root,
                    &directory,
                    &format!("{}/main.rs", entry.file_name().to_str().unwrap()),
                )
            } else {
                continue;
            };
            crate_roots.push(path);
        }
    }
    crate_roots
}

fn boundary_follow_sources(
    root: &std::path::Path,
    files: &mut Vec<std::path::PathBuf>,
    crate_roots: &[std::path::PathBuf],
) {
    // Reachability bypasses ignore rules. Unreachable files still receive the lint,
    // but only the broad walk above prunes caches and other ignored content.
    let mut pending: Vec<_> = files
        .iter()
        .filter(|p| p.file_name().unwrap() != "Cargo.toml")
        .map(|path| (path.clone(), None))
        .collect();
    pending.extend(crate_roots.iter().map(|path| {
        let directory = path.parent().unwrap().to_path_buf();
        (path.clone(), Some((directory.clone(), directory)))
    }));
    let mut seen = std::collections::HashSet::new();
    while let Some((path, context)) = pending.pop() {
        if !seen.insert((path.clone(), context.clone())) {
            continue;
        }
        let content = std::fs::read_to_string(&path).unwrap();
        let worker = path.starts_with(root.join("src/sources/worker"));
        if !worker {
            boundary_check_module_tokens(&content);
        }
        if let Some((base, attribute_base)) = &context {
            for (target, directory) in boundary_modules(root, &path, base, attribute_base, &content)
            {
                let attributes = target.parent().unwrap().to_path_buf();
                pending.push((target.clone(), Some((directory, attributes))));
                files.push(target);
            }
        }
        if worker {
            continue;
        }
        for (literal, rust) in boundary_references(&content) {
            let target = boundary_target(root, path.parent().unwrap(), &literal);
            if rust {
                assert!(
                    !target.starts_with(root.join("src/sources/worker")),
                    "worker source pulled outside worker: {literal}"
                );
                assert!(
                    target.extension().is_some_and(|e| e == "rs"),
                    "non-Rust source: {literal}"
                );
                assert!(
                    ["src", "tests", "examples", "benches", "tools"]
                        .iter()
                        .any(|name| target.starts_with(root.join(name))),
                    "source outside scanned roots: {literal}"
                );
                // include! preserves the including module's module and attribute
                // bases, even when the included text lives in another directory.
                pending.push((target.clone(), context.clone()));
            }
            assert!(
                target.is_file(),
                "missing included source: {}",
                target.display()
            );
            files.push(target);
        }
    }
}

fn boundary_module_directory(path: &std::path::Path) -> std::path::PathBuf {
    if path.file_name().unwrap() == "mod.rs" {
        path.parent().unwrap().to_path_buf()
    } else {
        path.with_extension("")
    }
}

fn cargo_source_paths(manifest: &std::path::Path) -> Vec<(String, &'static str)> {
    let text = std::fs::read_to_string(manifest).unwrap();
    let doc = text.parse::<toml_edit::DocumentMut>().unwrap();
    let mut paths = Vec::new();
    if let Some(build) = doc.get("package").and_then(|p| p.get("build")) {
        if let Some(path) = build.as_str() {
            paths.push((path.to_owned(), "build"));
        } else {
            assert!(build.as_bool().is_some(), "invalid package.build");
        }
    }
    if let Some(path) = doc.get("lib").and_then(|lib| lib.get("path")) {
        paths.push((path.as_str().unwrap().to_owned(), "target"));
    }
    for kind in ["bench", "bin", "example", "test"] {
        if let Some(targets) = doc.get(kind) {
            if let Some(tables) = targets.as_array_of_tables() {
                for target in tables {
                    if let Some(path) = target.get("path") {
                        paths.push((path.as_str().unwrap().to_owned(), "target"));
                    }
                }
            } else {
                for target in targets.as_array().unwrap() {
                    if let Some(path) = target.as_inline_table().unwrap().get("path") {
                        paths.push((path.as_str().unwrap().to_owned(), "target"));
                    }
                }
            }
        }
    }
    if let Some(members) = doc.get("workspace").and_then(|w| w.get("members")) {
        for member in members.as_array().unwrap() {
            let path = member.as_str().unwrap();
            assert!(
                !path.contains(['*', '?', '[', ']']),
                "workspace glob requires explicit members"
            );
            paths.push((path.to_owned(), "member"));
        }
    }
    cargo_dependency_sections(doc.as_item(), &mut paths);
    if let Some(workspace) = doc.get("workspace") {
        cargo_dependency_sections(workspace, &mut paths);
    }
    paths
}

fn cargo_dependencies(item: &toml_edit::Item, paths: &mut Vec<(String, &'static str)>) {
    cargo_dependencies_except(item, None, paths);
}
fn cargo_dependencies_except(
    item: &toml_edit::Item,
    skip: Option<&str>,
    paths: &mut Vec<(String, &'static str)>,
) {
    if let Some(table) = item.as_table_like() {
        for (_, dependency) in table.iter().filter(|(name, _)| Some(*name) != skip) {
            if let Some(path) = dependency.get("path") {
                paths.push((
                    path.as_str().expect("invalid dependency path").to_owned(),
                    "dependency",
                ));
            }
        }
    }
}
fn cargo_dependency_sections(item: &toml_edit::Item, paths: &mut Vec<(String, &'static str)>) {
    for name in [
        "dependencies",
        "dev-dependencies",
        "build-dependencies",
        "replace",
    ] {
        if let Some(table) = item.get(name) {
            cargo_dependencies(table, paths);
        }
    }
    for name in ["target", "patch"] {
        if let Some(table) = item.get(name).and_then(toml_edit::Item::as_table_like) {
            for (registry, child) in table.iter() {
                if name == "patch" {
                    // Exactly `[patch.crates-io] mupdf = { path = "vendor/mupdf" }` (no other
                    // keys, so no rename or feature change) is the vendored fork (DEP-29).
                    let vendored = (registry == "crates-io")
                        .then(|| child.get("mupdf").and_then(toml_edit::Item::as_table_like))
                        .flatten()
                        .filter(|dep| {
                            dep.len() == 1
                                && dep.get("path").and_then(toml_edit::Item::as_str)
                                    == Some(VENDORED_MUPDF)
                        });
                    if vendored.is_some() {
                        paths.push((VENDORED_MUPDF.to_owned(), "vendored-patch"));
                        cargo_dependencies_except(child, Some("mupdf"), paths);
                    } else {
                        cargo_dependencies(child, paths);
                    }
                } else {
                    cargo_dependency_sections(child, paths);
                }
            }
        }
    }
}

fn boundary_module_paths(meta: &syn::Meta, result: &mut Vec<String>) {
    if let syn::Meta::NameValue(value) = meta {
        if value.path.is_ident("path") {
            let syn::Expr::Lit(syn::ExprLit {
                lit: syn::Lit::Str(value),
                ..
            }) = &value.value
            else {
                panic!("nonliteral path attribute")
            };
            result.push(value.value());
        }
    } else if let syn::Meta::List(list) = meta
        && list.path.is_ident("cfg_attr")
    {
        let entries = list
            .parse_args_with(
                syn::punctuated::Punctuated::<syn::Meta, syn::Token![,]>::parse_terminated,
            )
            .unwrap();
        for entry in entries.iter().skip(1) {
            boundary_module_paths(entry, result);
        }
    }
}

fn boundary_modules(
    root: &std::path::Path,
    source: &std::path::Path,
    base: &std::path::Path,
    attribute_base: &std::path::Path,
    content: &str,
) -> Vec<(std::path::PathBuf, std::path::PathBuf)> {
    // Use syn's visitor for declarations inside functions/blocks as well as
    // top-level items. Stop at each module so its directory can be scoped below.
    #[derive(Default)]
    struct Modules<'a>(Vec<&'a syn::ItemMod>);
    impl<'ast> syn::visit::Visit<'ast> for Modules<'ast> {
        fn visit_item_mod(&mut self, module: &'ast syn::ItemMod) {
            self.0.push(module);
        }
    }
    fn walk(
        root: &std::path::Path,
        items: &[syn::Item],
        base: &std::path::Path,
        attribute_base: &std::path::Path,
        result: &mut Vec<(std::path::PathBuf, std::path::PathBuf)>,
        worker: bool,
    ) {
        let mut modules = Modules::default();
        for item in items {
            syn::visit::Visit::visit_item(&mut modules, item);
        }
        for module in modules.0 {
            let mut literals = Vec::new();
            for attr in &module.attrs {
                boundary_module_paths(&attr.meta, &mut literals);
            }
            let name = module.ident.to_string().trim_start_matches("r#").to_owned();
            // A conditional path does not replace the default on every target.
            let implicit = !module.attrs.iter().any(|a| a.path().is_ident("path"));
            if let Some((_, items)) = &module.content {
                let mut directories: Vec<_> = literals
                    .iter()
                    .map(|p| boundary_target(root, attribute_base, p))
                    .collect();
                if implicit {
                    directories.push(base.join(name));
                }
                for directory in directories {
                    assert!(
                        worker || !directory.starts_with(root.join("src/sources/worker")),
                        "worker inline module outside worker"
                    );
                    walk(root, items, &directory, &directory, result, worker);
                }
            } else {
                if implicit {
                    result.extend(
                        boundary_implicit_module(root, base, &name, literals.is_empty())
                            .into_iter()
                            .map(|target| {
                                let directory = boundary_module_directory(&target);
                                (target, directory)
                            }),
                    );
                }
                for literal in literals {
                    let target = boundary_target(root, attribute_base, &literal);
                    assert!(
                        !target.starts_with(root.join("src/sources/worker")) || worker,
                        "worker path module outside worker"
                    );
                    assert!(
                        target.is_file() && target.extension().is_some_and(|e| e == "rs"),
                        "missing/non-Rust path module: {literal}"
                    );
                    assert!(
                        ["src", "tests", "examples", "benches", "tools"]
                            .iter()
                            .any(|name| target.starts_with(root.join(name))),
                        "source outside scanned roots: {literal}"
                    );
                    // rustc treats an out-of-line #[path] target as mod.rs-style.
                    let directory = target.parent().unwrap().to_path_buf();
                    result.push((target, directory));
                }
            }
        }
    }
    let mut result = Vec::new();
    walk(
        root,
        &syn::parse_file(content).expect("invalid Rust source").items,
        base,
        attribute_base,
        &mut result,
        source.starts_with(root.join("src/sources/worker")),
    );
    result
}

fn boundary_check_module_tokens(content: &str) {
    fn token_count(stream: proc_macro2::TokenStream) -> usize {
        use proc_macro2::TokenTree;
        let tokens: Vec<_> = stream.into_iter().collect();
        let here = tokens
            .windows(3)
            .filter(|window| {
                matches!(&window[0], TokenTree::Ident(ident) if ident == "mod")
                    && matches!(&window[1], TokenTree::Ident(_))
                    && matches!(&window[2], TokenTree::Punct(punct) if punct.as_char() == ';')
            })
            .count();
        here + tokens
            .into_iter()
            .map(|token| {
                if let TokenTree::Group(group) = token {
                    token_count(group.stream())
                } else {
                    0
                }
            })
            .sum::<usize>()
    }
    #[derive(Default)]
    struct Modules(usize);
    impl<'ast> syn::visit::Visit<'ast> for Modules {
        fn visit_item_mod(&mut self, module: &'ast syn::ItemMod) {
            self.0 += usize::from(module.content.is_none());
            syn::visit::visit_item_mod(self, module);
        }
    }
    let mut modules = Modules::default();
    syn::visit::Visit::visit_file(
        &mut modules,
        &syn::parse_file(content).expect("invalid Rust source"),
    );
    assert_eq!(
        token_count(content.parse().expect("invalid Rust tokens")),
        modules.0,
        "module declaration inside a macro cannot be resolved"
    );
}

fn boundary_implicit_module(
    root: &std::path::Path,
    base: &std::path::Path,
    name: &str,
    required: bool,
) -> Vec<std::path::PathBuf> {
    let candidates = [format!("{name}.rs"), format!("{name}/mod.rs")]
        .map(|name| boundary_target(root, base, &name));
    let existing: Vec<_> = candidates.into_iter().filter(|p| p.exists()).collect();
    assert!(
        existing.len() == 1 || (!required && existing.is_empty()),
        "missing or ambiguous module {name} in {}",
        base.display()
    );
    existing
}

// No ignore/glob crate is locked. Read only checkout-local .gitignore files,
// supporting the simple patterns used here; reject unsupported syntax rather
// than silently widening exclusions. No git executable, metadata or global rules.
#[derive(Clone)]
struct BoundaryIgnore {
    base: std::path::PathBuf,
    pattern: regex::Regex,
    directory: bool,
    negated: bool,
}

fn boundary_ignore_rules(base: &std::path::Path, rules: &mut Vec<BoundaryIgnore>) {
    let path = base.join(".gitignore");
    let text = match std::fs::symlink_metadata(&path) {
        Ok(meta) => {
            assert!(meta.is_file() && !meta.is_symlink(), "invalid .gitignore");
            std::fs::read_to_string(path).unwrap()
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return,
        Err(error) => panic!(".gitignore: {error}"),
    };
    for line in text
        .lines()
        .filter(|l| !l.is_empty() && !l.starts_with('#'))
    {
        let (negated, line) = line.strip_prefix('!').map_or((false, line), |s| (true, s));
        assert!(
            !line.contains(['\\', '[', ']']) && !line.contains("**") && !line.ends_with(' '),
            "unsupported .gitignore pattern: {line}"
        );
        let directory = line.ends_with('/');
        let anchored = line.starts_with('/');
        let line = line.trim_matches('/');
        assert!(!line.is_empty(), "empty .gitignore pattern");
        let mut pattern = if anchored || line.contains('/') {
            "^"
        } else {
            "(?:^|/)"
        }
        .to_owned();
        for ch in line.chars() {
            match ch {
                '*' => pattern.push_str("[^/]*"),
                '?' => pattern.push_str("[^/]"),
                _ => pattern.push_str(&regex::escape(&ch.to_string())),
            }
        }
        pattern.push('$');
        rules.push(BoundaryIgnore {
            base: base.to_path_buf(),
            pattern: regex::Regex::new(&pattern).unwrap(),
            directory,
            negated,
        });
    }
}

fn boundary_ignored(path: &std::path::Path, directory: bool, rules: &[BoundaryIgnore]) -> bool {
    let mut ignored = false;
    for rule in rules {
        if (!rule.directory || directory)
            && path.strip_prefix(&rule.base).is_ok_and(|p| {
                rule.pattern
                    .is_match(p.to_str().expect("non-UTF8 source path"))
            })
        {
            ignored = !rule.negated;
        }
    }
    ignored
}

fn boundary_target(
    root: &std::path::Path,
    base: &std::path::Path,
    literal: &str,
) -> std::path::PathBuf {
    use std::path::Component;
    assert!(
        !std::path::Path::new(literal).is_absolute(),
        "absolute source path: {literal}"
    );
    let mut target = base.to_path_buf();
    for part in std::path::Path::new(literal).components() {
        match part {
            Component::CurDir => (),
            Component::ParentDir => {
                assert!(
                    target != root && target.starts_with(root),
                    "source path escapes repository: {literal}"
                );
                target.pop();
            }
            Component::Normal(name) => {
                target.push(name);
                if let Ok(metadata) = std::fs::symlink_metadata(&target) {
                    assert!(
                        !metadata.is_symlink(),
                        "symlink in source path: {}",
                        target.display()
                    );
                }
            }
            _ => panic!("invalid source path: {literal}"),
        }
    }
    target
}

fn boundary_references(content: &str) -> Vec<(String, bool)> {
    use proc_macro2::{TokenStream, TokenTree};
    fn walk(stream: TokenStream, references: &mut Vec<(String, bool)>) {
        let tokens: Vec<_> = stream.into_iter().collect();
        for (index, token) in tokens.iter().enumerate() {
            if let TokenTree::Ident(ident) = token {
                let name = ident.to_string();
                assert!(
                    name != "mod"
                        || !matches!(tokens.get(index + 1), Some(TokenTree::Punct(p)) if p.as_char() == '$'),
                    "macro-generated module cannot be resolved"
                );
                if matches!(name.as_str(), "include" | "include_str")
                    && matches!(tokens.get(index + 1), Some(TokenTree::Punct(p)) if p.as_char() == '!')
                {
                    let Some(TokenTree::Group(group)) = tokens.get(index + 2) else {
                        panic!("invalid include macro");
                    };
                    let path: syn::LitStr = syn::parse2(group.stream())
                        .expect("include must use a literal source path");
                    references.push((path.value(), name == "include"));
                }
            }
            if let TokenTree::Group(group) = token {
                walk(group.stream(), references);
            }
        }
    }
    let mut references = Vec::new();
    walk(
        content.parse().expect("invalid Rust tokens"),
        &mut references,
    );
    references
}

#[test]
fn boundary_fix10_literal_macro_modules_are_refused() {
    for source in [
        "macro_rules! m { () => { mod shim; } } m!();",
        "cfg_if! { if #[cfg(unix)] { mod shim; } }",
    ] {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        std::fs::create_dir(root.join("src")).unwrap();
        std::fs::write(root.join("Cargo.toml"), "").unwrap();
        std::fs::write(root.join("src/main.rs"), source).unwrap();
        std::fs::write(root.join("src/shim.rs"), "use mupdf;").unwrap();
        std::fs::write(root.join(".gitignore"), "/src/shim.rs\n").unwrap();
        let failure = std::panic::catch_unwind(|| boundary_files(root)).unwrap_err();
        let message = failure.downcast_ref::<String>().unwrap();
        assert!(message.contains("module declaration inside a macro cannot be resolved"));
    }
}

#[test]
fn boundary_fix10_path_module_uses_target_parent_without_decoy_base() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    std::fs::create_dir_all(root.join("src/gen/x")).unwrap();
    std::fs::write(root.join("Cargo.toml"), "").unwrap();
    std::fs::write(root.join("src/main.rs"), "#[path = \"gen/x.rs\"] mod x;").unwrap();
    std::fs::write(root.join("src/gen/x.rs"), "mod hidden;").unwrap();
    std::fs::write(root.join("src/gen/hidden.rs"), "use mupdf;").unwrap();
    std::fs::write(root.join("src/gen/x/hidden.rs"), "").unwrap();
    std::fs::write(root.join(".gitignore"), "/src/gen/hidden.rs\n").unwrap();
    let files = boundary_files(root);
    assert!(files.contains(&root.join("src/gen/hidden.rs")));
    assert!(native_rust_reference(
        &std::fs::read_to_string(root.join("src/gen/hidden.rs")).unwrap()
    ));
}

#[test]
fn boundary_fix10_include_inherits_module_bases() {
    for declaration in ["mod hidden;", "#[path = \"hidden.rs\"] mod hidden;"] {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        std::fs::create_dir_all(root.join("src/gen")).unwrap();
        std::fs::write(root.join("Cargo.toml"), "").unwrap();
        std::fs::write(root.join("src/main.rs"), "include!(\"gen/items.rs\");").unwrap();
        std::fs::write(root.join("src/gen/items.rs"), declaration).unwrap();
        std::fs::write(root.join("src/hidden.rs"), "use mupdf;").unwrap();
        std::fs::write(root.join(".gitignore"), "/src/hidden.rs\n").unwrap();
        assert!(boundary_files(root).contains(&root.join("src/hidden.rs")));
    }
}

#[test]
fn boundary_fix10_refuses_root_cargo_config() {
    for name in ["config", "config.toml"] {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        std::fs::create_dir(root.join("src")).unwrap();
        std::fs::create_dir(root.join(".cargo")).unwrap();
        std::fs::write(root.join("Cargo.toml"), "").unwrap();
        std::fs::write(root.join("src/main.rs"), "").unwrap();
        std::fs::write(root.join(".cargo").join(name), "paths = ['elsewhere']").unwrap();
        let failure = std::panic::catch_unwind(|| boundary_files(root)).unwrap_err();
        let message = failure.downcast_ref::<String>().unwrap();
        assert!(message.contains("unsupported Cargo source configuration"));
    }
}

#[test]
fn boundary_ignored_crate_roots_are_scanned() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    std::fs::create_dir(root.join("src")).unwrap();
    std::fs::write(root.join("Cargo.toml"), "[lib]\npath = 'src/custom.rs'\n").unwrap();
    for path in ["src/main.rs", "src/lib.rs", "src/custom.rs", "build.rs"] {
        std::fs::write(root.join(path), "").unwrap();
    }
    std::fs::write(root.join(".gitignore"), "/Cargo.toml\n/src/\n/build.rs\n").unwrap();
    let files = boundary_files(root);
    for path in [
        "Cargo.toml",
        "src/main.rs",
        "src/lib.rs",
        "src/custom.rs",
        "build.rs",
    ] {
        assert!(files.contains(&root.join(path)), "{path}");
    }
}

#[test]
fn boundary_module_resolution_follows_rust_directories() {
    for (entry, declaration, child) in [
        ("src/main.rs", "mod hidden;", "src/hidden.rs"),
        ("src/lib.rs", "mod hidden;", "src/hidden/mod.rs"),
        ("src/parent.rs", "mod hidden;", "src/parent/hidden.rs"),
        ("src/parent.rs", "mod hidden;", "src/parent/hidden/mod.rs"),
        ("src/main.rs", "mod a { mod hidden; }", "src/a/hidden.rs"),
        (
            "src/parent.rs",
            "mod a { mod hidden; }",
            "src/parent/a/hidden.rs",
        ),
        ("src/main.rs", "fn f() { mod hidden; }", "src/hidden.rs"),
        ("build.rs", "mod hidden;", "hidden.rs"),
        ("tests/entry.rs", "mod hidden;", "tests/hidden.rs"),
        ("examples/entry.rs", "mod hidden;", "examples/hidden.rs"),
        ("benches/entry.rs", "mod hidden;", "benches/hidden.rs"),
        ("src/bin/entry.rs", "mod hidden;", "src/bin/hidden.rs"),
        ("src/custom.rs", "mod hidden;", "src/hidden.rs"),
        (
            "src/main.rs",
            "mod a { #[path = \"hidden.rs\"] mod b; }",
            "src/a/hidden.rs",
        ),
    ] {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        for path in ["src/main.rs", entry, child] {
            std::fs::create_dir_all(root.join(path).parent().unwrap()).unwrap();
            std::fs::write(root.join(path), "").unwrap();
        }
        let manifest = if entry == "src/custom.rs" {
            "[lib]\npath = 'src/custom.rs'\n"
        } else {
            ""
        };
        std::fs::write(root.join("Cargo.toml"), manifest).unwrap();
        std::fs::write(root.join(".gitignore"), format!("/{child}\n")).unwrap();
        if entry == "src/parent.rs" {
            std::fs::write(root.join("src/main.rs"), "mod parent;").unwrap();
        }
        std::fs::write(root.join(entry), declaration).unwrap();
        assert!(
            boundary_files(root).contains(&root.join(child)),
            "{entry}: {declaration}"
        );
    }
}

#[test]
fn boundary_reachable_modules_cannot_be_missing_or_symlinks() {
    use std::os::unix::fs::symlink;
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    std::fs::create_dir(root.join("src")).unwrap();
    std::fs::write(root.join("Cargo.toml"), "").unwrap();
    std::fs::write(root.join("src/main.rs"), "mod hidden;").unwrap();
    std::fs::write(root.join(".gitignore"), "/src/hidden.rs\n").unwrap();
    assert!(std::panic::catch_unwind(|| boundary_files(root)).is_err());
    symlink("main.rs", root.join("src/hidden.rs")).unwrap();
    assert!(std::panic::catch_unwind(|| boundary_files(root)).is_err());
    std::fs::remove_file(root.join("src/hidden.rs")).unwrap();
    symlink("absent.rs", root.join("src/hidden.rs")).unwrap();
    assert!(std::panic::catch_unwind(|| boundary_files(root)).is_err());
}

#[test]
fn boundary_reachable_ignored_modules_are_scanned() {
    for (source, module, ignore) in [
        ("mod __pycache__;", "src/__pycache__/mod.rs", "__pycache__/"),
        ("mod shim;", "src/shim.rs", "/src/shim.rs"),
    ] {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        std::fs::create_dir_all(root.join(module).parent().unwrap()).unwrap();
        std::fs::write(root.join("Cargo.toml"), "").unwrap();
        std::fs::write(root.join("src/main.rs"), source).unwrap();
        std::fs::write(root.join(module), "use mupdf;").unwrap();
        std::fs::write(root.join(".gitignore"), ignore).unwrap();
        let paths = boundary_files(root);
        assert!(
            paths
                .iter()
                .any(|p| native_rust_reference(&std::fs::read_to_string(p).unwrap())),
            "reachable native source was hidden: {module}"
        );
    }
}

#[test]
fn boundary_conditional_path_also_scans_implicit_module() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    std::fs::create_dir(root.join("src")).unwrap();
    std::fs::write(root.join("Cargo.toml"), "").unwrap();
    std::fs::write(
        root.join("src/main.rs"),
        "#[cfg_attr(windows, path = \"alternate.rs\")] mod shim;",
    )
    .unwrap();
    std::fs::write(root.join("src/alternate.rs"), "").unwrap();
    std::fs::write(root.join("src/shim.rs"), "use mupdf;").unwrap();
    std::fs::write(root.join(".gitignore"), "/src/shim.rs\n").unwrap();
    assert!(boundary_files(root).contains(&root.join("src/shim.rs")));
}

#[test]
fn boundary_refuses_worker_cargo_roots() {
    for manifest in [
        "[lib]\npath = 'src/sources/worker/host_api.rs'\n",
        "[package]\nbuild = 'src/sources/worker/host_api.rs'\n",
        "[workspace]\nmembers = ['src/sources/worker']\n",
    ] {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        std::fs::create_dir_all(root.join("src/sources/worker")).unwrap();
        std::fs::write(root.join("src/main.rs"), "").unwrap();
        std::fs::write(root.join("Cargo.toml"), manifest).unwrap();
        std::fs::write(root.join("src/sources/worker/host_api.rs"), "").unwrap();
        std::fs::write(root.join("src/sources/worker/Cargo.toml"), "").unwrap();
        assert!(
            std::panic::catch_unwind(|| boundary_files(root)).is_err(),
            "{manifest}"
        );
    }
}

#[test]
fn boundary_path_dependencies_require_scanned_manifests() {
    for section in [
        "dependencies",
        "dev-dependencies",
        "build-dependencies",
        "target.'cfg(unix)'.dependencies",
        "target.'cfg(unix)'.dev-dependencies",
        "target.'cfg(unix)'.build-dependencies",
        "patch.crates-io",
        "replace",
        "workspace.dependencies",
    ] {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        std::fs::create_dir_all(root.join("src")).unwrap();
        std::fs::create_dir_all(root.join("outside")).unwrap();
        std::fs::write(root.join("src/main.rs"), "").unwrap();
        std::fs::write(root.join("outside/Cargo.toml"), "").unwrap();
        std::fs::write(
            root.join("Cargo.toml"),
            format!("[{section}]\nhelper = {{ path = 'outside' }}\n"),
        )
        .unwrap();
        assert!(
            std::panic::catch_unwind(|| boundary_files(root)).is_err(),
            "{section}"
        );
        std::fs::create_dir_all(root.join("tools/helper")).unwrap();
        std::fs::write(root.join("tools/helper/Cargo.toml"), "").unwrap();
        std::fs::write(
            root.join("Cargo.toml"),
            format!("[{section}]\nhelper = {{ path = 'tools/helper' }}\n"),
        )
        .unwrap();
        assert!(boundary_files(root).contains(&root.join("tools/helper/Cargo.toml")));
        std::fs::write(root.join(".gitignore"), "/tools/helper/\n").unwrap();
        assert!(
            std::panic::catch_unwind(|| boundary_files(root)).is_err(),
            "ignored dependency: {section}"
        );
    }
}

#[test]
fn boundary_refuses_macro_generated_modules() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    std::fs::create_dir(root.join("src")).unwrap();
    std::fs::write(root.join("Cargo.toml"), "").unwrap();
    std::fs::write(
        root.join("src/main.rs"),
        "macro_rules! source { ($name:ident) => { mod $name; }; } source!(shim);",
    )
    .unwrap();
    assert!(std::panic::catch_unwind(|| boundary_files(root)).is_err());
}

#[test]
fn boundary_walk_is_scoped_and_rejects_symlinks() {
    use std::os::unix::fs::{PermissionsExt, symlink};
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    let expected = [
        "Cargo.toml",
        "build.rs",
        "src/main.rs",
        "src/nested/Cargo.toml",
        "tests/test.rs",
        "examples/demo.rs",
        "tools/helper.rs",
        "tools/Cargo.toml",
        "benches/speed.rs",
    ];
    for name in expected
        .into_iter()
        .chain(["docs/ignored.rs", "stray.rs", "outside/hidden.rs"])
    {
        let path = root.join(name);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, "").unwrap();
    }
    let outside = root.join("outside");
    std::fs::set_permissions(&outside, std::fs::Permissions::from_mode(0o0)).unwrap();
    // Restore permissions even when the old walk panics, so TempDir can clean up.
    let walked = std::panic::catch_unwind(|| boundary_files(root));
    std::fs::set_permissions(outside, std::fs::Permissions::from_mode(0o700)).unwrap();
    let mut walked = walked.unwrap();
    walked.sort();
    let mut expected = expected.map(|name| root.join(name));
    expected.sort();
    assert_eq!(walked, expected);
    for (target, link) in [
        ("outside", "src/linked-dir"),
        ("stray.rs", "src/linked.rs"),
        ("missing", "tools/broken.rs"),
    ] {
        symlink(root.join(target), root.join(link)).unwrap();
        assert!(std::panic::catch_unwind(|| boundary_files(root)).is_err());
        std::fs::remove_file(root.join(link)).unwrap();
    }
}

#[test]
fn boundary_cargo_targets_are_scanned_and_cannot_escape() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    std::fs::create_dir(root.join("src")).unwrap();
    std::fs::write(root.join("src/main.rs"), "").unwrap();
    std::fs::create_dir(root.join("custom")).unwrap();
    for section in ["[lib]", "[[bench]]", "[[bin]]", "[[example]]", "[[test]]"] {
        std::fs::write(root.join("custom/target.rs"), "").unwrap();
        std::fs::write(
            root.join("Cargo.toml"),
            format!("{section}\npath = 'custom/target.rs'\n"),
        )
        .unwrap();
        assert!(std::panic::catch_unwind(|| boundary_files(root)).is_err());
        std::fs::write(
            root.join("Cargo.toml"),
            format!("{section}\npath = 'src/main.rs'\n"),
        )
        .unwrap();
        assert!(boundary_files(root).contains(&root.join("src/main.rs")));
        for escape in ["../outside.rs", "/outside.rs"] {
            std::fs::write(
                root.join("Cargo.toml"),
                format!("{section}\npath = '{escape}'\n"),
            )
            .unwrap();
            assert!(std::panic::catch_unwind(|| boundary_files(root)).is_err());
        }
    }
    std::fs::write(
        root.join("Cargo.toml"),
        "[package]\nbuild = 'custom/target.rs'\n",
    )
    .unwrap();
    assert!(std::panic::catch_unwind(|| boundary_files(root)).is_err());
}

#[test]
fn boundary_cargo_parser_handles_quoted_keys_and_inline_tables() {
    let dir = tempfile::tempdir().unwrap();
    let manifest = dir.path().join("Cargo.toml");
    std::fs::write(
        &manifest,
        r#"
"package" = { "build" = """build.rs""" }
"lib" = { "path" = 'src/lib.rs' }
"bin" = [{ "path" = 'src/main.rs' }]
"workspace" = { "members" = ['tools/helper'] }
[[example]]
"path" = 'examples/demo.rs'
"#,
    )
    .unwrap();
    assert_eq!(
        cargo_source_paths(&manifest),
        [
            ("build.rs", "build"),
            ("src/lib.rs", "target"),
            ("src/main.rs", "target"),
            ("examples/demo.rs", "target"),
            ("tools/helper", "member"),
        ]
        .map(|(path, kind)| (path.to_owned(), kind))
    );
}

#[test]
fn boundary_refuses_unwalked_cargo_children() {
    for manifest in [
        "[[bin]]\npath = 'custom/preview.rs'\n",
        "[workspace]\nmembers = ['custom']\n",
        "[workspace]\nmembers = ['custom/*']\n",
    ] {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        std::fs::create_dir(root.join("src")).unwrap();
        std::fs::create_dir(root.join("custom")).unwrap();
        std::fs::write(root.join("src/main.rs"), "").unwrap();
        std::fs::write(root.join("Cargo.toml"), manifest).unwrap();
        std::fs::write(root.join("custom/preview.rs"), "mod pdf;").unwrap();
        std::fs::write(root.join("custom/pdf.rs"), "use mupdf;").unwrap();
        std::fs::write(root.join("custom/Cargo.toml"), "").unwrap();
        assert!(
            std::panic::catch_unwind(|| boundary_files(root)).is_err(),
            "{manifest}"
        );
    }
}

#[test]
fn boundary_refuses_root_build_file_modules() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    std::fs::create_dir(root.join("src")).unwrap();
    std::fs::write(root.join("src/main.rs"), "").unwrap();
    std::fs::write(root.join("Cargo.toml"), "[package]\nbuild = 'build.rs'\n").unwrap();
    std::fs::write(root.join("build.rs"), "mod helper;").unwrap();
    std::fs::write(root.join("helper.rs"), "use mupdf;").unwrap();
    let files = boundary_files(root);
    assert!(files.contains(&root.join("helper.rs")));
    assert!(
        files
            .iter()
            .any(|path| native_rust_reference(&std::fs::read_to_string(path).unwrap()))
    );
}

#[test]
fn boundary_refuses_worker_sources_pulled_outside() {
    for text in [
        "#[path = \"sources/worker/operations.rs\"] mod bypass;",
        "include!(\"sources/worker/operations.rs\");",
    ] {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        std::fs::create_dir_all(root.join("src/sources/worker")).unwrap();
        std::fs::write(root.join("Cargo.toml"), "").unwrap();
        std::fs::write(root.join("src/main.rs"), text).unwrap();
        std::fs::write(root.join("src/sources/worker/operations.rs"), "").unwrap();
        assert!(
            std::panic::catch_unwind(|| boundary_files(root)).is_err(),
            "{text}"
        );
    }
}

#[test]
fn boundary_skips_ignored_local_content_without_git() {
    use std::os::unix::fs::symlink;
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    std::fs::create_dir_all(root.join("src")).unwrap();
    std::fs::create_dir_all(root.join("tools/.venv/bin")).unwrap();
    std::fs::create_dir_all(root.join("tools/__pycache__")).unwrap();
    std::fs::create_dir_all(root.join("tools/target")).unwrap();
    std::fs::write(root.join("Cargo.toml"), "").unwrap();
    std::fs::write(root.join("src/main.rs"), "").unwrap();
    std::fs::write(root.join(".gitignore"), ".venv*/\n__pycache__/\n/target/\n").unwrap();
    std::fs::write(
        root.join("tools/.gitignore"),
        "/target/\n*.tmp\n!kept.tmp\n",
    )
    .unwrap();
    for path in [
        "tools/.venv/bin/python",
        "tools/__pycache__/cache",
        "tools/target/link",
        "tools/local.tmp",
    ] {
        symlink("/missing", root.join(path)).unwrap();
    }
    assert_eq!(boundary_files(root).len(), 2);
    symlink("/missing", root.join("tools/kept.tmp")).unwrap();
    assert!(std::panic::catch_unwind(|| boundary_files(root)).is_err());
}

#[test]
fn boundary_rejects_hidden_rust_sources() {
    for text in [
        "#[path = \"../other/native.rs\"] mod bypass;",
        "include!(\"x.txt\");",
        "#[path = \"x.inc\"] mod bypass;",
        "#[cfg_attr(unix, path = r#\"../other/native.rs\"#)] mod bypass;",
        "include!(\"/outside.rs\");",
        "include_str!(\"../outside.txt\");",
        "include!(\"../Cargo.lock\");",
        "#[path = \"../Cargo.lock\"] mod bypass;",
    ] {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir(dir.path().join("src")).unwrap();
        std::fs::write(dir.path().join("src/main.rs"), text).unwrap();
        std::fs::write(dir.path().join("Cargo.toml"), "").unwrap();
        std::fs::write(dir.path().join("Cargo.lock"), "version = 4\n").unwrap();
        assert!(
            std::panic::catch_unwind(|| boundary_files(dir.path())).is_err(),
            "{text}"
        );
    }
}

#[test]
fn boundary_scans_included_text_and_requires_main() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    std::fs::write(root.join("Cargo.toml"), "").unwrap();
    assert!(std::panic::catch_unwind(|| boundary_files(root)).is_err());
    std::fs::create_dir(root.join("src")).unwrap();
    std::fs::write(
        root.join("src/main.rs"),
        "const TEXT: &str = include_str!(\"x.txt\");",
    )
    .unwrap();
    std::fs::write(root.join("src/x.txt"), "use mupdf;").unwrap();
    let paths = boundary_files(root);
    assert!(paths.contains(&root.join("src/x.txt")));
    assert!(
        paths
            .iter()
            .any(|p| native_rust_reference(&std::fs::read_to_string(p).unwrap()))
    );
    std::fs::write(root.join("Cargo.lock"), "version = 4\n").unwrap();
    std::fs::write(
        root.join("src/main.rs"),
        "const LOCK: &str = include_str!(\"../Cargo.lock\");",
    )
    .unwrap();
    assert!(boundary_files(root).contains(&root.join("Cargo.lock")));
    // Tokens in strings/comments are data, not source redirection.
    assert_eq!(
        boundary_references("// include!(\"x.txt\");\nconst X: &str = r#\"#[path = \"x.inc\"]\"#;"),
        Vec::<(String, bool)>::new()
    );
}

fn operations_reference() -> regex::Regex {
    regex::Regex::new(r"(?s)(::\s*operations\b|\boperations\s*::|\buse\b[^;]*\boperations\b|\bworker\s*::\s*(?:Input\b|\{[^;]*\bInput\b)|\b(?:inspect|render|render_each)\s*\(\s*(?:Input|operations\s*::\s*Input)\s*::)").unwrap()
}

fn native_reference() -> regex::Regex {
    regex::Regex::new(r#"(?s)(\bmupdf(?:_sys)?\b|#\s*\[\s*(?:path|cfg_attr\s*\([^;]*\bpath)\s*=\s*(?:r\#*)?"[^"]*(?:sources/worker|worker/)[^"]*"|\binclude\s*!\s*\([^;]*(?:sources/worker|worker/|\b(?:concat|env)\s*!)|\bpackage\s*=\s*["']mupdf(?:-sys)?["'])"#).unwrap()
}
fn cargo_alias_reference(text: &str) -> bool {
    let dependency = regex::Regex::new(r"^(?:mupdf|mupdf-sys)\s*=\s*\S").unwrap();
    // The package allowlist names the vendored fork's directory and inventory (DEP-29). Only
    // those exact entries, on the `include` line, are exempt; any other mention still counts.
    text.lines().any(|line| {
        let line = if line.starts_with("include = [") {
            line.replacen("\"/vendor/mupdf/**\", ", "", 1).replacen(
                "\"/vendor/mupdf.sha256\", ",
                "",
                1,
            )
        } else {
            line.to_owned()
        };
        line.to_ascii_lowercase().contains("mupdf") && !dependency.is_match(&line)
    })
}

fn native_rust_reference(text: &str) -> bool {
    if text == "\"mupdf\"" {
        return false;
    }
    // Only the explicit provenance field spellings are exempt. Keep whole-token
    // matching for native crate names, including comments/raw identifiers in imports.
    let provenance =
        regex::Regex::new(r#"\bmupdf\s*:\s*(?:String\b|"[^"]*"\.into\(\)|worker::MUPDF_VERSION\.into\(\))|\bdocument\.mupdf\b"#)
            .unwrap();
    native_reference().is_match(&provenance.replace_all(text, "provenance"))
}

#[test]
fn boundary_matchers_cover_aliases_and_imports() {
    assert!(cargo_alias_reference(
        "renamed = { package = \"mupdf\", version = \"0.8\" }"
    ));
    for text in [
        "renamed = { \"package\" = \"mupdf\", version = \"0.8\" }",
        "renamed = { package='mupdf', version = '0.8' }",
        "renamed = { package = 'MuPDF-sys', version = '0.8' }",
        "\"mupdf\" = '0.8'",
        "[dependencies.mupdf]",
        "dependencies = { mupdf = '0.8' }",
        "MuPDF = '0.8'",
        " mupdf = '0.8'",
        "# MuPDF dependency",
    ] {
        assert!(cargo_alias_reference(text), "{text}");
    }
    // Only the exact vendored-directory entry of the package allowlist is exempt.
    assert!(!cargo_alias_reference(
        "include = [\"/src/**\", \"/vendor/mupdf/**\", \"/tools/*.py\"]"
    ));
    for text in [
        "include = [\"/vendor/mupdf/**\", \"/mupdf/**\"]",
        "include = [\"/vendor/mupdf.sha256\", \"/vendor/mupdf.sha256\"]",
        "include = [\"/vendor/mupdf-sys/**\"]",
        "other = [\"/vendor/mupdf/**\"]",
    ] {
        assert!(cargo_alias_reference(text), "{text}");
    }
    for text in [
        "mupdf = '0.8'",
        "mupdf-sys = { version = '0.8' }",
        "[dependencies]\nother = '1'",
    ] {
        assert!(!cargo_alias_reference(text), "{text}");
    }
    for text in [
        "use mupdf;",
        "use mupdf as m;",
        "extern crate mupdf;",
        "extern crate mupdf as pdf;",
        "mupdf::Context",
        "::mupdf::Context",
        "use ::mupdf::{Context as C};",
        "use {mupdf as pdf};",
        "mupdf_sys::fz_context",
        "use mupdf_sys as native;",
        "extern crate mupdf_sys;",
        "use ::mupdf_sys::{fz_context};",
        "use /* c */ mupdf as x;",
        "extern crate r#mupdf as x;",
        "#[path = \"sources/worker/operations.rs\"] mod bypass;",
        "#[path = r\"sources/worker/operations.rs\"] mod bypass;",
        "#[path = r#\"worker/operations.rs\"#] mod bypass;",
        "#[cfg_attr(unix, path = \"worker/operations.rs\")] mod bypass;",
        "#[cfg_attr(unix, path = r#\"sources/worker/operations.rs\"#)] mod bypass;",
        "include!(\"worker/operations.rs\");",
        "include!(concat!(\"work\", \"er/operations.rs\"));",
        "include!(env!(\"NATIVE_SOURCE\"));",
        "renamed = { package = \"mupdf\", version = \"0.8\" }",
    ] {
        assert!(native_rust_reference(text), "{text}");
    }
    assert!(!native_rust_reference("mupdf_error"));
    let operations = operations_reference();
    for text in [
        "worker::{Input, render as r}",
        "operations::render_each(input)",
        "worker::operations as op",
        "use crate::sources::worker::{operations as pdf};",
        "use crate::sources::worker::operations::{inspect as i};",
        "crate::sources::worker::operations::render(input)",
        "render(Input::Bytes(bytes), &[1])",
    ] {
        assert!(operations.is_match(text), "{text}");
    }
    for text in [
        "worker::inspect(path)",
        "worker::inspect_cancellable(path, &cancel)",
        "worker::render(path, pages, &cancel, callback)",
        "use crate::sources::worker::{inspect, render, Cancel};",
        "worker::{Spec, records::read_inspection}",
        "pub mupdf: String",
        "mupdf: worker::MUPDF_VERSION.into()",
        "document.mupdf",
        "\"mupdf\"",
        "mupdf_error",
    ] {
        assert!(!native_rust_reference(text), "{text}");
        assert!(!operations.is_match(text), "{text}");
    }
}

// DEP-29: the worker builds against the vendored mupdf fork (`vendor/mupdf`), which adds the
// safe text-direction accessors. This fails to compile against crates.io mupdf 0.8.0.
#[test]
fn vendored_mupdf_fork_exposes_text_direction() {
    let bytes = fixture(0, [0, 0, 200, 300], 1.0, false);
    let page = mupdf::pdf::PdfDocument::from_bytes(&bytes)
        .unwrap()
        .load_pdf_page(0)
        .unwrap();
    let text = page.to_text_page(mupdf::TextPageFlags::empty()).unwrap();
    let mut chars = 0;
    for block in text.blocks() {
        let _ = block.structure();
        for line in block.lines() {
            let dir = line.dir();
            assert!((dir.x - 1.0).abs() < 1e-3 && dir.y.abs() < 1e-3, "{dir:?}");
            for ch in line.chars() {
                chars += 1;
                assert_eq!(ch.bidi() & 1, 0);
                assert!(!ch.is_rtl());
            }
        }
    }
    assert_eq!(chars, "Corner".len());
}

#[test]
fn boundary_allows_only_the_exact_vendored_mupdf_patch() {
    let layout = |root: &std::path::Path, manifest: &str, vendor: &str| {
        std::fs::create_dir_all(root.join("src")).unwrap();
        std::fs::create_dir_all(root.join(vendor)).unwrap();
        std::fs::write(root.join("src/main.rs"), "").unwrap();
        std::fs::write(root.join(vendor).join("Cargo.toml"), "").unwrap();
        std::fs::write(root.join("Cargo.toml"), manifest).unwrap();
    };
    let allowed = "[patch.crates-io]\nmupdf = { path = \"vendor/mupdf\" }\n";
    let dir = tempfile::tempdir().unwrap();
    layout(dir.path(), allowed, "vendor/mupdf");
    assert!(boundary_files(dir.path()).contains(&dir.path().join("Cargo.toml")));
    assert!(!cargo_alias_reference(allowed.lines().nth(1).unwrap()));
    for (manifest, vendor) in [
        // Other paths, registries, keys and crates are not allowed.
        (
            "[patch.crates-io]\nmupdf = { path = \"vendor/other\" }\n",
            "vendor/other",
        ),
        (
            "[patch.crates-io]\nmupdf = { path = \"vendor/mupdf/\" }\n",
            "vendor/mupdf",
        ),
        (
            "[patch.crates-io]\nmupdf = { path = \"./vendor/mupdf\" }\n",
            "vendor/mupdf",
        ),
        (
            "[patch.crates-io]\nmupdf = { path = \"vendor/mupdf\", features = [\"x\"] }\n",
            "vendor/mupdf",
        ),
        (
            "[patch.crates-io]\nmupdf = { path = \"vendor/mupdf\", package = \"other\" }\n",
            "vendor/mupdf",
        ),
        (
            "[patch.other]\nmupdf = { path = \"vendor/mupdf\" }\n",
            "vendor/mupdf",
        ),
        (
            "[patch.crates-io]\nmupdf-sys = { path = \"vendor/mupdf\" }\n",
            "vendor/mupdf",
        ),
        (
            "[patch.crates-io]\nother = { path = \"vendor/mupdf\" }\n",
            "vendor/mupdf",
        ),
        (
            "[patch.crates-io]\nmupdf = { path = \"vendor/mupdf\" }\nother = { path = \"vendor/other\" }\n",
            "vendor/mupdf",
        ),
        (
            "[dependencies]\nmupdf = { path = \"vendor/mupdf\" }\n",
            "vendor/mupdf",
        ),
        (
            "[replace]\n\"mupdf:0.8.0\" = { path = \"vendor/mupdf\" }\n",
            "vendor/mupdf",
        ),
    ] {
        let dir = tempfile::tempdir().unwrap();
        layout(dir.path(), manifest, vendor);
        let root = dir.path();
        assert!(
            std::panic::catch_unwind(|| boundary_files(root)).is_err(),
            "{manifest}"
        );
    }
    // The allowance is for the root manifest only, and the directory must exist.
    let dir = tempfile::tempdir().unwrap();
    layout(dir.path(), allowed, "vendor/mupdf");
    std::fs::remove_file(dir.path().join("vendor/mupdf/Cargo.toml")).unwrap();
    let root = dir.path();
    assert!(std::panic::catch_unwind(|| boundary_files(root)).is_err());
    let dir = tempfile::tempdir().unwrap();
    layout(
        dir.path(),
        "[workspace]\nmembers = ['tools/helper']\n",
        "vendor/mupdf",
    );
    std::fs::create_dir_all(dir.path().join("tools/helper")).unwrap();
    std::fs::write(dir.path().join("tools/helper/Cargo.toml"), allowed).unwrap();
    let root = dir.path();
    assert!(std::panic::catch_unwind(|| boundary_files(root)).is_err());
}
