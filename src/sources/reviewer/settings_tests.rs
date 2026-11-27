//! The settings validation and the row (docs/api-reviewer-plan.md step 3): the per-provider
//! base URL defaults and path rules (what rig 0.47.1 appends, as part 1's fixtures record),
//! the model name bound, and the single database row. The key is a dummy; no environment key
//! is read.
use super::settings::{self, MODEL_MAX_BYTES, MODEL_PREFIX, ORIGIN_MAX_BYTES};
use super::{Provider, key};
use crate::Store;

#[test]
fn provider_defaults_and_path_rules_match_the_wire_fixtures() {
    assert_eq!(
        Provider::Mistral.default_base_url(),
        "https://api.mistral.ai"
    );
    assert_eq!(
        Provider::OpenAi.default_base_url(),
        "https://api.openai.com/v1"
    );
    assert_eq!(
        Provider::Anthropic.default_base_url(),
        "https://api.anthropic.com"
    );
    // rig appends /v1/chat/completions to Mistral's bare host, /responses to OpenAI's /v1
    // base and /v1/messages to Anthropic's bare host (tests/fixtures/reviewer/*/request.json).
    assert_eq!(Provider::Mistral.base_path(), "/");
    assert_eq!(Provider::OpenAi.base_path(), "/v1");
    assert_eq!(Provider::Anthropic.base_path(), "/");
    for provider in [Provider::Mistral, Provider::OpenAi, Provider::Anthropic] {
        assert_eq!(Provider::parse(provider.as_str()), Some(provider));
        assert_ne!(provider.base_help(), "");
        assert_ne!(provider.label(), "");
    }
    assert_eq!(Provider::parse("google"), None);
}

#[test]
fn the_model_bound_is_100_bytes_minus_the_reviewer_prefix() {
    assert_eq!(MODEL_PREFIX, "reviewer:");
    assert_eq!(ORIGIN_MAX_BYTES, 100);
    assert_eq!(MODEL_MAX_BYTES, 91);
    assert_eq!(MODEL_PREFIX.len() + MODEL_MAX_BYTES, ORIGIN_MAX_BYTES);
}

#[test]
fn validate_accepts_the_defaults_and_refuses_bad_input() {
    let ok = settings::validate("mistral", "https://api.mistral.ai", "mistral-large-latest")
        .expect("the default base URL is valid");
    assert_eq!(ok.provider, Provider::Mistral);
    assert_eq!(ok.base_url.as_str(), "https://api.mistral.ai/");
    assert_eq!(ok.model, "mistral-large-latest");
    assert!(
        settings::validate("openai", "https://api.openai.com/v1", "gpt-5").is_ok(),
        "OpenAI's base includes /v1"
    );
    let err = settings::validate("openai", "https://api.openai.com", "gpt-5").unwrap_err();
    assert!(err.contains("/v1 path"), "{err}");
    let err = settings::validate("mistral", "https://api.mistral.ai/v1", "m").unwrap_err();
    assert!(err.contains("bare host"), "{err}");
    let err = settings::validate("anthropic", "https://api.anthropic.com/v1", "m").unwrap_err();
    assert!(err.contains("bare host"), "{err}");
    // http, IP literals in every form, userinfo, query, fragment, malformed, over-long.
    for (provider, url) in [
        ("mistral", "http://api.mistral.ai"),
        ("mistral", "https://127.0.0.1"),
        ("mistral", "https://127.1/"),
        ("mistral", "https://0x7f.1/"),
        ("mistral", "https://2130706433/"),
        ("mistral", "https://[::1]/v1"),
        ("openai", "https://user:secret@api.openai.com/v1"),
        ("openai", "https://user@api.openai.com/v1"),
        ("anthropic", "https://api.anthropic.com?x=1"),
        ("anthropic", "https://api.anthropic.com#f"),
        ("mistral", "not a url"),
        ("mistral", &format!("https://{}.example", "a".repeat(2100))),
    ] {
        assert!(settings::validate(provider, url, "m").is_err(), "{url}");
    }
    // An explicit port is fine (the tests' fake server uses one).
    assert!(settings::validate("mistral", "https://api.mistral.ai:8443", "m").is_ok());
    // The provider must be one of the three.
    assert!(settings::validate("google", "https://api.google.com", "m").is_err());
    // The model: empty after trimming, over-long, NUL; exactly at the bound is fine.
    assert!(settings::validate("mistral", "https://api.mistral.ai", "").is_err());
    assert!(settings::validate("mistral", "https://api.mistral.ai", "   ").is_err());
    let err = settings::validate("mistral", "https://api.mistral.ai", &"m".repeat(92)).unwrap_err();
    assert!(err.contains("91"), "{err}");
    assert!(
        settings::validate(
            "mistral",
            "https://api.mistral.ai",
            &"m".repeat(MODEL_MAX_BYTES)
        )
        .is_ok()
    );
    assert!(settings::validate("mistral", "https://api.mistral.ai", "m\0").is_err());
    // Any control character anywhere in the name is refused, like `Report::origin` does:
    // a DEL and a tab in the middle (the ends are trimmed, the middle is not).
    for bad in ["m\x7F", "ab\x7Fcd", "ab\tcd", "ab\ncd", "ab\rcd"] {
        let err = settings::validate("mistral", "https://api.mistral.ai", bad).unwrap_err();
        assert!(err.contains("control character"), "{bad:?}: {err}");
    }
    // A multi-byte model name counts bytes, not characters.
    let model = "ä".repeat(46); // 92 bytes
    assert!(settings::validate("mistral", "https://api.mistral.ai", &model).is_err());
    let model = "ä".repeat(45); // 90 bytes
    assert!(settings::validate("mistral", "https://api.mistral.ai", &model).is_ok());
}

#[tokio::test]
async fn the_settings_row_round_trips_and_defaults_when_empty() {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::open(&dir.path().join("private.sqlite3")).unwrap();
    let settings = store.reviewer_settings().await.unwrap();
    assert_eq!(settings.provider, Provider::Mistral);
    assert_eq!(settings.base_url, "https://api.mistral.ai");
    assert_eq!(settings.model, "");
    assert_eq!(settings.changed_at, 0);
    let valid = settings::validate(
        "anthropic",
        "https://api.anthropic.com",
        "claude-sonnet-4-5",
    )
    .unwrap();
    store
        .run(move |db| {
            let now = super::settings::Settings::save(db, 1, &valid)?;
            assert!(now > 0);
            let loaded = super::settings::Settings::load(db)?;
            assert_eq!(loaded.provider, Provider::Anthropic);
            assert_eq!(loaded.base_url, "https://api.anthropic.com/");
            assert_eq!(loaded.model, "claude-sonnet-4-5");
            assert_eq!(loaded.changed_at, now);
            // Replacing the row keeps exactly one row.
            let other = settings::validate("openai", "https://api.openai.com/v1", "gpt-5").unwrap();
            let second = super::settings::Settings::save(db, 1, &other)?;
            assert!(second >= now);
            let loaded = super::settings::Settings::load(db)?;
            assert_eq!(loaded.provider, Provider::OpenAi);
            assert_eq!(loaded.model, "gpt-5");
            let rows: i64 = db.query_row("SELECT count(*) FROM reviewer_settings", [], |row| {
                row.get(0)
            })?;
            assert_eq!(rows, 1);
            Ok(())
        })
        .await
        .unwrap();
    // The row's column bounds refuse what the application also refuses.
    store
        .run(move |db| {
            let refused = db.execute(
                "INSERT INTO reviewer_settings(id,provider,base_url,model,changed_by,changed_at)
                 VALUES(1,'mistral','https://api.mistral.ai',?1,1,1)",
                [&"m".repeat(92)],
            );
            assert!(
                refused.is_err(),
                "the over-long model is refused by the column too"
            );
            let refused = db.execute(
                "INSERT INTO reviewer_settings(id,provider,base_url,model,changed_by,changed_at)
                 VALUES(1,'google','https://api.google.com','m',1,1)",
                [],
            );
            assert!(
                refused.is_err(),
                "an unknown provider is refused by the column too"
            );
            Ok(())
        })
        .await
        .unwrap();
    // The key file's folder exists beside the database, and no key is set.
    assert_eq!(
        key::Secrets::in_directory(dir.path())
            .unwrap()
            .status()
            .unwrap(),
        None
    );
}
