#![cfg(target_os = "linux")]
use lopdf::{Document, Object, dictionary};
use mimik::sources::worker::{
    Error, RenderSettings, Spec,
    records::{self, Operation},
};

fn run_pdf(pdf: &mut Document, operation: Operation) -> std::process::Output {
    let dir = tempfile::tempdir().unwrap();
    let input = dir.path().join("synthetic.pdf");
    pdf.save(&input).unwrap();
    let spec = Spec {
        operation,
        pages: if operation == Operation::Render {
            vec![1]
        } else {
            vec![]
        },
        settings: RenderSettings::default(),
        input,
    };
    let path = dir.path().join("spec.json");
    std::fs::write(&path, serde_json::to_vec(&spec).unwrap()).unwrap();
    std::process::Command::new(env!("CARGO_BIN_EXE_mimik"))
        .args(["pdf-worker", "--spec"])
        .arg(path)
        .output()
        .unwrap()
}

fn simple_pdf() -> Document {
    let mut pdf = Document::with_version("1.7");
    let pages = pdf.new_object_id();
    let page = pdf.add_object(dictionary!{"Type"=>"Page", "Parent"=>pages, "MediaBox"=>vec![0.into(),0.into(),100.into(),200.into()]});
    pdf.objects.insert(
        pages,
        Object::Dictionary(dictionary! {"Type"=>"Pages", "Count"=>1, "Kids"=>vec![page.into()]}),
    );
    let root = pdf.add_object(dictionary! {"Type"=>"Catalog", "Pages"=>pages});
    pdf.trailer.set("Root", root);
    pdf
}

#[test]
fn password_and_page_limit_have_typed_terminal_reasons() {
    for operation in [Operation::Inspect, Operation::Render] {
        for reason in [Error::PasswordProtected, Error::TooManyPages] {
            let mut pdf = simple_pdf();
            if reason == Error::PasswordProtected {
                pdf.trailer.set(
                    "ID",
                    Object::Array(vec![
                        Object::string_literal("dummy-id-00000001"),
                        Object::string_literal("dummy-id-00000001"),
                    ]),
                );
                let encryption = lopdf::EncryptionState::try_from(lopdf::EncryptionVersion::V2 {
                    document: &pdf,
                    owner_password: "dummy-owner",
                    user_password: "dummy-user",
                    key_length: 128,
                    permissions: lopdf::Permissions::all(),
                })
                .unwrap();
                pdf.encrypt(&encryption).unwrap();
            } else {
                let root = pdf.trailer.get(b"Root").unwrap().as_reference().unwrap();
                let pages = pdf
                    .get_dictionary(root)
                    .unwrap()
                    .get(b"Pages")
                    .unwrap()
                    .as_reference()
                    .unwrap();
                pdf.get_dictionary_mut(pages).unwrap().set("Count", 2001);
            }
            let output = run_pdf(&mut pdf, operation);
            assert_eq!(output.status.code(), Some(reason.exit_code()));
            assert_eq!(
                records::read_inspection(&output.stdout[..]).unwrap_err(),
                reason
            );
            assert!(!output.stdout.windows(6).any(|w| w == b"\"done\""));
        }
    }
}

#[test]
fn preframing_errors_have_distinct_statuses() {
    let output = std::process::Command::new(env!("CARGO_BIN_EXE_mimik"))
        .arg("pdf-worker")
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(Error::Unsupported.exit_code()));
    assert_eq!(output.stdout, [] as [u8; 0]);
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("spec.json");
    for (bytes, reason) in [
        (b"invalid".to_vec(), Error::Unreadable),
        (vec![b'x'; 65537], Error::TooLarge),
    ] {
        std::fs::write(&path, bytes).unwrap();
        let output = std::process::Command::new(env!("CARGO_BIN_EXE_mimik"))
            .args(["pdf-worker", "--spec"])
            .arg(&path)
            .output()
            .unwrap();
        assert_eq!(output.status.code(), Some(reason.exit_code()));
        assert_eq!(output.stdout, [] as [u8; 0]);
    }
}

#[test]
fn built_worker_inspects_and_renders_over_stdout() {
    let dir = tempfile::tempdir().unwrap();
    let input = dir.path().join("synthetic.pdf");
    let mut pdf = Document::with_version("1.7");
    let pages = pdf.new_object_id();
    let page=pdf.add_object(dictionary!{"Type"=>"Page","Parent"=>pages,"MediaBox"=>vec![0.into(),0.into(),100.into(),200.into()]});
    pdf.objects.insert(
        pages,
        Object::Dictionary(dictionary! {"Type"=>"Pages","Count"=>1,"Kids"=>vec![page.into()]}),
    );
    let root = pdf.add_object(dictionary! {"Type"=>"Catalog","Pages"=>pages});
    pdf.trailer.set("Root", root);
    pdf.save(&input).unwrap();
    let run = |operation, pages| {
        let spec = Spec {
            operation,
            pages,
            settings: RenderSettings::default(),
            input: input.clone(),
        };
        let path = dir.path().join("spec.json");
        std::fs::write(&path, serde_json::to_vec(&spec).unwrap()).unwrap();
        let result = std::process::Command::new(env!("CARGO_BIN_EXE_mimik"))
            .args(["pdf-worker", "--spec"])
            .arg(path)
            .output()
            .unwrap();
        assert!(
            result.status.success(),
            "worker failed: {:?}",
            result.status
        );
        result.stdout
    };
    let inspection = records::read_inspection(&run(Operation::Inspect, vec![])[..]).unwrap();
    assert_eq!(inspection.document.pages, 1);
    let output = run(Operation::Render, vec![1]);
    let mut reader = records::RenderReader::new(&output[..], inspection.pages).unwrap();
    assert!(reader.next_page().unwrap().is_some());
    assert!(reader.next_page().unwrap().is_none());
}
