#[path = "../tests/support/source_fixture.rs"]
mod support;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let path = std::env::args_os()
        .nth(1)
        .ok_or("Provide a fixture output directory")?;
    let root = std::path::PathBuf::from(path);
    std::fs::create_dir_all(&root)?;
    support::fixture_pair(&root)?;
    println!("Synthetic source smoke fixture and review bundle/report created.");
    Ok(())
}
