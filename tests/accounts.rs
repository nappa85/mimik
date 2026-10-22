use mimik::{Store, accounts::Accounts};
use std::{
    fs::OpenOptions,
    io::Write,
    os::unix::fs::{OpenOptionsExt, PermissionsExt},
};

#[tokio::test]
async fn account_lifecycle_preserves_epoch_and_one_use_contract() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::set_permissions(dir.path(), std::fs::Permissions::from_mode(0o700)).unwrap();
    let db_path = dir.path().join("accounts.sqlite3");
    let store = Store::open(&db_path).unwrap();
    let accounts = Accounts::new(store);
    let password = dir.path().join("password");
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(&password)
        .unwrap();
    file.write_all(b"synthetic password 123").unwrap();
    assert!(accounts.list(0).await.is_err());
    let owner = accounts
        .bootstrap("owner".into(), "Local Owner".into(), &password)
        .await
        .unwrap();
    assert!(owner.owner);
    assert_eq!(owner.epoch, 1);
    assert!(
        accounts
            .bootstrap("other".into(), "Other".into(), &password)
            .await
            .is_err()
    );
    assert!(accounts.verify_password(owner.id, &password).await.unwrap());
    let token = dir.path().join("invite");
    let reserved = accounts
        .invite("player".into(), "Player".into(), &token)
        .await
        .unwrap();
    assert_eq!(reserved.state, "pending");
    let enrolled = accounts.enroll(&token, &password).await.unwrap();
    assert_eq!(enrolled.epoch, 2);
    assert!(!enrolled.owner);
    assert!(accounts.enroll(&token, &password).await.is_err());
    assert!(
        accounts
            .invite(
                "player".into(),
                "Player".into(),
                &dir.path().join("invalid")
            )
            .await
            .is_err()
    );
    assert!(
        accounts
            .issue_reset(enrolled.id, 1, &dir.path().join("stale"))
            .await
            .is_err()
    );
    let reset = dir.path().join("reset");
    let issued = accounts.issue_reset(enrolled.id, 2, &reset).await.unwrap();
    assert_eq!(issued.epoch, 3);
    assert!(
        accounts
            .verify_password(enrolled.id, &password)
            .await
            .unwrap()
    );
    assert!(accounts.enroll(&reset, &password).await.is_err());
    let changed = accounts.complete_reset(&reset, &password).await.unwrap();
    assert_eq!(changed.epoch, 4);
    assert!(accounts.complete_reset(&reset, &password).await.is_err());
    let disabled = accounts.disable(enrolled.id, 4).await.unwrap();
    assert_eq!(disabled.epoch, 5);
    assert!(
        !accounts
            .verify_password(enrolled.id, &password)
            .await
            .unwrap()
    );
    assert!(accounts.disable(enrolled.id, 5).await.is_err());
    let reopened = Accounts::new(Store::open(&db_path).unwrap());
    assert_eq!(reopened.show(enrolled.id).await.unwrap(), disabled);
    assert_eq!(reopened.list(0).await.unwrap().len(), 2);
    let db = rusqlite::Connection::open(&db_path).unwrap();
    assert_eq!(
        db.query_row("PRAGMA user_version", [], |r| r.get::<_, i64>(0))
            .unwrap(),
        mimik::SCHEMA_VERSION
    );
    identity_guards(&db);
    let owner = role_password_policy(&accounts, dir.path(), &owner, &password).await;
    let final_token = dir.path().join("owner-disabled-enroll");
    owner_disable(&accounts, &owner, &final_token, &password, enrolled.id).await;
}

async fn role_password_policy(
    accounts: &Accounts,
    directory: &std::path::Path,
    owner: &mimik::accounts::Summary,
    original_password: &std::path::Path,
) -> mimik::accounts::Summary {
    let short = directory.join("player-12.password");
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(&short)
        .unwrap();
    file.write_all(b"abcdefghijkl").unwrap();
    let token = directory.join("policy-invite.token");
    accounts
        .invite("policy-player".into(), "Policy player".into(), &token)
        .await
        .unwrap();
    let player = accounts.enroll(&token, &short).await.unwrap();
    assert!(accounts.verify_password(player.id, &short).await.unwrap());
    assert_eq!(
        accounts
            .authenticate(
                "policy-player".into(),
                zeroize::Zeroizing::new(b"abcdefghijkl".to_vec())
            )
            .await
            .unwrap()
            .unwrap()
            .id,
        player.id
    );
    let reset = directory.join("policy-owner-reset.token");
    accounts
        .issue_reset(owner.id, owner.epoch, &reset)
        .await
        .unwrap();
    assert!(
        accounts.complete_reset(&reset, &short).await.is_err(),
        "Owner minimum was relaxed to player minimum"
    );
    let completed = accounts
        .complete_reset(&reset, original_password)
        .await
        .unwrap();
    assert!(completed.owner);
    high_id_owner_policy(directory, &short, original_password).await;
    completed
}

async fn high_id_owner_policy(
    directory: &std::path::Path,
    short: &std::path::Path,
    password: &std::path::Path,
) {
    let path = directory.join("owner-above100.sqlite3");
    let store = Store::open(&path).unwrap();
    let db = rusqlite::Connection::open(&path).unwrap();
    db.execute("INSERT INTO accounts(id,login,display_name,state,epoch) VALUES(100,'reserved','Reserved','pending',1)",[]).unwrap();
    let accounts = Accounts::new(store);
    let owner = accounts
        .bootstrap("high-owner".into(), "High Owner".into(), password)
        .await
        .unwrap();
    assert!(owner.id > 100);
    let reset = directory.join("high-owner-reset.token");
    accounts
        .issue_reset(owner.id, owner.epoch, &reset)
        .await
        .unwrap();
    assert!(accounts.complete_reset(&reset, short).await.is_err());
    let fourteen = directory.join("owner14.password");
    let fifteen = directory.join("owner15.password");
    for (path, bytes) in [
        (&fourteen, b"abcdefghijklmn".as_slice()),
        (&fifteen, b"abcdefghijklmno".as_slice()),
    ] {
        OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(path)
            .unwrap()
            .write_all(bytes)
            .unwrap();
    }
    assert!(accounts.complete_reset(&reset, &fourteen).await.is_err());
    assert!(
        accounts
            .complete_reset(&reset, &fifteen)
            .await
            .unwrap()
            .owner
    );
    assert!(accounts.verify_password(owner.id, &fifteen).await.unwrap());
    assert!(accounts.verify_password(owner.id, &fourteen).await.is_err());
    let authenticated = accounts
        .authenticate(
            "high-owner".into(),
            zeroize::Zeroizing::new(b"abcdefghijklmno".to_vec()),
        )
        .await
        .unwrap()
        .unwrap();
    assert_eq!(authenticated.id, owner.id);
    assert!(authenticated.owner);
    assert!(
        accounts
            .authenticate(
                "high-owner".into(),
                zeroize::Zeroizing::new(b"abcdefghijklmn".to_vec())
            )
            .await
            .unwrap()
            .is_none()
    );
}

fn identity_guards(db: &rusqlite::Connection) {
    for sql in [
        "INSERT OR REPLACE INTO accounts SELECT * FROM accounts WHERE id=1",
        "INSERT OR REPLACE INTO account_owner VALUES(1,1)",
        "INSERT OR REPLACE INTO account_events(id,account_id,epoch,occurred_at,actor,action) VALUES(1,1,1,123,'local_operator','invite')",
        "DELETE FROM accounts",
        "UPDATE accounts SET login='replacement' WHERE id=1",
        "DELETE FROM account_owner",
        "UPDATE account_owner SET account_id=2",
        "DELETE FROM account_events",
        "UPDATE account_events SET action='invite'",
        "DELETE FROM account_credentials",
        "UPDATE accounts SET epoch=epoch+2 WHERE id=1",
        "UPDATE accounts SET state='enabled',epoch=epoch+1 WHERE id=2",
    ] {
        assert!(db.execute(sql, []).is_err(), "guard did not reject {sql}");
    }
}

async fn owner_disable(
    accounts: &Accounts,
    owner: &mimik::accounts::Summary,
    final_token: &std::path::Path,
    password: &std::path::Path,
    player_id: i64,
) {
    accounts
        .invite("late-player".into(), "Late Player".into(), final_token)
        .await
        .unwrap();
    accounts.disable(owner.id, owner.epoch).await.unwrap();
    assert!(accounts.show(player_id).await.is_err());
    assert!(accounts.list(0).await.is_err());
    assert!(
        accounts
            .invite(
                "blocked".into(),
                "Blocked".into(),
                &final_token.with_file_name("blocked-token")
            )
            .await
            .is_err()
    );
    assert!(
        accounts
            .bootstrap("replacement".into(), "Replacement".into(), password)
            .await
            .is_err()
    );
    let late = accounts.enroll(final_token, password).await.unwrap();
    assert!(!late.owner);
    assert!(accounts.verify_password(late.id, password).await.unwrap());
}

fn cli(db: &std::path::Path, args: &[&str]) -> serde_json::Value {
    let output = std::process::Command::new(env!("CARGO_BIN_EXE_mimik"))
        .arg("--database")
        .arg(db)
        .args(args)
        .output()
        .unwrap_or_else(|error| {
            panic!(
                "Cannot execute test binary {}: {error}",
                env!("CARGO_BIN_EXE_mimik")
            )
        });
    assert!(
        output.status.success(),
        "CLI failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(String::from_utf8_lossy(&output.stderr), "");
    let text = std::str::from_utf8(&output.stdout).unwrap();
    for forbidden in ["$argon2", "verifier", "digest", "synthetic password 123"] {
        assert!(!text.contains(forbidden));
    }
    serde_json::from_slice(&output.stdout).unwrap()
}

#[test]
fn cli_protected_file_lifecycle_and_secret_free_output() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::set_permissions(dir.path(), std::fs::Permissions::from_mode(0o700)).unwrap();
    let db = dir.path().join("cli.sqlite3");
    let password = dir.path().join("password");
    let alternate = dir.path().join("alternate");
    for (path, bytes) in [
        (&password, b"synthetic password 123".as_slice()),
        (&alternate, b"different password 123".as_slice()),
    ] {
        let mut file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(path)
            .unwrap();
        file.write_all(bytes).unwrap();
    }
    let pw = password.to_str().unwrap();
    let alt = alternate.to_str().unwrap();
    let owner = cli(
        &db,
        &[
            "bootstrap-owner",
            "--login",
            "owner",
            "--display-name",
            "Owner",
            "--password-file",
            pw,
        ],
    );
    assert_eq!(owner["owner"], true);
    let invite = dir.path().join("invite");
    let invite_path = invite.to_str().unwrap();
    let pending = cli(
        &db,
        &[
            "invite-account",
            "--login",
            "player",
            "--display-name",
            "Player",
            "--token-output",
            invite_path,
        ],
    );
    assert_eq!(pending["state"], "pending");
    let encoded = std::fs::read(&invite).unwrap();
    assert_eq!(encoded.len(), 64);
    assert!(
        encoded
            .iter()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(b))
    );
    assert_eq!(
        std::os::unix::fs::MetadataExt::mode(&std::fs::metadata(&invite).unwrap()) & 0o777,
        0o600
    );
    let enrolled = cli(
        &db,
        &[
            "enroll-account",
            "--token-file",
            invite_path,
            "--password-file",
            pw,
        ],
    );
    let id = enrolled["id"].as_i64().unwrap().to_string();
    assert_eq!(enrolled["epoch"], 2);
    cli_reset(&db, &dir, pw, alt, &id);
}

fn cli_reset(db: &std::path::Path, dir: &tempfile::TempDir, pw: &str, alt: &str, id: &str) {
    assert_eq!(
        cli(
            db,
            &["verify-account-password", "--id", id, "--password-file", pw]
        )["matched"],
        true
    );
    assert_eq!(
        cli(
            db,
            &[
                "verify-account-password",
                "--id",
                id,
                "--password-file",
                alt
            ]
        )["matched"],
        false
    );
    let reset = dir.path().join("reset");
    let reset_path = reset.to_str().unwrap();
    assert_eq!(
        cli(
            db,
            &[
                "issue-account-reset",
                "--id",
                id,
                "--expected-epoch",
                "2",
                "--token-output",
                reset_path
            ]
        )["epoch"],
        3
    );
    assert_eq!(
        cli(
            db,
            &[
                "complete-account-reset",
                "--token-file",
                reset_path,
                "--password-file",
                alt
            ]
        )["epoch"],
        4
    );
    assert_eq!(
        cli(
            db,
            &["verify-account-password", "--id", id, "--password-file", pw]
        )["matched"],
        false
    );
    assert_eq!(
        cli(
            db,
            &[
                "verify-account-password",
                "--id",
                id,
                "--password-file",
                alt
            ]
        )["matched"],
        true
    );
    assert_eq!(
        cli(
            db,
            &["disable-account", "--id", id, "--expected-epoch", "4"]
        )["state"],
        "disabled"
    );
    assert_eq!(cli(db, &["show-account", "--id", id])["epoch"], 5);
    assert_eq!(cli(db, &["list-accounts"]).as_array().unwrap().len(), 2);
    let output = std::process::Command::new(env!("CARGO_BIN_EXE_mimik"))
        .args(["enroll-account", "--password", "dummy-not-a-valid-option"])
        .output()
        .unwrap();
    assert!(!output.status.success());
    assert_eq!(String::from_utf8_lossy(&output.stdout), "");
}

const BEFORE_ACCOUNTS: [&str; 9] = [
    include_str!("../migrations/001_characters.sql"),
    include_str!("../migrations/002_mork_borg.sql"),
    include_str!("../migrations/003_sources.sql"),
    include_str!("../migrations/004_source_coverage.sql"),
    include_str!("../migrations/005_registry.sql"),
    include_str!("../migrations/006_definition_drafts.sql"),
    include_str!("../migrations/007_source_finalization.sql"),
    include_str!("../migrations/008_definition_publication.sql"),
    include_str!("../migrations/009_review_integrity.sql"),
];

#[test]
fn schema10_upgrades_real_historical_schemas_and_rolls_back_collision() {
    for version in [0, 1, 4, 5, 6, 7, 8, 9] {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("migration.sqlite3");
        let db = rusqlite::Connection::open(&path).unwrap();
        db.execute_batch("PRAGMA foreign_keys=ON").unwrap();
        for migration in BEFORE_ACCOUNTS.iter().take(version) {
            db.execute_batch(migration).unwrap();
        }
        if version > 0 {
            db.execute("INSERT INTO characters(name,system_id,edition,notes) VALUES('Retained','mork-borg','it-2025','Exact retained notes')",[]).unwrap();
        }
        drop(db);
        Store::open(&path).unwrap();
        Store::open(&path).unwrap();
        let db = rusqlite::Connection::open(&path).unwrap();
        assert_eq!(
            db.query_row("PRAGMA user_version", [], |r| r.get::<_, i64>(0))
                .unwrap(),
            mimik::SCHEMA_VERSION
        );
        assert_eq!(
            db.query_row("SELECT count(*) FROM accounts", [], |r| r.get::<_, i64>(0))
                .unwrap(),
            0
        );
        assert_eq!(
            db.query_row("SELECT count(*) FROM pragma_foreign_key_check", [], |r| r
                .get::<_, i64>(
                0
            ))
            .unwrap(),
            0
        );
        if version > 0 {
            assert_eq!(
                db.query_row("SELECT notes FROM characters", [], |r| r
                    .get::<_, String>(0))
                    .unwrap(),
                "Exact retained notes"
            );
        }
    }
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("rollback.sqlite3");
    let db = rusqlite::Connection::open(&path).unwrap();
    for migration in BEFORE_ACCOUNTS {
        db.execute_batch(migration).unwrap();
    }
    db.execute_batch("CREATE TABLE account_events(sentinel TEXT); INSERT INTO account_events VALUES('retained collision');").unwrap();
    drop(db);
    assert!(Store::open(&path).is_err());
    let db = rusqlite::Connection::open(&path).unwrap();
    assert_eq!(
        db.query_row("PRAGMA user_version", [], |r| r.get::<_, i64>(0))
            .unwrap(),
        9
    );
    assert_eq!(
        db.query_row(
            "SELECT count(*) FROM sqlite_master WHERE name='accounts'",
            [],
            |r| r.get::<_, i64>(0)
        )
        .unwrap(),
        0
    );
    assert_eq!(
        db.query_row("SELECT sentinel FROM account_events", [], |r| r
            .get::<_, String>(0))
            .unwrap(),
        "retained collision"
    );
}
