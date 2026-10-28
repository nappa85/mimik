//! The module boundary of the downloads, checked by scanning the sources: only
//! `src/assistant/download/` uses the HTTP client crate, and nothing in it (tests included)
//! imports a sheet, database, campaign, account or web type.
use std::path::Path;

/// Paths a download source must not name (its rows come through the `Rows` trait, its specs
/// are typed).
const FORBIDDEN: [&str; 16] = [
    "crate::campaigns",
    "crate::auth",
    "crate::accounts",
    "crate::store",
    "crate::definitions",
    "crate::sources",
    "crate::catalogues",
    "crate::games",
    "crate::registry",
    "crate::presentation",
    "crate::web",
    "crate::network",
    "crate::Store",
    "rusqlite",
    "axum",
    "tower_sessions",
];

fn code_lines(text: &str) -> impl Iterator<Item = &str> {
    text.lines()
        .filter(|line| !line.trim_start().starts_with("//"))
}

fn scan(dir: &Path, inside: bool, found: &mut Vec<String>) {
    for entry in std::fs::read_dir(dir).unwrap() {
        let path = entry.unwrap().path();
        let name = path.file_name().unwrap().to_string_lossy().into_owned();
        if path.is_dir() {
            scan(&path, inside || name == "download", found);
        } else if path.extension().is_some_and(|extension| extension == "rs") {
            let text = std::fs::read_to_string(&path).unwrap();
            let uses_client = code_lines(&text).any(|line| line.contains("reqwest"));
            if uses_client && !inside {
                found.push(format!("{} uses the HTTP client crate", path.display()));
            }
            // This file names the forbidden paths itself.
            if inside && name != "structure_tests.rs" {
                for forbidden in FORBIDDEN {
                    if code_lines(&text).any(|line| line.contains(forbidden)) {
                        found.push(format!("{} names {forbidden}", path.display()));
                    }
                }
            }
        }
    }
}

#[test]
fn only_the_download_module_uses_the_http_client_and_it_imports_no_sheet_or_database_type() {
    let mut found = Vec::new();
    scan(
        &Path::new(env!("CARGO_MANIFEST_DIR")).join("src"),
        false,
        &mut found,
    );
    assert!(found.is_empty(), "{found:?}");
}

#[test]
fn the_scan_sees_test_files_and_the_forbidden_names() {
    // A tripwire for the scan itself: the download directory has test files, and they would be
    // caught.
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("src/assistant/download");
    let tests = std::fs::read_dir(dir)
        .unwrap()
        .filter_map(Result::ok)
        .filter(|entry| entry.file_name().to_string_lossy().ends_with("_tests.rs"))
        .count();
    assert!(tests >= 8, "{tests}");
    assert!(FORBIDDEN.contains(&"rusqlite") && FORBIDDEN.contains(&"crate::auth"));
}
