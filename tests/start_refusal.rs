//! A launch that owner setup refuses: how the binary reports it and what it starts first.
use std::os::unix::fs::PermissionsExt;
use std::process::Command;

fn private(path: &std::path::Path) {
    std::fs::create_dir_all(path).unwrap();
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o700)).unwrap();
}

/// Runs `mimik --loopback <args>` in a fresh folder with no database and returns the output
/// and the folder.
fn refused_launch(args: &[&str]) -> (std::process::Output, tempfile::TempDir) {
    let dir = tempfile::tempdir().unwrap();
    private(dir.path());
    let output = Command::new(env!("CARGO_BIN_EXE_mimik"))
        .current_dir(dir.path())
        .env_remove("MIMIK_DATABASE")
        .env_remove("MIMIK_EXTERNAL_URL")
        .arg("--loopback")
        .args(args)
        .output()
        .unwrap();
    (output, dir)
}

#[test]
fn the_refusal_prints_with_real_line_breaks_and_exits_with_status_1() {
    let (output, dir) = refused_launch(&[]);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert_eq!(output.status.code(), Some(1), "{stderr}");
    assert!(
        stderr.starts_with("Error: Owner setup is required"),
        "{stderr}"
    );
    assert!(stderr.lines().count() >= 3, "{stderr}");
    assert!(!stderr.contains("\\n"), "{stderr}");
    assert!(!stderr.contains('"'), "{stderr}");
    let database = std::path::absolute(dir.path().join("data/mimik.sqlite3")).unwrap();
    assert!(
        stderr.contains(&format!("for database {}.", database.display())),
        "{stderr}"
    );
}

#[test]
fn a_refused_launch_never_starts_the_assistant() {
    let dir = tempfile::tempdir().unwrap();
    private(dir.path());
    let runtime = dir.path().join("runtime");
    let models = dir.path().join("models");
    private(&runtime);
    private(&models);
    let (output, _launch) = refused_launch(&[
        "--assistant",
        "--assistant-runtime",
        runtime.to_str().unwrap(),
        "--models-dir",
        models.to_str().unwrap(),
    ]);
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert_eq!(output.status.code(), Some(1), "{stderr}");
    assert!(stderr.contains("Owner setup is required"), "{stderr}");
    assert!(!stdout.contains("Assistant on"), "{stdout}");
    assert!(!stderr.contains("assistant:"), "{stderr}");
}

#[test]
fn an_unreadable_source_file_names_the_operating_system_reason() {
    let dir = tempfile::tempdir().unwrap();
    private(dir.path());
    let output = Command::new(env!("CARGO_BIN_EXE_mimik"))
        .env_remove("MIMIK_DATABASE")
        .arg("--database")
        .arg(dir.path().join("cli.sqlite3"))
        .args([
            "import-definition-draft",
            "--expected-revision",
            "0",
            "--file",
        ])
        .arg(dir.path().join("missing.json"))
        .output()
        .unwrap();
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert_eq!(output.status.code(), Some(1), "{stderr}");
    assert!(
        stderr.contains("No such file or directory (os error 2)"),
        "{stderr}"
    );
    assert!(!stderr.contains("missing.json"), "{stderr}");
}

#[test]
fn a_subcommand_failure_prints_the_plain_message_not_the_debug_form() {
    let dir = tempfile::tempdir().unwrap();
    private(dir.path());
    let password = dir.path().join("short");
    std::fs::write(&password, "short").unwrap();
    std::fs::set_permissions(&password, std::fs::Permissions::from_mode(0o600)).unwrap();
    let output = Command::new(env!("CARGO_BIN_EXE_mimik"))
        .env_remove("MIMIK_DATABASE")
        .arg("--database")
        .arg(dir.path().join("cli.sqlite3"))
        .args(["verify-account-password", "--id", "1", "--password-file"])
        .arg(&password)
        .output()
        .unwrap();
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert_eq!(output.status.code(), Some(1), "{stderr}");
    assert!(
        stderr.starts_with("Error: Player password must contain 12"),
        "{stderr}"
    );
    assert!(!stderr.contains("Invalid("), "{stderr}");
    assert!(!stderr.contains('"'), "{stderr}");
}
