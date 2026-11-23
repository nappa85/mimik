//! M4 follow-up: browser textarea submissions (CR LF) on the legacy character form are stored
//! with LF, and length limits count typed characters. The test fails if the field loses the
//! `form_text` normalisation. The same normalisation on runtime sheets is covered by
//! `browser_crlf_submission_is_stored_as_lf` (`src/campaigns/list_tests.rs`).
use axum::{
    body::Body,
    http::{Request, StatusCode},
};
use mimik::{CharacterInput, Store, app};
use tower::ServiceExt;

fn post(uri: &str, body: String) -> Request<Body> {
    Request::builder()
        .method("POST")
        .uri(uri)
        .header("host", "localhost:3000")
        .header("content-type", "application/x-www-form-urlencoded")
        .body(Body::from(body))
        .unwrap()
}

/// `count` typed characters ending lines with LF (what the user typed and what is stored).
fn typed(count: usize, letter: char) -> String {
    format!("{letter}\n").repeat(count / 2)
}

#[tokio::test]
async fn legacy_character_notes_crlf_at_the_limit_are_stored_as_lf() {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::open(&dir.path().join("legacy.sqlite3")).unwrap();
    let created = store
        .create(CharacterInput {
            name: "Newline fixture".into(),
            system_id: "dnd".into(),
            edition: "Example".into(),
            notes: String::new(),
            revision: 0,
        })
        .await
        .unwrap();
    let notes = typed(20_000, 'n');
    let body = serde_urlencoded::to_string([
        ("name", "Newline fixture"),
        ("system_id", "dnd"),
        ("edition", "Example"),
        ("revision", &created.input.revision.to_string()),
        ("notes", &notes.replace('\n', "\r\n")),
    ])
    .unwrap();
    assert!(body.contains("%0D%0A"));
    let response = app(store.clone())
        .oneshot(post(&format!("/characters/{}", created.id), body))
        .await
        .unwrap();
    assert_eq!(
        response.status(),
        StatusCode::SEE_OTHER,
        "20,000 typed characters"
    );
    assert_eq!(store.get(created.id).await.unwrap().input.notes, notes);
}
