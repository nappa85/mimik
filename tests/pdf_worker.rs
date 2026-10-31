#![cfg(target_os = "linux")]
use lopdf::{Document, Object, dictionary};
use mimik::sources::worker::{
    RenderSettings, Spec,
    records::{self, Operation},
};

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
