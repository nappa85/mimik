use super::*;
#[test]
fn synthetic_upload_worker_flag_is_development_only() {
    let parsed = Args::try_parse_from(["mimik", "--upload-dev-worker-ms", "100"]);
    #[cfg(all(target_os = "linux", feature = "dev-endpoint"))]
    assert_eq!(parsed.unwrap().upload_dev_worker_ms, Some(100));
    #[cfg(not(all(target_os = "linux", feature = "dev-endpoint")))]
    assert!(parsed.is_err());
}
#[test]
fn authentication_and_database_defaults_need_no_flags() {
    let normal = Args::try_parse_from(["mimik"]).unwrap();
    assert!(!normal.no_auth);
    assert!(!normal.loopback);
    assert!(normal.command.is_none());
    assert_eq!(
        database_path(None, None),
        std::path::PathBuf::from("data/mimik.sqlite3")
    );
    assert_eq!(
        database_path(None, Some("other.sqlite3".into())),
        std::path::PathBuf::from("other.sqlite3")
    );
    assert_eq!(
        database_path(
            Some("explicit.sqlite3".into()),
            Some("other.sqlite3".into())
        ),
        std::path::PathBuf::from("explicit.sqlite3")
    );
    for flags in [["mimik", "--loopback"], ["mimik", "--no-auth"]] {
        let args = Args::try_parse_from(flags).unwrap();
        assert!(matches!(
            server_options(&args, None).unwrap().network,
            mimik::network::Network::Loopback
        ));
    }
}
#[test]
fn removed_auth_and_conflicting_modes_are_rejected() {
    for flags in [
        vec!["mimik", "--auth"],
        vec!["mimik", "--no-auth", "--lan-address", "192.168.1.2"],
        vec![
            "mimik",
            "--no-auth",
            "--external-url",
            "https://mimik.example",
        ],
        vec!["mimik", "--no-auth", "--dev-owner-play"],
        vec!["mimik", "--loopback", "--lan-address", "192.168.1.2"],
    ] {
        assert!(Args::try_parse_from(flags).is_err());
    }
    let no_auth = Args::try_parse_from(["mimik", "--no-auth"]).unwrap();
    assert!(server_options(&no_auth, Some("https://mimik.example".into())).is_err());
}
#[test]
fn local_commands_ignore_server_configuration_and_reject_serving_flags() {
    let operator = Args::try_parse_from(["mimik", "list-accounts"]).unwrap();
    validate_serving_args(&operator).unwrap();
    let options = server_options(&operator, Some("malformed external URL".into())).unwrap();
    assert!(options.external_url.is_none());
    assert!(matches!(options.network, mimik::network::Network::Loopback));
    for flags in [
        ["mimik", "--loopback", "list-accounts"],
        ["mimik", "--no-auth", "list-accounts"],
    ] {
        let args = Args::try_parse_from(flags).unwrap();
        assert!(validate_serving_args(&args).is_err());
    }
}

/// The folders `create_data_dir` makes, in a process whose umask is permissive (Ubuntu's 002):
/// the parent starts this very test binary with that umask, so the check does not depend on the
/// umask of the run, and fails without explicit modes (the old `create_dir_all` gave 0775).
#[test]
fn the_data_folders_mimik_makes_are_private_under_a_permissive_umask() {
    use std::os::unix::process::CommandExt;
    let root = tempfile::tempdir().unwrap();
    let mut command = std::process::Command::new(std::env::current_exe().unwrap());
    command
        .args([
            "--exact",
            "tests::data_folder_child",
            "--nocapture",
            "--test-threads=1",
        ])
        .env("MIMIK_TEST_DATA_ROOT", root.path());
    // SAFETY: `umask` is async-signal-safe and touches no memory.
    unsafe {
        command.pre_exec(|| {
            libc::umask(0o002);
            Ok(())
        });
    }
    let output = command.output().unwrap();
    assert!(
        output.status.success(),
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    // The child really ran its checks (a filtered-out child also exits 0).
    assert!(String::from_utf8_lossy(&output.stdout).contains("data folders checked"));
}

#[test]
fn data_folder_child() {
    use std::os::unix::fs::PermissionsExt;
    let Some(root) = std::env::var_os("MIMIK_TEST_DATA_ROOT") else {
        return;
    };
    let existing = std::path::PathBuf::from(root).join("existing");
    std::fs::create_dir(&existing).unwrap();
    std::fs::set_permissions(&existing, std::fs::Permissions::from_mode(0o775)).unwrap();
    let data = existing.join("data/nested");
    create_data_dir(&data).unwrap();
    let mode =
        |path: &std::path::Path| std::fs::metadata(path).unwrap().permissions().mode() & 0o7777;
    assert_eq!(mode(&data), 0o700);
    assert_eq!(mode(&existing.join("data")), 0o700);
    // A folder that was there is the owner's to set.
    assert_eq!(mode(&existing), 0o775);
    create_data_dir(&data).unwrap();
    println!("data folders checked");
}
#[test]
fn import_source_points_to_the_owner_review_screen() {
    let message = import_source_message(7);
    assert!(message.contains("/admin/materials"), "{message}");
    assert!(!message.contains("Open /sources"), "{message}");
}

#[test]
fn import_words_message_says_what_happened_to_the_word_boxes() {
    use mimik::sources::WordsReport;
    let said = |stored, missing, size_mismatch, unavailable: Option<&str>| {
        import_words_message(&WordsReport {
            stored,
            missing,
            size_mismatch,
            unavailable: unavailable.map(str::to_owned),
        })
    };
    assert!(said(3, 0, 0, None).contains("stored for 3 page(s)"));
    assert!(said(0, 0, 0, None).contains("already has them"));
    assert!(said(2, 1, 0, None).contains("1 page(s) have none"));
    let message = said(0, 4, 0, Some("sandbox step unshare failed"));
    assert!(message.contains("not read") && message.contains("sandbox step unshare failed"));
    assert!(message.contains("4 page(s) have none"), "{message}");
    // Some pages stored, then the worker went away.
    let partial = said(2, 3, 0, Some("process cleanup timed out"));
    assert!(
        partial.contains("stored for 2 page(s), then the worker became unavailable"),
        "{partial}"
    );
    // Size mismatches are counted apart: running again cannot help.
    let sized = said(1, 0, 2, None);
    assert!(
        sized.contains("stored for 1 page(s).") && sized.contains("2 page(s) differ in size"),
        "{sized}"
    );
    assert!(!sized.contains("Run import-source again"), "{sized}");
    assert!(said(0, 0, 1, None).contains("1 page(s) differ in size"));
}
