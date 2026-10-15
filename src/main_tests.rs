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
