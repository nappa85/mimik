//! M4 follow-up: browser textarea submissions (CR LF) on the legacy form decoders are stored
//! with LF, and length limits count typed characters. Each test fails if its field loses the
//! `form_text` normalisation.
use axum::{
    body::{Body, to_bytes},
    http::{Request, StatusCode},
};
use mimik::{CharacterInput, Store, app, mork_borg::EDITION};
use tower::ServiceExt;

#[path = "support/source_fixture.rs"]
mod support;

fn post(uri: &str, body: String) -> Request<Body> {
    Request::builder()
        .method("POST")
        .uri(uri)
        .header("host", "localhost:3000")
        .header("content-type", "application/x-www-form-urlencoded")
        .body(Body::from(body))
        .unwrap()
}

async fn text(response: axum::response::Response) -> String {
    String::from_utf8(
        to_bytes(response.into_body(), 4_000_000)
            .await
            .unwrap()
            .to_vec(),
    )
    .unwrap()
}

fn textarea(html: &str, name: &str) -> String {
    scraper::Html::parse_document(html)
        .select(&scraper::Selector::parse(&format!("textarea[name={name}]")).unwrap())
        .next()
        .unwrap_or_else(|| panic!("textarea {name}"))
        .text()
        .collect()
}

/// `count` typed characters ending lines with LF (what the user typed and what is stored).
fn typed(count: usize, letter: char) -> String {
    format!("{letter}\n").repeat(count / 2)
}

fn crlf(value: &str) -> String {
    value.replace('\n', "\r\n")
}

async fn mork_character(store: &Store) -> (i64, i64) {
    let created = store
        .create(CharacterInput {
            name: "Newline fixture".into(),
            system_id: "mork-borg".into(),
            edition: EDITION.into(),
            notes: String::new(),
            revision: 0,
        })
        .await
        .unwrap();
    (created.id, created.input.revision)
}

#[tokio::test]
async fn mork_borg_sheet_textareas_crlf_at_the_limit_are_stored_as_lf() {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::open(&dir.path().join("sheet.sqlite3")).unwrap();
    let (id, revision) = mork_character(&store).await;
    let long = |letter| typed(20_000, letter);
    let short = |letter| typed(2_000, letter);
    let fields = [
        ("equipment", long('e')),
        ("description", long('d')),
        ("class_notes", long('c')),
        ("powers", long('p')),
        ("weapon_one", short('w')),
        ("weapon_two", short('v')),
    ];
    let mut form: Vec<(String, String)> = [
        ("revision", revision.to_string()),
        ("agility", "0".into()),
        ("presence", "0".into()),
        ("strength", "0".into()),
        ("toughness", "0".into()),
        ("hp", "3".into()),
        ("max_hp", "5".into()),
        ("worn_armor", "0".into()),
        ("protection", "0".into()),
        ("standard_items", "6".into()),
        ("silver", "10".into()),
        ("omens", "2".into()),
    ]
    .into_iter()
    .map(|(k, v)| (k.to_owned(), v))
    .collect();
    form.extend(fields.iter().map(|(k, v)| ((*k).to_owned(), crlf(v))));
    let body = serde_urlencoded::to_string(&form).unwrap();
    assert!(body.contains("%0D%0A"));
    let response = app(store.clone())
        .oneshot(post(&format!("/characters/{id}/mork-borg"), body))
        .await
        .unwrap();
    assert_eq!(
        response.status(),
        StatusCode::SEE_OTHER,
        "typed characters within limits"
    );
    let (_, sheet, _) = store.mork_borg_sheet(id).await.unwrap();
    let stored = [
        ("equipment", &sheet.equipment),
        ("description", &sheet.description),
        ("class_notes", &sheet.class_notes),
        ("powers", &sheet.powers),
        ("weapon_one", &sheet.weapon_one),
        ("weapon_two", &sheet.weapon_two),
    ];
    for ((name, want), (_, got)) in fields.iter().zip(stored) {
        assert_eq!(got, want, "{name} stored with LF");
    }
}

#[tokio::test]
async fn creation_notes_crlf_at_the_limit_are_stored_as_lf() {
    let dir = tempfile::tempdir().unwrap();
    support::fixture_pair(dir.path()).unwrap();
    let store = Store::open(&dir.path().join("create.sqlite3")).unwrap();
    store
        .import_source(
            &dir.path().join("adapter-bundle"),
            &dir.path().join("original.pdf"),
        )
        .await
        .unwrap();
    let notes = typed(20_000, 'n');
    let body = serde_urlencoded::to_string([
        ("adapter_id", "mork-borg-it-2025"),
        ("name", "Created with notes"),
        ("notes", &crlf(&notes)),
    ])
    .unwrap();
    let response = app(store.clone())
        .oneshot(post("/characters", body))
        .await
        .unwrap();
    assert_eq!(
        response.status(),
        StatusCode::SEE_OTHER,
        "20,000 typed characters"
    );
    let created = store.list().await.unwrap().remove(0);
    assert_eq!(store.get(created.id).await.unwrap().input.notes, notes);
}

#[tokio::test]
async fn creation_page_keeps_leading_newlines_in_a_rejected_draft() {
    let dir = tempfile::tempdir().unwrap();
    support::fixture_pair(dir.path()).unwrap();
    let store = Store::open(&dir.path().join("draft.sqlite3")).unwrap();
    store
        .import_source(
            &dir.path().join("adapter-bundle"),
            &dir.path().join("original.pdf"),
        )
        .await
        .unwrap();
    // A blank name is refused; the creation page re-renders the draft.
    let body = serde_urlencoded::to_string([
        ("adapter_id", "mork-borg-it-2025"),
        ("name", "   "),
        ("notes", "\r\n\r\ndraft notes"),
    ])
    .unwrap();
    let response = app(store.clone())
        .oneshot(post("/characters", body))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::UNPROCESSABLE_ENTITY);
    let html = text(response).await;
    assert!(!html.contains('\r'));
    assert_eq!(textarea(&html, "notes"), "\n\ndraft notes");
    assert!(store.list().await.unwrap().is_empty());
}
