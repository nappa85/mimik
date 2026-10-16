//! M3: derived values and advisory checks on runtime sheet routes (view, save, history).
use super::{fixture, published_fixture, router_request};
use crate::{Store, auth::RequestAuth};
use axum::http::StatusCode;
use serde_json::{Value, json};
use std::collections::BTreeSet;

const DERIVED: [&str; 6] = ["pace", "lucky", "lucky_twice", "tier", "marked", "echo"];

fn mapped() -> Value {
    json!({"state": "mapped", "regions": [{"source_id": 1, "page_id": 1, "unit_id": 1}]})
}

fn field(id: &str, label: &str, control: &str, nullable: bool) -> Value {
    json!({"id": id, "label": label, "control": control, "nullable": nullable,
           "mapping": mapped()})
}

fn derived(id: &str, label: &str, control: &str, expr: &str) -> Value {
    let mut value = field(id, label, control, false);
    value["formula"] = json!({"expr": expr, "rule_ids": ["r-1"],
                              "evidence": {"state": "unresolved", "reason": "Synthetic"}});
    value
}

/// Scalar-only formula definition (lists stay gated until M4). Labels, messages, option
/// labels and an expression literal carry markup so escaping is checked everywhere.
pub(super) fn formula_definition() -> Value {
    let mut kind = field("kind", "Kind", "enum", false);
    kind["options"] = json!([{"id": "plain", "label": "<script>"}, {"id": "odd", "label": "Odd"}]);
    let mut tier = derived("tier", "Tier", "enum", "vitality > 3 ? \"high\" : \"low\"");
    tier["options"] =
        json!([{"id": "high", "label": "<b>High</b>"}, {"id": "low", "label": "Low"}]);
    let fields = vec![
        field("label", "<script>Label", "text", false),
        field("vitality", "Vitality", "integer", false),
        field("luck", "Luck", "integer", true),
        kind,
        derived("pace", "Pace", "integer", "6 - vitality"),
        derived("lucky", "Lucky", "integer", "luck + 1"),
        derived("lucky_twice", "Lucky twice", "integer", "lucky * 2"),
        tier,
        derived("marked", "Marked", "boolean", "label == \"<script>\""),
        derived("echo", "Echo", "text", "label"),
    ];
    let ids: Vec<Value> = fields.iter().map(|field| field["id"].clone()).collect();
    json!({
        "contract_version": 1, "game_id": "synthetic-game", "edition_id": "first",
        "definition_id": "formula-sheet", "title": "Formula sheet",
        "sources": [{"source_id": 1, "pdf_sha256": "0".repeat(64), "roles": ["sheet"]}],
        "fields": fields,
        "groups": [{"id": "main", "label": "Main", "fields": ids, "column_hint": "main"}],
        "formula_profile": 1,
        "constraints": [
            {"id": "vitality-cap", "expr": "vitality <= 10", "rule_ids": ["r-2"],
             "message": "<script>alert(1)</script>",
             "evidence": {"state": "unresolved", "reason": "Synthetic"}},
            {"id": "lucky-positive", "expr": "lucky > 0", "rule_ids": [],
             "message": "Lucky must be positive.",
             "evidence": {"state": "unresolved", "reason": "Synthetic"}}
        ]
    })
}

/// Publishes `definition` after the generic fixture (which loads the synthetic source).
pub(super) async fn publish(store: &Store, definition: &Value) -> i64 {
    published_fixture(store).await;
    crate::definitions::publish_unshipped_for_tests();
    let draft = store
        .save_definition_draft(serde_json::to_vec(definition).unwrap(), 0)
        .await
        .unwrap();
    let comparison = store.compare_definition(draft.id, false).await.unwrap();
    store
        .approve_definition(
            draft.id,
            false,
            crate::definitions::ApprovalRequest {
                comparison_hash: comparison.comparison_hash,
                actor: "Synthetic test".into(),
                rationale: "Synthetic original compared".into(),
                originals_compared: true,
            },
        )
        .await
        .unwrap()
        .id
}

pub(super) struct Setup {
    pub _dir: tempfile::TempDir,
    pub store: Store,
    pub auth: Vec<RequestAuth>,
    pub app: axum::Router,
    pub campaign: i64,
    pub version: i64,
}

pub(super) async fn setup(definition: &Value) -> Setup {
    let (dir, store, auth, _clock) = fixture().await;
    let version = publish(&store, definition).await;
    let campaign = store
        .create_campaign(
            auth[1].clone(),
            "Formula".into(),
            "synthetic-game".into(),
            "first".into(),
        )
        .await
        .unwrap();
    let app = crate::auth::app(store.clone()).await.unwrap();
    Setup {
        _dir: dir,
        store,
        auth,
        app,
        campaign,
        version,
    }
}

pub(super) fn form(revision: i64, vitality: &str, luck: Option<&str>) -> Vec<(String, String)> {
    vec![
        ("csrf".into(), "a".repeat(64)),
        ("revision".into(), revision.to_string()),
        ("name".into(), "Formula sheet".into()),
        ("notes".into(), String::new()),
        ("value-label".into(), "<script>".into()),
        ("value-vitality".into(), vitality.into()),
        ("value-luck".into(), luck.unwrap_or_default().into()),
        (
            "null-luck".into(),
            if luck.is_some() { "known" } else { "unknown" }.into(),
        ),
        ("value-kind".into(), "plain".into()),
    ]
}

pub(super) async fn body(response: axum::response::Response) -> String {
    String::from_utf8(
        axum::body::to_bytes(response.into_body(), 4 * 1024 * 1024)
            .await
            .unwrap()
            .to_vec(),
    )
    .unwrap()
}

impl Setup {
    pub(super) async fn get(&self, path: &str) -> (StatusCode, String) {
        let response = router_request(&self.app, &self.auth[1], path, None).await;
        (response.status(), body(response).await)
    }
    pub(super) async fn post(
        &self,
        path: &str,
        form: Vec<(String, String)>,
    ) -> (StatusCode, String) {
        let response = router_request(&self.app, &self.auth[1], path, Some(form)).await;
        (response.status(), body(response).await)
    }
    fn create_path(&self) -> String {
        format!(
            "/campaigns/{}/sheets/new?version={}",
            self.campaign, self.version
        )
    }
    /// Creates sheet 1 at revision 1 (vitality 4, luck 2: everything passes).
    pub(super) async fn create(&self) {
        let (status, _) = self
            .post(&self.create_path(), form(0, "4", Some("2")))
            .await;
        assert_eq!(status, StatusCode::SEE_OTHER);
    }
    async fn stored(&self) -> (i64, Value, i64) {
        let (view, permit) = self
            .store
            .private_read(
                self.auth[1].clone(),
                1,
                None,
                crate::definitions::admit().unwrap(),
            )
            .await
            .unwrap();
        drop(permit);
        let history = self
            .store
            .private_history(self.auth[1].clone(), 1, 0)
            .await
            .unwrap()
            .len();
        (
            view.snapshot.revision,
            view.snapshot.data,
            i64::try_from(history).unwrap(),
        )
    }
}

/// Every `name="…"` attribute inside the sheet form.
fn names(html: &str) -> BTreeSet<String> {
    let form = &html[html.find("<form").unwrap()..html.find("</form>").unwrap()];
    form.split("name=\"")
        .skip(1)
        .map(|rest| rest[..rest.find('"').unwrap()].to_owned())
        .collect()
}

/// The section with the given ID, up to its closing tag (sections do not nest).
fn section<'a>(html: &'a str, id: &str) -> &'a str {
    let start = html
        .find(&format!("id=\"{id}\""))
        .unwrap_or_else(|| panic!("section {id} missing"));
    let end = start + html[start..].find("</section>").unwrap();
    &html[start..end]
}

/// The inline derived-value blocks, in page order.
fn derived_blocks(html: &str) -> Vec<&str> {
    html.split("<dl class=\"derived\">")
        .skip(1)
        .map(|rest| &rest[..rest.find("</dl>").unwrap()])
        .collect()
}

fn derived_text<'a>(html: &'a str, id: &str) -> &'a str {
    let block = derived_blocks(html)
        .into_iter()
        .find(|block| block.contains(&format!("id=\"label-{id}\"")))
        .unwrap_or_else(|| panic!("derived {id} not rendered"));
    let value = &block[block.find("<dd").unwrap()..];
    &value[value.find('>').unwrap() + 1..value.find("</dd>").unwrap()]
}

/// Numeric (Askama) and named HTML escapes of `raw`.
fn escaped(raw: &str) -> [String; 2] {
    let with = |amp: &str, lt: &str, gt: &str, quote: &str| {
        raw.replace('&', amp)
            .replace('<', lt)
            .replace('>', gt)
            .replace('"', quote)
    };
    [
        with("&#38;", "&#60;", "&#62;", "&#34;"),
        with("&amp;", "&lt;", "&gt;", "&quot;"),
    ]
}

fn assert_escaped(html: &str, raw: &str, context: &str) {
    assert!(!html.contains(raw), "{context}: raw {raw} present");
    assert!(
        escaped(raw).iter().any(|text| html.contains(text.as_str())),
        "{context}: escaped {raw} absent"
    );
}

#[tokio::test]
async fn formula_sheet_view_shows_derived_values_and_advisory_checks() {
    let _auth_lock = crate::auth::test_resource_lock().await;
    let _runtime_lock = crate::definitions::TEST_RESOURCE_LOCK.lock().await;
    let setup = setup(&formula_definition()).await;
    setup.create().await;
    let (status, html) = setup.get("/private-sheets/1").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(derived_text(&html, "pace"), "2");
    assert_eq!(derived_text(&html, "lucky"), "3");
    assert_eq!(derived_text(&html, "lucky_twice"), "6");
    assert_eq!(derived_text(&html, "marked"), "True");
    let checks = section(&html, "advisory-checks");
    assert!(
        checks.contains("Advisory only: checks never block saving and are not a legality ruling.")
    );
    assert!(checks.contains("0 violated, 0 not evaluated, 2 passed."));
    assert!(checks.contains("Lucky must be positive."));
    assert!(
        html.contains(
            "<a href=\"#advisory-checks\">Advisory checks: 0 violated, 0 not evaluated</a>"
        )
    );
    assert!(section(&html, "formula-results").contains("Computed from saved revision 1"));
    assert!(html.contains(
        "<summary>How this is computed</summary><pre class=\"evidence-text\">6 - vitality</pre>"
    ));
}

#[tokio::test]
async fn derived_fields_render_without_form_names() {
    let _auth_lock = crate::auth::test_resource_lock().await;
    let _runtime_lock = crate::definitions::TEST_RESOURCE_LOCK.lock().await;
    let setup = setup(&formula_definition()).await;
    setup.create().await;
    let expected: BTreeSet<String> = [
        "csrf",
        "revision",
        "name",
        "notes",
        "value-label",
        "value-vitality",
        "value-luck",
        "null-luck",
        "value-kind",
    ]
    .into_iter()
    .map(String::from)
    .collect();
    for path in ["/private-sheets/1", "/private-sheets/1/history/1"] {
        let (status, html) = setup.get(path).await;
        assert_eq!(status, StatusCode::OK);
        // History is read-only; its controls are the same form, disabled.
        assert_eq!(names(&html), expected, "{path}");
        for id in DERIVED {
            assert!(
                !html.contains(&format!("for=\"value-{id}\"")),
                "{path} {id}"
            );
            assert!(!html.contains(&format!("id=\"value-{id}\"")), "{path} {id}");
            assert!(
                !derived_text(&html, id).is_empty(),
                "{path}: derived {id} must be rendered"
            );
        }
    }
}

#[tokio::test]
async fn submitted_derived_key_is_rejected_and_not_saved() {
    let _auth_lock = crate::auth::test_resource_lock().await;
    let _runtime_lock = crate::definitions::TEST_RESOURCE_LOCK.lock().await;
    let setup = setup(&formula_definition()).await;
    setup.create().await;
    let before = setup.stored().await;
    let mut extra = form(1, "4", Some("2"));
    extra.push(("value-pace".into(), "99".into()));
    let mut substituted = form(1, "4", Some("2"));
    substituted.retain(|(key, _)| key != "value-kind");
    substituted.push(("value-pace".into(), "99".into()));
    let mut null_key = form(1, "4", Some("2"));
    null_key.retain(|(key, _)| key != "value-kind");
    null_key.push(("null-pace".into(), "unknown".into()));
    for submitted in [extra, substituted, null_key] {
        let (status, _) = setup.post("/private-sheets/1", submitted).await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
    }
    assert_eq!(setup.stored().await, before);
}

#[tokio::test]
async fn violating_save_succeeds_and_view_lists_violation() {
    let _auth_lock = crate::auth::test_resource_lock().await;
    let _runtime_lock = crate::definitions::TEST_RESOURCE_LOCK.lock().await;
    let setup = setup(&formula_definition()).await;
    setup.create().await;
    let (status, _) = setup.post("/private-sheets/1", form(1, "11", None)).await;
    assert_eq!(
        status,
        StatusCode::SEE_OTHER,
        "advisory checks never block saving"
    );
    let (revision, data, _) = setup.stored().await;
    assert_eq!(revision, 2);
    let keys: BTreeSet<&str> = data
        .as_object()
        .unwrap()
        .keys()
        .map(String::as_str)
        .collect();
    assert_eq!(keys, BTreeSet::from(["label", "vitality", "luck", "kind"]));
    let (_, html) = setup.get("/private-sheets/1").await;
    assert!(html.contains("Advisory checks: 1 violated, 1 not evaluated</a>"));
    let checks = section(&html, "advisory-checks");
    let violated = checks.find("<strong>Violated</strong>").unwrap();
    let not_evaluated = checks.find("<strong>Not evaluated</strong>").unwrap();
    assert!(violated < not_evaluated, "violations first");
    assert!(checks.contains("<code>dependency_failed</code>"));
    assert!(derived_text(&html, "lucky").contains("<code>null_value</code>"));
    assert!(derived_text(&html, "lucky").starts_with("Not computed: "));
    assert!(derived_text(&html, "lucky_twice").contains("<code>dependency_failed</code>"));
    assert_eq!(derived_text(&html, "pace"), "-5");
}

#[tokio::test]
async fn failed_save_rerender_labels_the_saved_revision() {
    let _auth_lock = crate::auth::test_resource_lock().await;
    let _runtime_lock = crate::definitions::TEST_RESOURCE_LOCK.lock().await;
    let setup = setup(&formula_definition()).await;
    setup.create().await;
    let (status, html) = setup
        .post("/private-sheets/1", form(1, "not-a-number", None))
        .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
    assert!(html.contains("value=\"not-a-number\""), "draft kept");
    let results = section(&html, "formula-results");
    assert!(results.contains("Computed from saved revision 1 — not from your unsaved changes"));
    assert!(results.contains(
        "<dt>Pace <span class=\"badge\">Computed</span></dt><dd class=\"evidence-text\">2</dd>"
    ));
    assert!(section(&html, "advisory-checks").contains("Advisory checks of saved revision 1"));
    for id in DERIVED {
        assert!(
            derived_text(&html, id).starts_with("Not recomputed for unsaved changes"),
            "{id}: the draft is never shown with saved values inline"
        );
    }
}

/// Derived and advisory fragments of one page (full pages carry CSRF and revisions).
fn fragments(html: &str) -> (Vec<String>, String) {
    (
        derived_blocks(html).into_iter().map(String::from).collect(),
        section(html, "advisory-checks").to_owned(),
    )
}

#[tokio::test]
async fn history_recomputes_identically() {
    let _auth_lock = crate::auth::test_resource_lock().await;
    let _runtime_lock = crate::definitions::TEST_RESOURCE_LOCK.lock().await;
    let setup = setup(&formula_definition()).await;
    setup.create().await;
    let (_, first) = setup.get("/private-sheets/1").await;
    let (status, _) = setup
        .post("/private-sheets/1", form(1, "11", Some("2")))
        .await;
    assert_eq!(status, StatusCode::SEE_OTHER);
    let (status, history) = setup.get("/private-sheets/1/history/1").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(fragments(&history), fragments(&first));
    let (_, current) = setup.get("/private-sheets/1").await;
    assert_ne!(
        fragments(&current),
        fragments(&first),
        "history uses historical data"
    );
    assert_eq!(derived_text(&history, "pace"), "2");
    assert_eq!(derived_text(&current, "pace"), "-5");
    assert!(history.contains("value-vitality\" value=\"4\" inputmode=\"numeric\" disabled"));
}

#[tokio::test]
async fn withdrawn_approval_still_evaluates_with_warning() {
    let _auth_lock = crate::auth::test_resource_lock().await;
    let _runtime_lock = crate::definitions::TEST_RESOURCE_LOCK.lock().await;
    let setup = setup(&formula_definition()).await;
    setup.create().await;
    setup
        .store
        .run(|db| {
            let sql: String = db.query_row(
                "SELECT sql FROM sqlite_schema WHERE name='source_finalize_delete'",
                [],
                |r| r.get(0),
            )?;
            db.execute_batch("DROP TRIGGER source_finalize_delete;")?;
            db.execute("DELETE FROM source_finalizations WHERE source_id=1", [])?;
            db.execute_batch(&sql)?;
            Ok(())
        })
        .await
        .unwrap();
    let (status, html) = setup.get("/private-sheets/1").await;
    assert_eq!(status, StatusCode::OK);
    assert!(html.contains("evidence is unavailable"));
    assert_eq!(derived_text(&html, "pace"), "2");
    assert!(section(&html, "advisory-checks").contains("2 passed."));
}

#[tokio::test]
async fn formula_text_is_escaped() {
    let _auth_lock = crate::auth::test_resource_lock().await;
    let _runtime_lock = crate::definitions::TEST_RESOURCE_LOCK.lock().await;
    let setup = setup(&formula_definition()).await;
    setup.create().await;
    let (_, view) = setup.get("/private-sheets/1").await;
    let (status, rerender) = setup.post("/private-sheets/1", form(1, "bad", None)).await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
    let (_, history) = setup.get("/private-sheets/1/history/1").await;
    let (status, _) = setup.post("/private-sheets/1", form(1, "11", None)).await;
    assert_eq!(status, StatusCode::SEE_OTHER);
    let (_, violated) = setup.get("/private-sheets/1").await;
    assert!(violated.contains("<strong>Violated</strong>"));
    for (context, html) in [
        ("view", &view),
        ("re-render", &rerender),
        ("history", &history),
        ("violated", &violated),
    ] {
        assert!(!html.contains("<script"), "{context}: raw script tag");
        assert!(!html.contains("<b>"), "{context}: raw option label markup");
        assert_escaped(html, "<script>Label", context);
        assert_escaped(html, "<script>alert(1)</script>", context);
        assert_escaped(html, "<b>High</b>", context);
        // Expression string literal, enum option label and an input echoed by a derived
        // text field (also inside the `value` attribute of the label input).
        assert_escaped(html, "label == \"<script>\"", context);
        assert!(
            escaped("<script>")
                .iter()
                .any(|text| html.contains(&format!("name=\"value-label\" value=\"{text}\""))),
            "{context}: attribute value"
        );
    }
    assert!(escaped("<script>").contains(&derived_text(&view, "echo").to_owned()));
    let template = include_str!("../../templates/private_sheet.html");
    assert!(!template.contains("|safe") && !template.contains("| safe"));
}

#[tokio::test]
async fn viewing_never_writes() {
    let _auth_lock = crate::auth::test_resource_lock().await;
    let _runtime_lock = crate::definitions::TEST_RESOURCE_LOCK.lock().await;
    let setup = setup(&formula_definition()).await;
    setup.create().await;
    let before = setup.stored().await;
    for path in [
        "/private-sheets/1",
        "/private-sheets/1/history/1",
        "/private-sheets/1/history",
        "/private-sheets/1",
    ] {
        assert_eq!(setup.get(path).await.0, StatusCode::OK, "{path}");
    }
    assert_eq!(setup.stored().await, before);
}

#[tokio::test]
async fn busy_runtime_after_a_save_returns_503_with_retry_after() {
    let _auth_lock = crate::auth::test_resource_lock().await;
    let _runtime_lock = crate::definitions::TEST_RESOURCE_LOCK.lock().await;
    let setup = setup(&formula_definition()).await;
    setup.create().await;
    let held = crate::definitions::admit().unwrap();
    let response = router_request(&setup.app, &setup.auth[1], "/private-sheets/1", None).await;
    assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
    assert_eq!(response.headers()["retry-after"], "1");
    drop(held);
    assert_eq!(setup.get("/private-sheets/1").await.0, StatusCode::OK);
}

/// A list sheet whose stored values are valid (JSON within 1 MiB) but whose converted input
/// exceeds the engine's aggregate charge: evaluation refuses the input and the page still
/// renders. Lists are unpublishable until M4; the test publishes through the test-only path.
#[tokio::test]
async fn stored_valid_sheet_with_refused_input_shows_unavailable() {
    let _auth_lock = crate::auth::test_resource_lock().await;
    let _runtime_lock = crate::definitions::TEST_RESOURCE_LOCK.lock().await;
    let mut definition = formula_definition();
    let mut properties: Vec<Value> = (0..31)
        .map(|n| json!({"id": format!("p{n}"), "label": "P", "control": "integer", "optional": false}))
        .collect();
    properties.push(json!({"id": "t", "label": "T", "control": "text", "optional": false}));
    definition["fields"].as_array_mut().unwrap().push(json!({
        "id": "items", "label": "Items", "control": "list", "nullable": false,
        "mapping": mapped(), "items": {"properties": properties}}));
    definition["groups"][0]["fields"]
        .as_array_mut()
        .unwrap()
        .push("items".into());
    let setup = setup(&definition).await;
    let (mut view, permit) = setup
        .store
        .private_create_context(
            setup.auth[1].clone(),
            setup.campaign,
            setup.version,
            crate::definitions::admit().unwrap(),
        )
        .await
        .unwrap();
    let items: Vec<Value> = (0..256)
        .map(|n| {
            let mut item = json!({"id": format!("i-{n}"), "t": "x".repeat(3600)});
            for p in 0..31 {
                item[format!("p{p}")] = 0.into();
            }
            item
        })
        .collect();
    view.snapshot.name = "Unavailable".into();
    view.snapshot.data = json!({"label": "x", "vitality": 4, "luck": 2, "kind": "plain",
                                "items": items});
    assert!(serde_json::to_vec(&view.snapshot.data).unwrap().len() < 1024 * 1024);
    view.immutable
        .as_ref()
        .unwrap()
        .validate_values(&view.snapshot.data)
        .unwrap();
    setup
        .store
        .private_save(setup.auth[1].clone(), view, permit)
        .await
        .unwrap();
    let (status, html) = setup.get("/private-sheets/1").await;
    assert_eq!(status, StatusCode::OK);
    let results = section(&html, "formula-results");
    assert!(results.contains(
        "Derived values unavailable: the saved values exceed the formula input size limit."
    ));
    assert!(results.contains("<code>input_too_large</code>"));
    assert!(!html.contains("id=\"advisory-checks\""));
    assert!(derived_text(&html, "pace").contains("<code>input_too_large</code>"));
}
