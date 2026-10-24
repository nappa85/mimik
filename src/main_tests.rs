use super::*;
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

#[test]
fn the_data_folders_mimik_makes_are_private_whatever_the_umask() {
    use std::os::unix::fs::PermissionsExt;
    let root = tempfile::tempdir().unwrap();
    let existing = root.path().join("existing");
    std::fs::create_dir(&existing).unwrap();
    std::fs::set_permissions(&existing, std::fs::Permissions::from_mode(0o755)).unwrap();
    let data = existing.join("data/nested");
    create_data_dir(&data).unwrap();
    let mode =
        |path: &std::path::Path| std::fs::metadata(path).unwrap().permissions().mode() & 0o7777;
    // Explicit 0700 (a umask can only remove bits from it, so 002 leaves it as it is).
    assert_eq!(mode(&data), 0o700);
    assert_eq!(mode(&existing.join("data")), 0o700);
    // A folder that was there is the owner's to set.
    assert_eq!(mode(&existing), 0o755);
    create_data_dir(&data).unwrap();
}
