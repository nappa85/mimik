//! Lexical retrieval regressions (C1/C3/C4/B6). Synthetic text only.
use mimik::{Store, sources::UnitReview};
use rusqlite::{Connection, params};
use std::path::Path;

#[path = "support/schema_fixture.rs"]
mod schema_fixture;

struct Unit {
    id: i64,
    page: i64,
    html: &'static str,
    state: &'static str,
    correction: &'static str,
}

const fn unit(id: i64, page: i64, html: &'static str) -> Unit {
    Unit {
        id,
        page,
        html,
        state: "accepted",
        correction: "",
    }
}

/// Insert one non-finalized source per (`id`, edition label, optional ruleset) with
/// complete pages and the given units, as a legacy database would contain them.
fn seed(
    db: &Connection,
    source: i64,
    edition: &str,
    ruleset: Option<&str>,
    pages: &[(i64, bool)],
    units: &[Unit],
) {
    db.execute("INSERT INTO sources (id,bundle_hash,source_id,title,system_id,edition,language,pdf_hash,pdf,raw_json,prepared_json,total_pages) VALUES (?1,?2,'synthetic',?3,'mork-borg',?4,'it',printf('synthetic-pdf-%d',?1),x'00','{}','{}',?5)", params![source, format!("bundle-{source}"), format!("Synthetic {source}"), edition, i64::try_from(pages.len()).unwrap()]).unwrap();
    if let Some(edition_id) = ruleset {
        db.execute(
            "INSERT INTO source_rulesets VALUES (?1,'mork-borg',?2)",
            params![source, edition_id],
        )
        .unwrap();
    }
    for (number, (page, complete)) in pages.iter().enumerate() {
        db.execute("INSERT INTO source_pages (id,source_id,pdf_page,png,complete,printed_label) VALUES (?1,?2,?3,x'00',?4,'p')", params![page, source, i64::try_from(number).unwrap() + 1, complete]).unwrap();
    }
    for unit in units {
        let text = html2text::from_read(unit.html.as_bytes(), 120).unwrap();
        db.execute("INSERT INTO source_units (id,page_id,block_id,kind,polygon,raw_html,raw_text,state,correction) VALUES (?1,?2,?3,'Text','[[0,0],[1,0],[1,1],[0,1]]',?4,?5,?6,?7)", params![unit.id, unit.page, format!("/page/0/Text/{}", unit.id), unit.html, text, unit.state, unit.correction]).unwrap();
    }
}

fn open_seeded(dir: &Path, seeder: impl FnOnce(&Connection)) -> Store {
    let path = dir.join("search.sqlite3");
    drop(Store::open(&path).unwrap());
    let db = Connection::open(&path).unwrap();
    // Seed as a pre-search-schema database so reopening exercises the upgrade backfill.
    schema_fixture::drop_search_schema(&db);
    db.execute_batch(&format!(
        "DROP TABLE catalogue_name_fold; DROP TABLE catalogue_entry_names; DROP TABLE catalogue_names_fts; DROP TABLE catalogue_seals; DROP TABLE catalogue_entry_units; DROP TABLE catalogue_entries; DROP TABLE catalogue_entry_types; DROP TABLE catalogue_versions; DROP TABLE catalogues; DROP TABLE catalogue_drafts; DROP TABLE private_sheet_notes; DROP TABLE assistant_downloads; DROP TABLE assistant_model_choice; DROP TABLE assistant_models; PRAGMA user_version={}; PRAGMA foreign_keys=ON;",
        mimik::sources::SEARCH_SCHEMA - 1
    ))
    .unwrap();
    seeder(&db);
    drop(db);
    Store::open(&path).unwrap()
}

async fn texts(store: &Store, query: &str) -> Vec<String> {
    store
        .search_ruleset_evidence("mork-borg".into(), "it-2025".into(), query.into())
        .await
        .unwrap()
        .matches
        .into_iter()
        .map(|evidence| evidence.text)
        .collect()
}

#[tokio::test]
async fn phrase_across_former_wrap_column_matches() {
    // 17 x "parola " is 119 columns, so html2text(120) wraps before "fine".
    const WRAPPED: &str = "<p>parola parola parola parola parola parola parola parola parola parola parola parola parola parola parola parola parola fine della frase</p>";
    let stored = html2text::from_read(WRAPPED.as_bytes(), 120).unwrap();
    assert!(stored.contains("parola\nfine"), "{stored:?}");
    let dir = tempfile::tempdir().unwrap();
    let store = open_seeded(dir.path(), |db| {
        seed(
            db,
            1,
            "MÖRK BORG — IT 2025",
            Some("it-2025"),
            &[(1, true)],
            &[unit(1, 1, WRAPPED)],
        );
    });
    assert_eq!(texts(&store, "\"parola fine della\"").await.len(), 1);
}

#[tokio::test]
async fn narrow_table_cell_phrase_matches() {
    let dir = tempfile::tempdir().unwrap();
    let store = open_seeded(dir.path(), |db| {
        seed(
            db,
            1,
            "MÖRK BORG — IT 2025",
            Some("it-2025"),
            &[(1, true)],
            &[unit(
                1,
                1,
                "<table><tr><th>Arma</th><th>Danno</th><th>Note</th></tr><tr><td>Spada lunga a due mani con elsa pesante e lama di ferro nero forgiata nel nord desolato</td><td>d8 più forza</td><td>richiede entrambe le mani e una prova di forza superiore a dodici per brandirla correttamente</td></tr></table>",
            )],
        );
    });
    assert_eq!(texts(&store, "lama di ferro nero forgiata").await.len(), 1);
}

#[tokio::test]
async fn non_ascii_case_folding_matches() {
    let dir = tempfile::tempdir().unwrap();
    let store = open_seeded(dir.path(), |db| {
        seed(
            db,
            1,
            "MÖRK BORG — IT 2025",
            Some("it-2025"),
            &[(1, true)],
            &[unit(
                1,
                1,
                "<p>La CITTÀ di Galgenbeck e il culto di mörk</p>",
            )],
        );
    });
    assert_eq!(texts(&store, "città").await.len(), 1);
    assert_eq!(texts(&store, "MÖRK").await.len(), 1);
}

#[tokio::test]
async fn italian_diacritics_are_removed() {
    let dir = tempfile::tempdir().unwrap();
    let store = open_seeded(dir.path(), |db| {
        seed(
            db,
            1,
            "MÖRK BORG — IT 2025",
            Some("it-2025"),
            &[(1, true)],
            &[unit(1, 1, "<p>Perché la città è perduta</p>")],
        );
    });
    assert_eq!(texts(&store, "perche citta").await.len(), 1);
}

#[tokio::test]
async fn relevance_ranks_before_page_order() {
    let dir = tempfile::tempdir().unwrap();
    let store = open_seeded(dir.path(), |db| {
        seed(
            db,
            1,
            "MÖRK BORG — IT 2025",
            Some("it-2025"),
            &[(1, true), (2, true)],
            &[
                unit(
                    1,
                    1,
                    "<p>Un lungo paragrafo sulle regole di viaggio, i mercati, le strade e infine il veleno di passaggio tra molte altre parole irrilevanti per la ricerca corrente</p>",
                ),
                unit(
                    2,
                    2,
                    "<p>Veleno: il veleno infligge danni; il veleno dura tre round.</p>",
                ),
            ],
        );
    });
    let found = texts(&store, "veleno").await;
    assert_eq!(found.len(), 2);
    assert!(found[0].starts_with("Veleno:"), "{found:?}");
}

#[tokio::test]
async fn fts_syntax_in_user_queries_is_literal() {
    let dir = tempfile::tempdir().unwrap();
    let store = open_seeded(dir.path(), |db| {
        seed(
            db,
            1,
            "MÖRK BORG — IT 2025",
            Some("it-2025"),
            &[(1, true)],
            &[unit(1, 1, "<p>Forza OR agilità NEAR veleno body: test</p>")],
        );
    });
    for query in [
        "OR",
        "NEAR(forza veleno)",
        "body:forza",
        "\"forza",
        "forza*",
        "^forza",
        "(forza",
        "-forza",
        "{body}: forza",
        "forza AND NOT",
        "a\"\"b",
    ] {
        assert!(
            store
                .search_ruleset_evidence("mork-borg".into(), "it-2025".into(), query.into())
                .await
                .is_ok(),
            "{query}"
        );
    }
    assert_eq!(texts(&store, "NEAR(veleno body:").await.len(), 1);
    assert_eq!(
        texts(&store, "assente OR forza").await,
        Vec::<String>::new()
    );
}

#[tokio::test]
async fn legacy_reviewed_units_are_backfilled_into_the_index() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("search.sqlite3");
    let store = open_seeded(dir.path(), |db| {
        seed(
            db,
            1,
            "MÖRK BORG — IT 2025",
            Some("it-2025"),
            &[(1, true)],
            &[
                unit(1, 1, "<p>Regola antica</p>"),
                Unit {
                    state: "unreviewed",
                    ..unit(2, 1, "<p>Regola grezza</p>")
                },
            ],
        );
    });
    drop(store);
    let db = Connection::open(path).unwrap();
    let indexed: Vec<i64> = db
        .prepare("SELECT rowid FROM source_search_fts ORDER BY rowid")
        .unwrap()
        .query_map([], |row| row.get(0))
        .unwrap()
        .collect::<Result<_, _>>()
        .unwrap();
    assert_eq!(indexed, vec![1]);
}

#[tokio::test]
async fn rejection_and_correction_maintain_the_index() {
    let dir = tempfile::tempdir().unwrap();
    let store = open_seeded(dir.path(), |db| {
        seed(
            db,
            1,
            "MÖRK BORG — IT 2025",
            Some("it-2025"),
            &[(1, true)],
            &[unit(1, 1, "<p>Testo originale sbagliato</p>")],
        );
    });
    let review = |revision, state: &str, correction: &str| UnitReview {
        revision,
        state: state.into(),
        correction: correction.into(),
        actor: "fixture".into(),
        rationale: "Checked synthetic page".into(),
    };
    store
        .review_source_unit(1, 1, review(1, "corrected", "Testo corretto giusto"))
        .await
        .unwrap();
    complete(&store).await;
    assert_eq!(
        texts(&store, "corretto").await,
        vec!["Testo corretto giusto".to_owned()]
    );
    assert_eq!(texts(&store, "sbagliato").await, Vec::<String>::new());
    store
        .review_source_unit(1, 1, review(2, "accepted", ""))
        .await
        .unwrap();
    complete(&store).await;
    assert_eq!(texts(&store, "corretto").await, Vec::<String>::new());
    assert_eq!(texts(&store, "sbagliato").await.len(), 1);
    store
        .review_source_unit(1, 1, review(3, "corrected", "Di nuovo corretto"))
        .await
        .unwrap();
    store
        .review_source_unit(1, 1, review(4, "rejected", ""))
        .await
        .unwrap();
    complete(&store).await;
    assert_eq!(texts(&store, "corretto").await, Vec::<String>::new());
    assert_eq!(texts(&store, "sbagliato").await, Vec::<String>::new());
}

async fn complete(store: &Store) {
    let page = store.source_page(1).await.unwrap();
    store
        .complete_source_page(
            1,
            mimik::sources::PageReview {
                revision: page.revision,
                printed_label: "1".into(),
                actor: "fixture".into(),
                rationale: "Checked synthetic page".into(),
            },
        )
        .await
        .unwrap();
}

fn fts_rows(dir: &Path) -> Vec<(i64, String)> {
    let db = Connection::open(dir.join("search.sqlite3")).unwrap();
    db.prepare("SELECT rowid,body FROM source_search_fts ORDER BY rowid")
        .unwrap()
        .query_map([], |row| Ok((row.get(0)?, row.get(1)?)))
        .unwrap()
        .collect::<Result<_, _>>()
        .unwrap()
}

#[tokio::test]
async fn evidence_carries_unit_identity_and_effective_unwrapped_text() {
    let dir = tempfile::tempdir().unwrap();
    let store = open_seeded(dir.path(), |db| {
        seed(
            db,
            7,
            "MÖRK BORG — IT 2025",
            Some("it-2025"),
            &[(3, true)],
            &[unit(
                9,
                3,
                "<p>parola parola parola parola parola parola parola parola parola parola parola parola parola parola parola parola parola fine</p>",
            )],
        );
    });
    let result = store
        .search_ruleset_evidence("mork-borg".into(), "it-2025".into(), "fine".into())
        .await
        .unwrap();
    assert!(!result.truncated);
    let hit = &result.matches[0];
    assert_eq!(
        (hit.unit_id, hit.page_id, hit.source_id, hit.unit_revision),
        (9, 3, 7, 1)
    );
    assert_eq!(hit.block_id, "/page/0/Text/9");
    assert_eq!(hit.polygon, "[[0,0],[1,0],[1,1],[0,1]]");
    assert_eq!(hit.state, "accepted");
    assert!(
        !hit.text.contains('\n') || hit.text.ends_with('\n') && !hit.text.trim_end().contains('\n'),
        "{:?}",
        hit.text
    );
}

#[tokio::test]
async fn truncation_flag_reports_more_than_fifty() {
    for (count, truncated) in [(50, false), (51, true)] {
        let dir = tempfile::tempdir().unwrap();
        let store = open_seeded(dir.path(), |db| {
            let units: Vec<Unit> = (1..=count)
                .map(|id| unit(id, 1, "<p>regola comune</p>"))
                .collect();
            seed(
                db,
                1,
                "MÖRK BORG — IT 2025",
                Some("it-2025"),
                &[(1, true)],
                &units,
            );
        });
        let result = store
            .search_ruleset_evidence("mork-borg".into(), "it-2025".into(), "comune".into())
            .await
            .unwrap();
        assert_eq!(result.matches.len(), 50);
        assert_eq!(result.truncated, truncated);
    }
}

#[tokio::test]
async fn edition_and_source_set_filters_apply_before_ranking() {
    let dir = tempfile::tempdir().unwrap();
    let store = open_seeded(dir.path(), |db| {
        db.execute("INSERT INTO registry_editions VALUES ('mork-borg','alt-2026','MÖRK BORG — ALT 2026',0)", []).unwrap();
        // The other edition has 60 stronger matches; none may displace or join our hits.
        let strong: Vec<Unit> = (100..160)
            .map(|id| unit(id, 10, "<p>veleno veleno veleno</p>"))
            .collect();
        seed(
            db,
            1,
            "MÖRK BORG — ALT 2026",
            Some("alt-2026"),
            &[(10, true)],
            &strong,
        );
        seed(
            db,
            2,
            "MÖRK BORG — IT 2025",
            Some("it-2025"),
            &[(20, true)],
            &[unit(1, 20, "<p>un veleno debole</p>")],
        );
        seed(
            db,
            3,
            "MÖRK BORG — IT 2025",
            Some("it-2025"),
            &[(30, true)],
            &[unit(2, 30, "<p>altro veleno</p>")],
        );
        seed(
            db,
            4,
            "legacy-label",
            None,
            &[(40, true)],
            &[unit(3, 40, "<p>veleno legacy</p>")],
        );
    });
    let ids = |result: mimik::sources::EvidenceSearch| {
        let mut ids: Vec<i64> = result.matches.iter().map(|hit| hit.unit_id).collect();
        ids.sort_unstable();
        (ids, result.truncated)
    };
    let own = store
        .search_ruleset_evidence("mork-borg".into(), "it-2025".into(), "veleno".into())
        .await
        .unwrap();
    assert_eq!(ids(own), (vec![1, 2], false));
    let set = store
        .search_ruleset_sources(
            "mork-borg".into(),
            "it-2025".into(),
            Some(vec![3, 1]),
            "veleno".into(),
        )
        .await
        .unwrap();
    assert_eq!(ids(set), (vec![2], false));
    let legacy = store
        .search_evidence("mork-borg".into(), "legacy-label".into(), "veleno".into())
        .await
        .unwrap();
    assert_eq!(ids(legacy), (vec![3], false));
    let other = store
        .search_ruleset_evidence("mork-borg".into(), "alt-2026".into(), "veleno".into())
        .await
        .unwrap();
    assert_eq!(other.matches.len(), 50);
    assert!(other.truncated);
    assert!(other.matches.iter().all(|hit| hit.source_id == 1));
    assert!(
        store
            .search_ruleset_sources(
                "mork-borg".into(),
                "it-2025".into(),
                Some(vec![]),
                "veleno".into()
            )
            .await
            .is_err()
    );
}

#[tokio::test]
async fn unreviewed_and_incomplete_units_are_excluded() {
    let dir = tempfile::tempdir().unwrap();
    let store = open_seeded(dir.path(), |db| {
        seed(
            db,
            1,
            "MÖRK BORG — IT 2025",
            Some("it-2025"),
            &[(1, true), (2, false)],
            &[
                Unit {
                    state: "unreviewed",
                    ..unit(1, 1, "<p>segreto grezzo</p>")
                },
                Unit {
                    state: "rejected",
                    ..unit(2, 1, "<p>segreto rifiutato</p>")
                },
                unit(3, 2, "<p>segreto incompleto</p>"),
            ],
        );
    });
    assert_eq!(texts(&store, "segreto").await, Vec::<String>::new());
    let indexed: Vec<i64> = fts_rows(dir.path()).into_iter().map(|row| row.0).collect();
    assert_eq!(indexed, vec![3]);
}

#[tokio::test]
async fn table_cells_are_flattened_so_phrases_can_span_adjacent_cells() {
    let dir = tempfile::tempdir().unwrap();
    let store = open_seeded(dir.path(), |db| {
        seed(
            db,
            1,
            "MÖRK BORG — IT 2025",
            Some("it-2025"),
            &[(1, true)],
            &[unit(
                1,
                1,
                "<table><tr><td>Spada lunga</td><td>d8 danni</td></tr></table>",
            )],
        );
    });
    // Documented limitation: cell boundaries are token separators, not phrase barriers.
    assert_eq!(texts(&store, "\"lunga d8\"").await.len(), 1);
    assert_eq!(
        texts(&store, "\"spada lunga\"").await,
        vec!["Spada lunga | d8 danni\n".to_owned()]
    );
}

#[tokio::test]
async fn completeness_changes_do_not_rewrite_the_index() {
    let dir = tempfile::tempdir().unwrap();
    let store = open_seeded(dir.path(), |db| {
        seed(
            db,
            1,
            "MÖRK BORG — IT 2025",
            Some("it-2025"),
            &[(1, false)],
            &[unit(1, 1, "<p>regola sospesa</p>")],
        );
    });
    let before = fts_rows(dir.path());
    assert_eq!(texts(&store, "sospesa").await, Vec::<String>::new());
    complete(&store).await;
    assert_eq!(fts_rows(dir.path()), before);
    assert_eq!(texts(&store, "sospesa").await.len(), 1);
}

fn oversized() -> &'static str {
    Box::leak(format!("<p>veleno rotto {}</p>", "parola ".repeat(10_000)).into_boxed_str())
}

#[tokio::test]
async fn unsanitizable_legacy_unit_does_not_block_open() {
    let dir = tempfile::tempdir().unwrap();
    let store = open_seeded(dir.path(), |db| {
        seed(
            db,
            1,
            "MÖRK BORG — IT 2025",
            Some("it-2025"),
            &[(1, true)],
            &[unit(
                1,
                1,
                "<table><tr><td colspan=\"0\">x</td></tr></table>",
            )],
        );
    });
    assert_eq!(store.search_index_failures().await.unwrap(), 1);
    assert_eq!(fts_rows(dir.path()), Vec::<(i64, String)>::new());
}

#[tokio::test]
async fn unnormalisable_legacy_unit_is_recorded_and_excluded_without_blocking_open() {
    let dir = tempfile::tempdir().unwrap();
    let store = open_seeded(dir.path(), |db| {
        seed(
            db,
            1,
            "MÖRK BORG — IT 2025",
            Some("it-2025"),
            &[(1, true)],
            &[
                // Over the 64 KiB raw bound, so no search projection can be derived.
                unit(1, 1, oversized()),
                unit(2, 1, "<p>veleno buono</p>"),
            ],
        );
    });
    assert_eq!(store.search_index_failures().await.unwrap(), 1);
    let result = store
        .search_ruleset_evidence("mork-borg".into(), "it-2025".into(), "veleno".into())
        .await
        .unwrap();
    assert_eq!(result.unindexed_units, 1);
    assert_eq!(
        result
            .matches
            .iter()
            .map(|hit| hit.unit_id)
            .collect::<Vec<_>>(),
        vec![2]
    );
    let review = |state: &str, correction: &str| UnitReview {
        revision: 1,
        state: state.into(),
        correction: correction.into(),
        actor: "fixture".into(),
        rationale: "Checked synthetic page".into(),
    };
    let blocked = store.review_source_unit(1, 1, review("accepted", "")).await;
    assert!(
        matches!(&blocked, Err(mimik::AppError::Database(error)) if error.to_string().contains("record a correction or rejection")),
        "{:?}",
        blocked.err()
    );
    // Documented repair: transcribe the unit as a correction.
    store
        .review_source_unit(1, 1, review("corrected", "veleno trascritto"))
        .await
        .unwrap();
    complete(&store).await;
    let repaired = store
        .search_ruleset_evidence("mork-borg".into(), "it-2025".into(), "trascritto".into())
        .await
        .unwrap();
    assert_eq!(repaired.unindexed_units, 0);
    assert_eq!(repaired.matches[0].unit_id, 1);
    drop(store);
    assert!(Store::open(&dir.path().join("search.sqlite3")).is_ok());
}

#[tokio::test]
async fn backfill_streams_more_units_than_one_batch() {
    let dir = tempfile::tempdir().unwrap();
    let store = open_seeded(dir.path(), |db| {
        let units: Vec<Unit> = (1..=300).map(|id| unit(id, 1, "<p>lotto</p>")).collect();
        seed(
            db,
            1,
            "MÖRK BORG — IT 2025",
            Some("it-2025"),
            &[(1, true)],
            &units,
        );
    });
    drop(store);
    assert_eq!(fts_rows(dir.path()).len(), 300);
}

#[tokio::test]
async fn accepted_search_text_is_retained_and_unreviewed_units_delete_cleanly() {
    let dir = tempfile::tempdir().unwrap();
    let store = open_seeded(dir.path(), |db| {
        seed(
            db,
            1,
            "MÖRK BORG — IT 2025",
            Some("it-2025"),
            &[(1, true)],
            &[
                unit(1, 1, "<p>da cancellare</p>"),
                Unit {
                    state: "unreviewed",
                    ..unit(2, 1, "<p>grezzo</p>")
                },
            ],
        );
    });
    drop(store);
    let db = Connection::open(dir.path().join("search.sqlite3")).unwrap();
    db.execute_batch("PRAGMA foreign_keys=ON;").unwrap();
    let error = db
        .execute("DELETE FROM source_search_text WHERE unit_id=1", [])
        .unwrap_err();
    assert!(error.to_string().contains("retained"), "{error}");
    assert!(
        db.execute("DELETE FROM source_search_text WHERE unit_id=2", [])
            .is_ok()
    );
    // Non-finalized units without history can be removed; derived rows follow. Document
    // membership and primary decision history retain the source identity in schema 24.
    db.execute_batch("DELETE FROM source_units WHERE page_id=1;")
        .unwrap();
    assert!(db.execute("DELETE FROM sources WHERE id=1", []).is_err());
    for table in [
        "source_search_text",
        "source_search_fts",
        "source_search_failures",
    ] {
        let count: i64 = db
            .query_row(&format!("SELECT count(*) FROM {table}"), [], |row| {
                row.get(0)
            })
            .unwrap();
        assert_eq!(count, 0, "{table}");
    }
}

#[tokio::test]
async fn raw_html_is_immutable_for_unfinalized_sources_too() {
    let dir = tempfile::tempdir().unwrap();
    let store = open_seeded(dir.path(), |db| {
        seed(
            db,
            1,
            "MÖRK BORG — IT 2025",
            Some("it-2025"),
            &[(1, true)],
            &[Unit {
                state: "unreviewed",
                ..unit(1, 1, "<p>originale</p>")
            }],
        );
    });
    drop(store);
    let db = Connection::open(dir.path().join("search.sqlite3")).unwrap();
    let error = db
        .execute(
            "UPDATE source_units SET raw_html='<p>altro</p>' WHERE id=1",
            [],
        )
        .unwrap_err();
    assert!(error.to_string().contains("immutable"), "{error}");
}

#[tokio::test]
async fn reviewed_units_cannot_bypass_search_text() {
    let dir = tempfile::tempdir().unwrap();
    let store = open_seeded(dir.path(), |db| {
        seed(
            db,
            1,
            "MÖRK BORG — IT 2025",
            Some("it-2025"),
            &[(1, true)],
            &[Unit {
                state: "unreviewed",
                ..unit(1, 1, "<p>x</p>")
            }],
        );
    });
    drop(store);
    let db = Connection::open(dir.path().join("search.sqlite3")).unwrap();
    assert!(db.execute("INSERT INTO source_units (page_id,block_id,kind,polygon,raw_html,raw_text,state) VALUES (1,'b','Text','[]','','','accepted')", []).is_err());
    assert!(
        db.execute("DELETE FROM source_search_text WHERE unit_id=1", [])
            .is_ok()
    );
    assert!(
        db.execute(
            "UPDATE source_units SET state='accepted',revision=2 WHERE id=1",
            []
        )
        .is_err()
    );
}

#[tokio::test]
async fn reopen_repairs_a_tampered_index_and_rebuild_is_idempotent() {
    let dir = tempfile::tempdir().unwrap();
    let store = open_seeded(dir.path(), |db| {
        seed(
            db,
            1,
            "MÖRK BORG — IT 2025",
            Some("it-2025"),
            &[(1, true)],
            &[unit(1, 1, "<p>regola riparata</p>")],
        );
    });
    drop(store);
    let expected = fts_rows(dir.path());
    let db = Connection::open(dir.path().join("search.sqlite3")).unwrap();
    db.execute("DELETE FROM source_search_fts", []).unwrap();
    drop(db);
    let store = Store::open(&dir.path().join("search.sqlite3")).unwrap();
    assert_eq!(fts_rows(dir.path()), expected);
    store.rebuild_search_index().await.unwrap();
    assert_eq!(fts_rows(dir.path()), expected);
    assert_eq!(texts(&store, "riparata").await.len(), 1);
}

#[tokio::test]
async fn exact_words_rank_before_prefix_only_matches() {
    let dir = tempfile::tempdir().unwrap();
    let store = open_seeded(dir.path(), |db| {
        seed(
            db,
            1,
            "MÖRK BORG — IT 2025",
            Some("it-2025"),
            &[(1, true), (2, true)],
            &[
                unit(1, 1, "<p>Un colpo forzato apre la porta</p>"),
                unit(2, 2, "<p>Una prova di forza apre la porta</p>"),
            ],
        );
    });
    let found = texts(&store, "forza").await;
    assert_eq!(found.len(), 2);
    assert!(found[0].contains("forza "), "{found:?}");
}

/// Measurement only (`cargo test --release --test source_search -- --ignored --nocapture`).
#[tokio::test]
#[ignore = "performance measurement; recorded in docs/results.md"]
async fn measure_lookup_latency_on_100k_units() {
    const WORDS: [&str; 24] = [
        "veleno", "forza", "agilità", "presenza", "tenacia", "armatura", "scudo", "spada", "città",
        "perché", "danno", "prova", "round", "difesa", "magia", "rotolo", "pietà", "morte",
        "nemico", "ferita", "di", "il", "la", "e",
    ];
    let dir = tempfile::tempdir().unwrap();
    let store = open_seeded(dir.path(), |db| {
        seed(
            db,
            1,
            "MÖRK BORG — IT 2025",
            Some("it-2025"),
            &[(1, true)],
            &[],
        );
        db.execute_batch("BEGIN").unwrap();
        let mut insert = db.prepare("INSERT INTO source_units (id,page_id,block_id,kind,polygon,raw_html,raw_text,state) VALUES (?1,1,?2,'Text','[]',?3,'',?4)").unwrap();
        let units: i64 = std::env::var("MIMIK_BENCH_UNITS")
            .ok()
            .and_then(|value| value.parse().ok())
            .unwrap_or(100_000);
        for id in 1..=units {
            let text: Vec<&str> = (0..40)
                .map(|n| WORDS[usize::try_from((id * 7 + n * 13 + id / 3 * n) % 24).unwrap()])
                .collect();
            insert
                .execute(params![
                    id,
                    format!("b{id}"),
                    format!("<p>{}</p>", text.join(" ")),
                    if id % 10 == 0 {
                        "unreviewed"
                    } else {
                        "accepted"
                    }
                ])
                .unwrap();
        }
        db.execute_batch("COMMIT").unwrap();
    });
    let mut timings = Vec::new();
    for query in [
        "veleno",
        "di",
        "for",
        "città perché",
        "\"spada danno\"",
        "magia rotolo pietà morte",
        "assente",
    ] {
        for _ in 0..5 {
            let start = std::time::Instant::now();
            let result = store
                .search_ruleset_evidence("mork-borg".into(), "it-2025".into(), query.into())
                .await
                .unwrap();
            timings.push((
                start.elapsed(),
                query,
                result.matches.len(),
                result.truncated,
            ));
        }
    }
    timings.sort();
    let p95 = timings[timings.len() * 95 / 100 - 1];
    println!(
        "max={:?} p95={:?} all={timings:?}",
        timings.last().unwrap(),
        p95
    );
}

const WEAPONS: &str = "<table><thead><tr><th>Arma</th><th>Danno</th><th>Note</th></tr></thead><tbody><tr><td>Spada <b>lunga</b></td><td>d8</td><td></td></tr><tr><td>Ascia</td><td>d6</td><td>a una mano<br>o due</td></tr></tbody></table>";

#[tokio::test]
async fn table_rows_are_single_lines_and_two_facts_of_a_row_share_a_line() {
    let dir = tempfile::tempdir().unwrap();
    let store = open_seeded(dir.path(), |db| {
        seed(
            db,
            1,
            "MÖRK BORG — IT 2025",
            Some("it-2025"),
            &[(1, true)],
            &[unit(1, 1, WEAPONS)],
        );
    });
    let found = texts(&store, "spada d8").await;
    assert_eq!(found.len(), 1);
    let lines: Vec<&str> = found[0].lines().collect();
    assert_eq!(
        lines,
        vec![
            "Arma | Danno | Note",
            "Spada lunga | d8 | ",
            "Ascia | d6 | a una mano o due"
        ]
    );
    assert!(
        lines
            .iter()
            .any(|l| l.contains("Spada") && l.contains("d8"))
    );
    assert!(
        !lines
            .iter()
            .any(|l| l.contains("Spada") && l.contains("d6"))
    );
}

#[tokio::test]
async fn reindex_after_a_normaliser_bump_rebuilds_old_rows_only_in_value() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("search.sqlite3");
    let store = open_seeded(dir.path(), |db| {
        seed(
            db,
            1,
            "MÖRK BORG — IT 2025",
            Some("it-2025"),
            &[(1, true)],
            &[
                unit(1, 1, WEAPONS),
                unit(2, 1, "<p>Regola <b>antica</b></p>"),
            ],
        );
    });
    drop(store);
    // Rewind to the previous normaliser: one cell per line for the table.
    let db = Connection::open(&path).unwrap();
    db.execute(
        "UPDATE source_search_text SET normaliser=1,search_text=CASE unit_id WHEN 1 THEN 'Arma\nDanno\nNote\nSpada lunga\nd8\n' ELSE search_text END",
        [],
    )
    .unwrap();
    drop(db);
    let store = Store::open(&path).unwrap();
    let db = Connection::open(&path).unwrap();
    let rows: Vec<(i64, i64, String)> = db
        .prepare("SELECT unit_id,normaliser,search_text FROM source_search_text ORDER BY unit_id")
        .unwrap()
        .query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))
        .unwrap()
        .collect::<Result<_, _>>()
        .unwrap();
    assert_eq!(rows.len(), 2);
    assert!(rows.iter().all(|r| r.1 == 2));
    assert!(
        rows[0]
            .2
            .starts_with("Arma | Danno | Note\nSpada lunga | d8 | \n")
    );
    assert_eq!(rows[1].2, "Regola antica\n");
    let fts = fts_rows(dir.path());
    assert_eq!(fts.len(), 2);
    assert_eq!(fts[0].1, rows[0].2);
    assert_eq!(fts[1].1, rows[1].2);
    assert_eq!(texts(&store, "ascia d6").await.len(), 1);
}
