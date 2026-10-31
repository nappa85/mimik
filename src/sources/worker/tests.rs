use super::*;
use lopdf::{Document, Object, Stream, dictionary};

fn fixture(rotate: i32, crop: [i32; 4], unit: f32, inherited: bool) -> Vec<u8> {
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
    for (rotate, crop, unit, inherited) in [
        (0, [0, 0, 200, 300], 1.0, false),
        (90, [0, 0, 200, 300], 1.0, false),
        (180, [0, 0, 200, 300], 1.0, false),
        (270, [0, 0, 200, 300], 1.0, false),
        (0, [20, 30, 180, 280], 1.0, false),
        (0, [-100, -100, 400, 500], 1.0, false),
        (0, [0, 0, 200, 300], 2.0, false),
        (90, [20, 30, 180, 280], 2.0, true),
    ] {
        let bytes = fixture(rotate, crop, unit, inherited);
        let inspection = inspect(Input::Bytes(&bytes)).unwrap();
        let info = &inspection.pages[0];
        let page = render(Input::Bytes(&bytes), &[1]).unwrap().remove(0);
        assert!(page.lines.iter().any(|l| l.text == "Corner"));
        let b = geometry::transformed_box([35.0, 45.0, 45.0, 55.0], info.transform);
        let x = (b[0].midpoint(b[2]) / info.width * f64::from(page.width_px)).floor();
        let y = (b[1].midpoint(b[3]) / info.height * f64::from(page.height_px)).floor();
        let mut decoder = png::Decoder::new(std::io::Cursor::new(&page.png))
            .read_info()
            .unwrap();
        let mut pixels = vec![0; decoder.output_buffer_size().unwrap()];
        decoder.next_frame(&mut pixels).unwrap();
        let x: usize = x.to_string().parse().unwrap();
        let y: usize = y.to_string().parse().unwrap();
        let offset = (y * usize::try_from(page.width_px).unwrap() + x) * 3;
        assert!(
            pixels[offset..offset + 3].iter().all(|&c| c < 20),
            "rotate={rotate} crop={crop:?}"
        );
        let text = &page.lines[0].bounds;
        let square_corner = ((b[0] + b[2]) < info.width, (b[1] + b[3]) < info.height);
        assert_eq!(
            square_corner,
            (
                (text[0] + text[2]) < info.width,
                (text[1] + text[3]) < info.height
            )
        );
        if rotate == 0 {
            compare_unrotated_sizes(&bytes, info);
        }
        let mut wire = Vec::new();
        let mut writer = records::RecordWriter::new(&mut wire, records::Operation::Render, 1);
        writer.rendered(&page).unwrap();
        writer.finish().unwrap();
        let mut reader = records::RenderReader::new(&wire[..], vec![info.clone()]).unwrap();
        assert_eq!(reader.next_page().unwrap().unwrap().lines[0].text, "Corner");
        assert!(reader.next_page().unwrap().is_none());
    }
}

fn compare_unrotated_sizes(bytes: &[u8], info: &Page) {
    let doc = Document::load_mem(bytes).unwrap();
    let page = doc.get_dictionary(doc.get_pages()[&1]).unwrap();
    let parent = doc
        .get_dictionary(page.get(b"Parent").unwrap().as_reference().unwrap())
        .unwrap();
    let read_box = |key: &[u8]| {
        page.get(key)
            .or_else(|_| parent.get(key))
            .unwrap()
            .as_array()
            .unwrap()
            .iter()
            .map(|o| f64::from(o.as_float().unwrap()))
            .collect::<Vec<_>>()
    };
    let crop = read_box(b"CropBox");
    let media = read_box(b"MediaBox");
    let unit = f64::from(page.get(b"UserUnit").unwrap().as_float().unwrap());
    let width = (crop[2].min(media[2]) - crop[0].max(media[0])) * unit;
    let height = (crop[3].min(media[3]) - crop[1].max(media[1])) * unit;
    let native = mupdf::pdf::PdfDocument::from_bytes(bytes)
        .unwrap()
        .load_pdf_page(0)
        .unwrap()
        .bounds()
        .unwrap();
    assert_eq!((info.width, info.height), (width, height));
    assert_eq!(
        (f64::from(native.width()), f64::from(native.height())),
        (width, height)
    );
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
        for entry in std::fs::read_dir(path).unwrap() {
            let path = entry.unwrap().path();
            if path.ends_with("sources/worker") {
                continue;
            }
            if path.is_dir() {
                visit(&path);
            } else {
                let content = std::fs::read_to_string(&path).unwrap();
                assert!(
                    !content.contains(concat!("mupdf", "::")),
                    "{}",
                    path.display()
                );
            }
        }
    }
    visit(&std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src"));
}
