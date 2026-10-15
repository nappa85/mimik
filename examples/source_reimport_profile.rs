//! Owned synthetic workload, never a manual or a source-fidelity claim.
#[path = "../tests/support/source_fixture.rs"]
mod fixture;
use mimik::Store;
use std::{fs, path::PathBuf, time::Instant};

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let root = PathBuf::from(std::env::args().nth(1).ok_or("output directory required")?);
    fs::create_dir(&root)?;
    fixture::fixture_pair(&root)?;
    let pdf = root.join("original.pdf");
    let mut doc = lopdf::Document::load(&pdf)?;
    let stream = doc
        .objects
        .values_mut()
        .find_map(|object| {
            if let lopdf::Object::Stream(stream) = object {
                Some(stream)
            } else {
                None
            }
        })
        .ok_or("fixture stream missing")?;
    stream.set_content(vec![b' '; 128 * 1024 * 1024 - 4096]);
    doc.save(&pdf)?;
    drop(doc);
    let hash = fixture::hash(&fs::read(&pdf)?);
    let manifest = root.join("bundle/bundle.json");
    let mut data: serde_json::Value = serde_json::from_slice(&fs::read(&manifest)?)?;
    data["source_sha256"] = hash.into();
    fs::write(&manifest, serde_json::to_vec(&data)?)?;
    let store = Store::open(&root.join("profile.sqlite3"))?;
    let started = Instant::now();
    let id = store.import_source(&root.join("bundle"), &pdf).await?;
    println!(
        "pdf_bytes={} import_ms={:.1}",
        fs::metadata(&pdf)?.len(),
        started.elapsed().as_secs_f64() * 1000.0
    );
    let started = Instant::now();
    assert_eq!(store.import_source(&root.join("bundle"), &pdf).await?, id);
    println!(
        "reimport_ms={:.1}",
        started.elapsed().as_secs_f64() * 1000.0
    );
    let other = Store::open(&root.join("profile.sqlite3"))?;
    let bundle = root.join("bundle");
    let started = Instant::now();
    let (first, second) = tokio::join!(
        store.import_source(&bundle, &pdf),
        other.import_source(&bundle, &pdf)
    );
    assert_eq!(first?, id);
    assert_eq!(second?, id);
    println!(
        "concurrent_reimports_ms={:.1}",
        started.elapsed().as_secs_f64() * 1000.0
    );
    Ok(())
}
