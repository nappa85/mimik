//! Synthetic maximum mapping cardinality; no real source approval.
#[path = "../tests/support/source_fixture.rs"]
mod fixture;
use mimik::{Store, definitions::ApprovalRequest, registry::RegisterRuleset};
use serde_json::json;
use std::{fs, path::PathBuf, time::Instant};

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let root = PathBuf::from(std::env::args().nth(1).ok_or("output path required")?);
    fs::create_dir(&root)?;
    fixture::fixture_pair(&root)?;
    let bundle = root.join("unknown-bundle");
    let mut manifest: serde_json::Value =
        serde_json::from_slice(&fs::read(bundle.join("bundle.json"))?)?;
    let original = manifest["pages"][0]["units"][0].clone();
    let mut units = Vec::new();
    let mut nodes = Vec::new();
    for index in 0..1024 {
        let mut unit = original.clone();
        unit["id"] = format!("/page/0/Text/{index}").into();
        nodes.push(json!({"id":unit["id"],"block_type":"Text","polygon":unit["polygon"],"html":unit["html"],"children":null}));
        units.push(unit);
    }
    let mut raw: serde_json::Value =
        serde_json::from_slice(&fs::read(bundle.join("source.json"))?)?;
    raw["children"][0]["children"] = nodes.into();
    let raw = serde_json::to_vec(&raw)?;
    fs::write(bundle.join("source.json"), &raw)?;
    manifest["extraction_sha256"] = fixture::hash(&raw).into();
    manifest["pages"][0]["units"] = units.into();
    fs::write(bundle.join("bundle.json"), serde_json::to_vec(&manifest)?)?;
    let store = Store::open(&root.join("profile.sqlite3"))?;
    store
        .register_ruleset(RegisterRuleset {
            game_id: "synthetic-game".into(),
            game_label: "Synthetic game".into(),
            edition_id: "first".into(),
            edition_label: "First edition".into(),
        })
        .await?;
    store
        .import_source(&bundle, &root.join("original.pdf"))
        .await?;
    let source = store.sources().await?.remove(0);
    let page = store.source_page(source.pages[0].id).await?;
    let mut definition: serde_json::Value =
        serde_json::from_str(include_str!("../schemas/definition-example.json"))?;
    definition["sources"] =
        json!([{"source_id":source.id,"pdf_sha256":source.pdf_hash,"roles":["sheet"]}]);
    let mut fields = Vec::new();
    let mut names = Vec::new();
    for index in 0..256 {
        let mut field = definition["fields"][0].clone();
        let name = format!("field-{index}");
        field["id"] = name.clone().into();
        let regions: Vec<_> = page.units[index * 4..index * 4 + 4]
            .iter()
            .map(|unit| json!({"source_id":source.id,"page_id":page.id,"unit_id":unit.id}))
            .collect();
        field["mapping"] = json!({"state":"mapped","regions":regions});
        fields.push(field);
        names.push(name);
    }
    definition["fields"] = fields.into();
    definition["groups"][0]["fields"] = json!(names);
    let draft = store
        .save_definition_draft(serde_json::to_vec(&definition)?, 0)
        .await?;
    let started = Instant::now();
    let compare = store.compare_definition(draft.id, false).await?;
    println!(
        "references={} compare_ms={:.1} response_bytes={}",
        compare.original_region_urls.len(),
        started.elapsed().as_secs_f64() * 1000.0,
        serde_json::to_vec(&compare)?.len()
    );
    let started = Instant::now();
    let version = store
        .approve_definition(
            draft.id,
            false,
            ApprovalRequest {
                comparison_hash: compare.comparison_hash,
                actor: "synthetic profile".into(),
                rationale: "Synthetic evidence only".into(),
                originals_compared: true,
            },
        )
        .await?;
    println!(
        "approval_ms={:.1} status={:?}",
        started.elapsed().as_secs_f64() * 1000.0,
        version.wiring_status
    );
    Ok(())
}
