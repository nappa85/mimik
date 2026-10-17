use lopdf::{Document, Object, Stream, dictionary};
use serde_json::json;
use sha2::{Digest, Sha256};
use std::{
    fs,
    path::{Path, PathBuf},
};

#[must_use]
pub fn hash(bytes: &[u8]) -> String {
    hex::encode(Sha256::digest(bytes))
}

/// Generate synthetic inputs shared by regression tests and artifact smoke checks.
///
/// # Errors
/// Returns filesystem, PDF/image encoding or JSON errors.
pub fn fixture(root: &Path) -> Result<(PathBuf, PathBuf), Box<dyn std::error::Error>> {
    let bundle = root.join("bundle");
    fs::create_dir(&bundle)?;
    let pdf = root.join("original.pdf");
    let mut document = Document::with_version("1.5");
    let pages = document.new_object_id();
    let content = document.add_object(Stream::new(dictionary! {}, Vec::new()));
    let page = document.add_object(dictionary! {"Type" => "Page", "Parent" => pages, "MediaBox" => vec![0.into(),0.into(),200.into(),300.into()], "Contents" => content});
    document.objects.insert(
        pages,
        Object::Dictionary(
            dictionary! {"Type"=>"Pages", "Kids"=>vec![Object::Reference(page)], "Count"=>1},
        ),
    );
    let catalog = document.add_object(dictionary! {"Type"=>"Catalog", "Pages"=>pages});
    document.trailer.set("Root", catalog);
    document.save(&pdf)?;
    let mut encoder = png::Encoder::new(fs::File::create(bundle.join("page-1.png"))?, 20, 30);
    encoder.set_color(png::ColorType::Rgb);
    encoder.set_depth(png::BitDepth::Eight);
    encoder
        .write_header()?
        .write_image_data(&vec![255; 20 * 30 * 3])?;
    let polygon = json!([[10, 10], [100, 10], [100, 40], [10, 40]]);
    let html = "<p>Strength capacity +8</p><script>window.__synthetic_xss=true;alert(1)</script><img src='https://outside.example/image'>";
    let unit = json!({"id":"/page/0/Text/1", "kind":"Text", "polygon":polygon, "html":html});
    let raw = serde_json::to_vec(
        &json!({"children":[{"id":"/page/0/Page/0", "block_type":"Page", "polygon":[[0,0],[200,0],[200,300],[0,300]], "html":"", "children":[{"id":"/page/0/Text/1", "block_type":"Text", "polygon":polygon, "html":html, "children":null}]}]}),
    )?;
    fs::write(bundle.join("source.json"), &raw)?;
    let manifest = json!({"contract_version":1,"source_id":"synthetic-core","title":"Synthetic source","system_id":"mork-borg","edition":"fixture-edition","language":"en","source_sha256":hash(&fs::read(&pdf)?),"extraction_sha256":hash(&raw),"extractor":{"name":"fixture","version":"1","settings":{}},"renderer":{"name":"fixture","version":"1","settings":{}},"pages":[{"pdf_page":1,"width_points":200,"height_points":300,"image_sha256":hash(&fs::read(bundle.join("page-1.png"))?),"units":[unit]}]});
    fs::write(bundle.join("bundle.json"), serde_json::to_vec(&manifest)?)?;
    Ok((bundle, pdf))
}

/// Generate separate lookup-only and ready-adapter sources with independent identities.
///
/// # Errors
/// Returns fixture/JSON/filesystem failures.
pub fn fixture_pair(root: &Path) -> Result<(), Box<dyn std::error::Error>> {
    let (lookup, _) = fixture(root)?;
    let ready = root.join("adapter-bundle");
    fs::create_dir(&ready)?;
    for name in ["source.json", "page-1.png"] {
        fs::copy(lookup.join(name), ready.join(name))?;
    }
    let mut manifest: serde_json::Value =
        serde_json::from_slice(&fs::read(lookup.join("bundle.json"))?)?;
    manifest["source_id"] = json!("synthetic-mork-adapter");
    manifest["title"] = json!("Synthetic MÖRK BORG adapter materials");
    manifest["edition"] = json!(mimik::mork_borg::EDITION);
    fs::write(ready.join("bundle.json"), serde_json::to_vec(&manifest)?)?;
    let unknown = root.join("unknown-bundle");
    fs::create_dir(&unknown)?;
    for name in ["source.json", "page-1.png"] {
        fs::copy(lookup.join(name), unknown.join(name))?;
    }
    manifest["contract_version"] = json!(2);
    manifest["source_id"] = json!("synthetic-unknown-core");
    manifest["title"] = json!("Unknown-game synthetic source");
    manifest["system_id"] = json!("synthetic-game");
    manifest["edition_id"] = json!("first");
    manifest["edition"] = json!("First edition");
    fs::write(unknown.join("bundle.json"), serde_json::to_vec(&manifest)?)?;
    fs::write(
        root.join("definition.json"),
        include_bytes!("../../schemas/definition-example.json"),
    )?;
    fs::write(
        root.join("formula-definition.json"),
        serde_json::to_vec(&formula_definition())?,
    )?;
    fs::write(
        root.join("list-definition.json"),
        serde_json::to_vec(&list_definition())?,
    )?;
    Ok(())
}

/// List definition for the unpacked browser gate (M4): an equipment list with a derived
/// `load` sum, a formula reading `item.id`, the advisory check `load <= 10`, and a second
/// list of 32 properties of every control so the 390 px layout is exercised.
#[must_use]
pub fn list_definition() -> serde_json::Value {
    let unresolved = json!({"state": "unresolved", "reason": "Synthetic fixture"});
    let property = |id: &str, label: &str, control: &str, optional: bool| json!({"id": id, "label": label, "control": control, "optional": optional});
    let mut kind = property("kind", "Kind", "enum", true);
    kind["options"] = json!([{"id": "light", "label": "Light"}, {"id": "heavy", "label": "Heavy"}]);
    let equipment = vec![
        property("name", "Item name", "text", false),
        kind,
        property("quantity", "Quantity", "integer", false),
        property("weight", "Weight", "integer", false),
        property("equipped", "Equipped", "boolean", false),
        property("note", "Item note", "multiline", true),
    ];
    let controls = ["text", "integer", "boolean", "enum", "multiline"];
    let traits: Vec<serde_json::Value> = (0..32)
        .map(|n| {
            let control = controls[n % controls.len()];
            let mut value = property(
                &format!("t{n:02}"),
                &format!("Trait property {n}"),
                control,
                n % 3 == 0,
            );
            if control == "enum" {
                value["options"] = json!([{"id": "a", "label": "Option A"},
                                          {"id": "b", "label": "Option B"}]);
            }
            value
        })
        .collect();
    let list = |id: &str, label: &str, properties: Vec<serde_json::Value>| {
        json!({"id": id, "label": label, "control": "list", "nullable": false,
               "mapping": unresolved, "items": {"properties": properties}})
    };
    let derived = |id: &str, label: &str, expr: &str| {
        json!({"id": id, "label": label, "control": "integer", "nullable": false,
               "mapping": unresolved,
               "formula": {"expr": expr, "rule_ids": ["synthetic-1"], "evidence": unresolved}})
    };
    let fields = vec![
        json!({"id": "name", "label": "Name", "control": "text", "nullable": false,
               "mapping": unresolved}),
        list("equipment", "Equipment", equipment),
        list("traits", "Traits", traits),
        derived(
            "load",
            "Load",
            "sum(equipment.map(item, item.quantity * item.weight))",
        ),
        derived(
            "first_kept",
            "First row kept",
            "size(equipment.filter(item, item.id == \"r1-1\"))",
        ),
    ];
    let ids: Vec<serde_json::Value> = fields.iter().map(|field| field["id"].clone()).collect();
    json!({
        "contract_version": 1, "game_id": "synthetic-game", "edition_id": "first",
        "definition_id": "list-sheet", "title": "Synthetic list sheet", "sources": [],
        "fields": fields,
        "groups": [{"id": "main", "label": "Main", "fields": ids, "column_hint": "main"}],
        "formula_profile": 1,
        "constraints": [{"id": "load-cap", "expr": "load <= 10", "rule_ids": ["synthetic-2"],
                         "message": "Load is at most 10.", "evidence": unresolved}]
    })
}

/// Scalar, list-free formula definition for the unpacked browser gate (lists wait for M4).
/// It has a hyphenated unreferenced field, a long text echoed by a derived value and an
/// unbroken 1,024-byte expression, so reflow and projection are checked in a real browser.
#[must_use]
pub fn formula_definition() -> serde_json::Value {
    let unresolved = json!({"state": "unresolved", "reason": "Synthetic fixture"});
    let field = |id: &str, label: &str, control: &str, nullable: bool| {
        json!({"id": id, "label": label, "control": control, "nullable": nullable,
               "mapping": unresolved})
    };
    let derived = |id: &str, label: &str, control: &str, expr: &str| {
        let mut value = field(id, label, control, false);
        value["formula"] = json!({"expr": expr, "rule_ids": ["synthetic-1"],
                                  "evidence": unresolved});
        value
    };
    let head = "name==\"";
    let tail = "\"?1:0";
    let marker = format!("{head}{}{tail}", "x".repeat(1024 - head.len() - tail.len()));
    let fields = vec![
        field("name", "Name", "text", false),
        field("vitality", "Vitality", "integer", false),
        field("hit-points", "Hit points", "integer", true),
        field("story", "Story", "multiline", false),
        derived("pace", "Pace", "integer", "6 - vitality"),
        derived("echo", "Story echo", "multiline", "story"),
        derived("marker", "Marker", "integer", &marker),
    ];
    let ids: Vec<serde_json::Value> = fields.iter().map(|field| field["id"].clone()).collect();
    json!({
        "contract_version": 1, "game_id": "synthetic-game", "edition_id": "first",
        "definition_id": "formula-sheet", "title": "Synthetic formula sheet", "sources": [],
        "fields": fields,
        "groups": [{"id": "main", "label": "Main", "fields": ids, "column_hint": "main"}],
        "formula_profile": 1,
        "constraints": [{"id": "vitality-cap", "expr": "vitality <= 10",
                         "rule_ids": ["synthetic-2"], "message": "Vitality is at most 10.",
                         "evidence": unresolved}]
    })
}
