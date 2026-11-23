//! Generated synthetic data; ordinary tests use fake workers, ignored live tests use the host runner.
use super::*;
use crate::sources::staging::{self, MIB};
use crate::{AppError, Store, auth::RequestAuth};
use rusqlite::params;
use std::os::unix::fs::MetadataExt;
use std::{
    io::Write,
    sync::{
        Arc,
        atomic::{AtomicBool, AtomicUsize, Ordering},
    },
    time::{Duration, Instant},
};

const PDF: &[u8] = b"%PDF-1.7\nsynthetic spool; the fake inspects without parsing\n";
const ID: &str = "11111111111111111111111111111111";

/// Manual boxes on pages 1-3 (nothing confirmed from a proposal).
fn exclusion() -> exclusions::ExclusionRequest {
    exclusions::ExclusionRequest {
        manual: (1..=3).map(|n| (n, vec![[1., 2., 4., 5.]])).collect(),
        rationale: "RATIONALE-PRIVATE-MARKER".into(),
        ..Default::default()
    }
}
/// The confirmation of one proposal, by its fingerprint.
fn confirming(fingerprint: &str) -> exclusions::ExclusionRequest {
    exclusions::ExclusionRequest {
        confirmed: vec![exclusions::Confirmation {
            fingerprint: fingerprint.into(),
            ..Default::default()
        }],
        rationale: "RATIONALE-PRIVATE-MARKER".into(),
        ..Default::default()
    }
}

#[test]
fn exclusion_confirmation_proposal_does_not_hold_store_mutex() {
    let (_dir, m, o, _) = fixture();
    let id = create(&m, &o);
    selected(&m, &o, &id, &[1, 2, 3]);
    start(&m, &o, &id);
    wait(&m);
    let revision = m.read(&id).unwrap().revision;
    let e = confirming(&m.propose_exclusions(&o.auth, &id, revision).unwrap()[0].fingerprint);
    let store = m.0.store.clone();
    exclusions::PROPOSAL_HOOK.with(|hook| {
        *hook.borrow_mut() = Some(Box::new(move || {
            let (tx, rx) = std::sync::mpsc::channel();
            std::thread::spawn(move || {
                store
                    .upload_storage(|db| {
                        db.query_row("SELECT 1", [], |_| Ok(()))?;
                        Ok(())
                    })
                    .unwrap();
                let _ = tx.send(());
            });
            assert!(
                rx.recv_timeout(Duration::from_secs(2)).is_ok(),
                "Store user blocked during proposal computation"
            );
        }));
    });
    m.set_exclusion(&o, &id, revision, 0, &e).unwrap();
}

#[test]
fn exclusion_fingerprints_are_upload_private_and_discard_removes_key() {
    let (_dir, m, o, _) = fixture();
    let mut fingerprints = Vec::new();
    for _ in 0..2 {
        let id = create(&m, &o);
        selected(&m, &o, &id, &[1, 2, 3]);
        start(&m, &o, &id);
        wait(&m);
        let revision = m.read(&id).unwrap().revision;
        let first = m.propose_exclusions(&o.auth, &id, revision).unwrap();
        let second = m.propose_exclusions(&o.auth, &id, revision).unwrap();
        assert_eq!(first[0].fingerprint, second[0].fingerprint);
        assert_ne!(first[0].fingerprint, hash(first[0].text.as_bytes()));
        let key =
            m.0.store
                .upload_storage(|db| {
                    let key: Vec<u8> = db.query_row(
                        "SELECT fingerprint_key FROM source_uploads WHERE id=?1",
                        [&id],
                        |r| r.get(0),
                    )?;
                    assert_eq!(key.len(), 32);
                    Ok(key)
                })
                .unwrap();
        assert!(
            !serde_json::to_string(&m.contract(&id).unwrap())
                .unwrap()
                .contains(&hex::encode(key))
        );
        fingerprints.push(first[0].fingerprint.clone());
        m.discard(&o, &id).unwrap();
        m.0.store
            .upload_storage(|db| {
                assert_eq!(
                    db.query_row("SELECT count(*) FROM source_uploads", [], |r| r
                        .get::<_, i64>(0))?,
                    0
                );
                Ok(())
            })
            .unwrap();
    }
    assert_ne!(fingerprints[0], fingerprints[1]);
}

#[test]
fn exclusion_contract_refuses_a_missing_deselected_rendering() {
    let (_dir, m, o, _) = fixture();
    let id = create(&m, &o);
    selected(&m, &o, &id, &[1, 2, 3]);
    start(&m, &o, &id);
    wait(&m);
    let revision = m.read(&id).unwrap().revision;
    m.set_exclusion(&o, &id, revision, 0, &exclusion()).unwrap();
    m.0.store
        .upload_storage(|db| {
            db.execute(
                "DELETE FROM source_upload_pages WHERE upload_id=?1 AND pdf_page=3",
                [&id],
            )?;
            db.execute(
                "UPDATE source_uploads SET selected_pages='[1,2]' WHERE id=?1",
                [&id],
            )?;
            Ok(())
        })
        .unwrap();
    assert!(m.contract(&id).is_err());
}

#[test]
fn exclusion_propose_refuses_queued_and_running_jobs() {
    let (_dir, m, o, _) = fixture();
    let id = create(&m, &o);
    selected(&m, &o, &id, &[1, 2, 3]);
    start(&m, &o, &id);
    wait(&m);
    let revision = m.read(&id).unwrap().revision;
    for state in ["queued", "running"] {
        m.0.store
            .upload_storage(|db| {
                db.execute(
                    "UPDATE source_jobs SET state=?2 WHERE upload_id=?1",
                    params![id, state],
                )?;
                Ok(())
            })
            .unwrap();
        assert!(matches!(
            m.propose_exclusions(&o.auth, &id, revision),
            Err(AppError::Conflict)
        ));
    }
}

#[test]
fn exclusion_revision_replace_remove_concurrent_writes_and_cascade() {
    let (_dir, m, o, _) = fixture();
    let id = create(&m, &o);
    selected(&m, &o, &id, &[1, 2, 3]);
    start(&m, &o, &id);
    wait(&m);
    let original_png = m.0.files.read_image(&id, 1).unwrap();
    let revision = m.read(&id).unwrap().revision;
    assert!(matches!(
        m.set_exclusion(&o, &id, revision - 1, 0, &exclusion()),
        Err(AppError::Conflict)
    ));
    let mut writers = Vec::new();
    for _ in 0..2 {
        let m = m.clone();
        let o = o.clone();
        let id = id.clone();
        writers.push(std::thread::spawn(move || {
            m.set_exclusion(&o, &id, revision, 0, &exclusion())
        }));
    }
    assert_eq!(
        writers
            .into_iter()
            .filter(|w| w.thread().id() != std::thread::current().id())
            .map(|w| w.join().unwrap().is_ok())
            .filter(|ok| *ok)
            .count(),
        1
    );
    let first = m.read(&id).unwrap();
    assert_eq!(first.exclusion_revision, 1);
    assert_eq!(first.revision, revision);
    assert!(m.remove_exclusion(&o, &id, revision, 0, "stale").is_err());
    let mut replacement = exclusion();
    replacement.manual.get_mut(&1).unwrap()[0][2] = 6.;
    replacement.rationale = "replacement".into();
    m.set_exclusion(&o, &id, revision, 1, &replacement).unwrap();
    assert_eq!(
        m.contract(&id).unwrap().exclusion.unwrap().boxes[&1],
        [[1., 2., 6., 5.]]
    );
    assert!(m.set(&o, &id, revision, &metadata(), &[1, 2]).is_err());
    assert!(matches!(
        m.set(&o, &id, revision - 1, &metadata(), &[1, 2]),
        Err(AppError::Conflict)
    ));
    m.remove_exclusion(&o, &id, revision, 2, "removed").unwrap();
    assert_eq!(m.read(&id).unwrap().exclusion_revision, 3);
    assert!(m.contract(&id).unwrap().exclusion.is_none());
    assert_eq!(m.0.files.read_image(&id, 1).unwrap(), original_png);
    m.0.store
        .upload_storage(|db| {
            assert_eq!(
                db.query_row("SELECT count(*) FROM source_upload_exclusions", [], |r| r
                    .get::<_, i64>(
                    0
                ))?,
                3
            );
            Ok(())
        })
        .unwrap();
    m.discard(&o, &id).unwrap();
    m.0.store
        .upload_storage(|db| {
            assert_eq!(
                db.query_row("SELECT count(*) FROM source_upload_exclusions", [], |r| r
                    .get::<_, i64>(
                    0
                ))?,
                0
            );
            Ok(())
        })
        .unwrap();
}

#[test]
fn exclusion_proposal_is_current_hash_verified_and_extension_is_explicit() {
    let (dir, m, o, _) = fixture();
    let id = create(&m, &o);
    selected(&m, &o, &id, &[1, 2, 3, 4]);
    start(&m, &o, &id);
    wait(&m);
    let text = dir.path().join("staging").join(&id).join("text-4.json");
    std::fs::write(&text, b"[]").unwrap();
    m.0.store.upload_storage(|db| {
        db.execute("UPDATE source_upload_pages SET text_sha256=?2,text_bytes=2 WHERE upload_id=?1 AND pdf_page=4",
            params![id, hash(b"[]")])?; Ok(())
    }).unwrap();
    let revision = m.read(&id).unwrap().revision;
    let proposals = m.propose_exclusions(&o.auth, &id, revision).unwrap();
    assert_eq!(proposals.len(), 1);
    assert_eq!(proposals[0].boxes.len(), 3);
    let mut e = confirming(&proposals[0].fingerprint);
    e.confirmed[0].absent.push(4);
    assert!(m.set_exclusion(&o, &id, revision, 0, &e).is_err());
    e.extend_absent = true;
    m.set_exclusion(&o, &id, revision, 0, &e).unwrap();
    let stored = m.read(&id).unwrap().exclusion.unwrap();
    assert_eq!(stored.pages, [1, 2, 3, 4]);
    assert_eq!(
        stored.proposal_fingerprints,
        [proposals[0].fingerprint.clone()]
    );
    assert_eq!(m.read(&id).unwrap().exclusion_revision, 1);
    std::fs::write(text, b"[PRIVATE-TEXT-MARKER]").unwrap();
    let error = m.propose_exclusions(&o.auth, &id, revision).err().unwrap();
    assert!(!error.to_string().contains("PRIVATE-TEXT-MARKER"));
    assert!(!format!("{error:?}").contains("PRIVATE-TEXT-MARKER"));
    let corrupt = b"[PRIVATE-TEXT-MARKER]";
    m.0.store.upload_storage(|db| {
        db.execute("UPDATE source_upload_pages SET text_sha256=?2,text_bytes=?3 WHERE upload_id=?1 AND pdf_page=4",
            params![id, hash(corrupt), corrupt.len()])?; Ok(())
    }).unwrap();
    let error = m.propose_exclusions(&o.auth, &id, revision).err().unwrap();
    assert!(!error.to_string().contains("PRIVATE-TEXT-MARKER"));
    assert!(!format!("{error:?}").contains("PRIVATE-TEXT-MARKER"));
    assert!(m.propose_exclusions(&o.auth, &id, revision - 1).is_err());
}

#[test]
fn exclusion_validates_geometry_authority_and_personal_data_stays_in_its_row() {
    let (dir, m, o, _) = fixture();
    let id = create(&m, &o);
    selected(&m, &o, &id, &[1, 2, 3]);
    start(&m, &o, &id);
    wait(&m);
    let revision = m.read(&id).unwrap().revision;
    let invalid = OwnerWrite {
        auth: o.auth.clone(),
        csrf: o.auth.stale_csrf_fixture(),
    };
    assert!(
        m.set_exclusion(&invalid, &id, revision, 0, &exclusion())
            .is_err()
    );
    assert!(
        m.remove_exclusion(&invalid, &id, revision, 0, "private")
            .is_err()
    );
    for bounds in [
        [f64::NAN, 0., 1., 1.],
        [-1., 0., 1., 1.],
        [1., 1., 1., 2.],
        [0., 0., 11., 1.],
        [0., 0., 1., 11.],
    ] {
        let mut e = exclusion();
        e.manual.insert(1, vec![bounds]);
        let error = m.set_exclusion(&o, &id, revision, 0, &e).unwrap_err();
        assert!(!error.to_string().contains(&e.rationale));
    }
    let mut forged = confirming("PRIVATE-TEXT-MARKER");
    forged.extend_absent = true;
    let error = m.set_exclusion(&o, &id, revision, 0, &forged).unwrap_err();
    assert!(!format!("{error:?}").contains("PRIVATE-TEXT-MARKER"));
    let marker = "PRIVATE-TEXT-MARKER";
    let bytes = worker::records::json(&vec![worker::records::TextLine {
        text: marker.into(),
        bounds: [1., 1., 3., 3.],
    }])
    .unwrap();
    for n in 1..=3 {
        std::fs::write(
            dir.path()
                .join("staging")
                .join(&id)
                .join(format!("text-{n}.json")),
            &bytes,
        )
        .unwrap();
    }
    m.0.store
        .upload_storage(|db| {
            db.execute(
                "UPDATE source_upload_pages SET text_sha256=?2,text_bytes=?3 WHERE upload_id=?1",
                params![id, hash(&bytes), bytes.len()],
            )?;
            Ok(())
        })
        .unwrap();
    let proposals = m.propose_exclusions(&o.auth, &id, revision).unwrap();
    assert_eq!(proposals[0].text, "private-text-marker");
    let e = confirming(&proposals[0].fingerprint);
    m.set_exclusion(&o, &id, revision, 0, &e).unwrap();
    assert_private_upload_rows(&m);
}

fn assert_private_upload_rows(m: &Uploads) {
    m.0.store
        .upload_storage(|db| {
            let tables = db
                .prepare("SELECT name FROM sqlite_schema WHERE type='table'")?
                .query_map([], |r| r.get::<_, String>(0))?
                .collect::<Result<Vec<_>, _>>()?;
            let mut rationales = 0;
            for table in tables {
                let mut q =
                    db.prepare(&format!("SELECT * FROM \"{}\"", table.replace('"', "\"\"")))?;
                let columns = q.column_count();
                let mut rows = q.query([])?;
                while let Some(row) = rows.next()? {
                    for column in 0..columns {
                        let value = match row.get_ref(column)? {
                            rusqlite::types::ValueRef::Text(bytes)
                            | rusqlite::types::ValueRef::Blob(bytes) => {
                                String::from_utf8_lossy(bytes).into_owned()
                            }
                            other => format!("{other:?}"),
                        };
                        assert!(!value.to_lowercase().contains("private-text-marker"));
                        assert!(!value.contains("BUYER-MARKER-NAME"));
                        assert!(
                            !value
                                .to_lowercase()
                                .contains("worker-stderr-planted-marker")
                        );
                        if value.contains("RATIONALE-PRIVATE-MARKER") {
                            assert_eq!(table, "source_upload_exclusions");
                            rationales += 1;
                        }
                    }
                }
            }
            assert_eq!(rationales, 1);
            Ok(())
        })
        .unwrap();
}

#[test]
fn exclusion_private_markers_do_not_escape_stdout_stderr() {
    // Capture the ordinary test harness and application stderr, including diagnostics from
    // malformed staged text and refused writes. Each child runs exactly one marker fixture.
    let mut filters = vec![
        "sources::uploads::tests::exclusion_validates_geometry_authority_and_personal_data_stays_in_its_row",
        "sources::uploads::tests::exclusion_proposal_is_current_hash_verified_and_extension_is_explicit",
        "sources::uploads::tests::exclusion_stderr_helper_discards_markers_and_maps_exit",
        "auth::tests::upload_route_tests::upload_route_filename_not_stored_and_actions_admitted",
    ];
    if cfg!(feature = "dev-endpoint") {
        filters.push("auth::tests::upload_route_tests::upload_route_propose_confirm_adjust_remove_displayed_boxes");
    }
    for filter in filters {
        let output = std::process::Command::new(std::env::current_exe().unwrap())
            .args([filter, "--exact", "--nocapture"])
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "Private-data fixture subprocess failed."
        );
        assert!(
            String::from_utf8_lossy(&output.stdout)
                .lines()
                .any(|line| line.starts_with("test result: ok. 1 passed;")),
            "Private-data fixture did not run exactly one test."
        );
        for capture in [&output.stdout, &output.stderr] {
            let text = String::from_utf8_lossy(capture).to_lowercase();
            for marker in [
                "private-text-marker",
                "buyer-marker-name",
                "rationale-private-marker",
                "worker-stderr-planted-marker",
            ] {
                assert!(
                    !text.contains(marker),
                    "Private marker escaped into log/stderr capture."
                );
            }
        }
    }
}

#[test]
fn exclusion_history_expires_with_its_upload() {
    let (_dir, m, o, _) = fixture();
    let id = create(&m, &o);
    selected(&m, &o, &id, &[1, 2, 3]);
    start(&m, &o, &id);
    wait(&m);
    m.set_exclusion(&o, &id, m.read(&id).unwrap().revision, 0, &exclusion())
        .unwrap();
    m.0.store
        .upload_storage(|db| {
            db.execute(
                "UPDATE source_uploads SET created_at=1,updated_at=1 WHERE id=?1",
                [&id],
            )?;
            Ok(())
        })
        .unwrap();
    m.sweep().unwrap();
    assert!(m.read(&id).is_err());
    m.0.store
        .upload_storage(|db| {
            assert_eq!(
                db.query_row("SELECT count(*) FROM source_upload_exclusions", [], |r| r
                    .get::<_, i64>(
                    0
                ))?,
                0
            );
            Ok(())
        })
        .unwrap();
}

#[test]
fn exclusion_stderr_helper_discards_markers_and_maps_exit() {
    // Helper-level drain/exit mapping through a fake Worker, not runner launch coverage.
    struct StderrWorker(bool);
    impl Worker for StderrWorker {
        fn inspect(&self, _: &Path, _: &Pins, _: &Cancel) -> Result<Inspection, Reason> {
            worker::stderr_fixture(self.0)?;
            Ok(inspection())
        }
        fn render(
            &self,
            _: &Path,
            _: &Pins,
            _: &[Page],
            _: &Cancel,
            _: &mut (dyn FnMut(RenderedPage) -> Result<(), worker::Error> + Send),
        ) -> Result<(), Reason> {
            unreachable!()
        }
    }
    for success in [true, false] {
        let (_dir, m, o) = fixture_worker(Arc::new(StderrWorker(success)));
        let id = create(&m, &o);
        let u = m.read(&id).unwrap();
        assert_eq!(u.job.state, if success { "succeeded" } else { "failed" });
        m.0.store
            .upload_storage(|db| {
                let diagnostic: Option<String> = db.query_row(
                    "SELECT reason FROM source_jobs WHERE upload_id=?1",
                    [&id],
                    |r| r.get(0),
                )?;
                assert_eq!(
                    diagnostic.as_deref(),
                    if success { None } else { Some("unreadable") }
                );
                Ok(())
            })
            .unwrap();
    }
}

#[test]
fn rotated_displayed_exclusion_round_trips_without_applying_transform_twice() {
    let (_dir, m, o, _) = fixture();
    let id = create(&m, &o);
    let mut inspection = inspection();
    inspection.pages[0].rotate = 90;
    inspection.pages[0].transform = [0., 1., 1., 0., 0., 0.];
    m.0.store
        .upload_storage(|db| {
            db.execute(
                "UPDATE source_uploads SET inspection=?2 WHERE id=?1",
                params![id, json(&InspectionData::from(inspection))?],
            )?;
            Ok(())
        })
        .unwrap();
    selected(&m, &o, &id, &[1]);
    start(&m, &o, &id);
    wait(&m);
    let mut e = exclusion();
    e.manual.retain(|n, _| *n == 1);
    let revision = m.read(&id).unwrap().revision;
    m.set_exclusion(&o, &id, revision, 0, &e).unwrap();
    let contract = m.contract(&id).unwrap();
    assert_eq!(contract.pages[0].transform, [0., 1., 1., 0., 0., 0.]);
    assert_eq!(contract.exclusion.unwrap().boxes[&1], e.manual[&1]);
}

pub(crate) fn assert_rendered_exclusion_round_trip(inspection: Inspection, rendered: RenderedPage) {
    struct Synthetic {
        inspection: Inspection,
        rendered: Mutex<Option<RenderedPage>>,
    }
    impl Worker for Synthetic {
        fn inspect(&self, _: &Path, _: &Pins, _: &Cancel) -> Result<Inspection, Reason> {
            Ok(self.inspection.clone())
        }
        fn render(
            &self,
            _: &Path,
            _: &Pins,
            _: &[Page],
            _: &Cancel,
            publish: &mut (dyn FnMut(RenderedPage) -> Result<(), worker::Error> + Send),
        ) -> Result<(), Reason> {
            publish(self.rendered.lock().unwrap().take().unwrap())?;
            Ok(())
        }
    }
    let fact = inspection.pages[0].clone();
    let expected = [
        fact.width / 10.,
        fact.height / 8.,
        fact.width / 3.,
        fact.height / 2.,
    ];
    let worker = Synthetic {
        inspection,
        rendered: Mutex::new(Some(rendered)),
    };
    let (_dir, m, o) = fixture_worker(Arc::new(worker));
    let id = create(&m, &o);
    selected(&m, &o, &id, &[1]);
    start(&m, &o, &id);
    wait(&m);
    let e = exclusions::ExclusionRequest {
        manual: [(1, vec![expected])].into(),
        rationale: "Synthetic rotated page".into(),
        ..Default::default()
    };
    m.set_exclusion(&o, &id, m.read(&id).unwrap().revision, 0, &e)
        .unwrap();
    // Stored rounded outward to 0.01 pt: it covers the box and exceeds it by less than 0.01.
    let covers = |stored: &[[f64; 4]]| {
        let [b] = stored else { panic!("one box") };
        b[0] <= expected[0]
            && b[1] <= expected[1]
            && b[2] >= expected[2]
            && b[3] >= expected[3]
            && expected[0] - b[0] < 0.01
            && expected[1] - b[1] < 0.01
            && b[2] - expected[2] < 0.01
            && b[3] - expected[3] < 0.01
    };
    let u = m.read(&id).unwrap();
    assert!(covers(&u.exclusion.unwrap().boxes[&1]));
    let contract = m.contract(&id).unwrap();
    assert_eq!(contract.pages[0].transform, fact.transform);
    assert!(covers(&contract.exclusion.unwrap().boxes[&1]));
}

fn live_repeated_text(environment: &str, require_proposal: bool) {
    let path =
        std::env::var_os(environment).expect("Set the local PDF path for this ignored check.");
    let selection = std::env::var("MIMIK_3C_LIVE_PAGES")
        .expect("Set MIMIK_3C_LIVE_PAGES to selected PDF page numbers, separated by commas.");
    let pages = selection
        .split(',')
        .map(|p| p.parse::<u32>().expect("Invalid page number."))
        .collect::<Vec<_>>();
    let bytes = crate::local_file::read_path(Path::new(&path), 128 * 1024 * 1024).unwrap();
    let (_dir, m, o) = fixture_worker(Arc::new(PdfWorker));
    let mut phase = m.begin_upload().unwrap();
    phase.file().write_all(&bytes).unwrap();
    let id = m
        .create(&o, phase, &hash(&bytes), bytes.len() as u64, &metadata())
        .unwrap();
    wait(&m);
    selected(&m, &o, &id, &pages);
    start(&m, &o, &id);
    wait(&m);
    let u = m.read(&id).unwrap();
    let proposals = m.propose_exclusions(&o.auth, &id, u.revision).unwrap();
    if require_proposal {
        assert!(!proposals.is_empty(), "No repeated-text proposal.");
    }
    // Never print text, filename, rationale, fingerprint or worker diagnostics.
    println!("selected page count: {}", pages.len());
    for proposal in proposals {
        println!(
            "affected page count: {}; boxes: {:?}",
            proposal.boxes.len(),
            proposal.boxes
        );
    }
    m.discard(&o, &id).unwrap();
}

#[test]
#[ignore = "coordinator-only local PDF and confinement: MIMIK_IT_BUYER_PDF, MIMIK_3C_LIVE_PAGES"]
fn live_repeated_text_it_buyer_boxes_only() {
    live_repeated_text("MIMIK_IT_BUYER_PDF", true);
}

#[test]
#[ignore = "coordinator-only local PDF and confinement: MIMIK_BARE_BONES_PDF, MIMIK_3C_LIVE_PAGES"]
fn live_repeated_text_bare_bones_boxes_only() {
    live_repeated_text("MIMIK_BARE_BONES_PDF", false);
}

#[test]
fn staged_contract_includes_confirmed_displayed_exclusion() {
    let (_dir, m, o, _) = fixture();
    let id = create(&m, &o);
    selected(&m, &o, &id, &[1]);
    start(&m, &o, &id);
    wait(&m);
    m.write(&o, |tx| {
        tx.execute(
            "INSERT INTO source_upload_exclusions
             (upload_id,revision,actor,actor_id,actor_epoch,session_digest,rationale,pages,boxes,created_at)
             VALUES(?1,1,'account:1',1,1,?2,'Synthetic rationale','[1]',
                    '{\"1\":[[1,2,3,4]]}',?3)",
            params![id, o.auth.digest(), now()?],
        )?;
        Ok(())
    }).unwrap();
    assert!(m.contract(&id).unwrap().exclusion.is_some());
}

fn metadata() -> Metadata {
    Metadata {
        system_id: "mork-borg".into(),
        edition: "Synthetic".into(),
        language: "en".into(),
    }
}
fn page(n: u32) -> Page {
    Page {
        n,
        width: 10.,
        height: 10.,
        rotate: 0,
        user_unit: 1.,
        transform: [1., 0., 0., -1., 0., 10.],
        visible: [0., 0., 10., 10.],
        tagged: false,
    }
}
fn inspection() -> Inspection {
    Inspection {
        document: worker::records::Document {
            pages: 70,
            encrypted: false,
            mupdf: "1.27.2".into(),
        },
        pages: (1..=70).map(page).collect(),
    }
}
fn rendered(n: u32) -> RenderedPage {
    let (width, height) = worker::pixel_size(10., 10.).unwrap();
    let mut png = Vec::new();
    {
        let mut encoder = png::Encoder::new(&mut png, width, height);
        encoder.set_color(png::ColorType::Rgb);
        encoder.set_depth(png::BitDepth::Eight);
        encoder
            .write_header()
            .unwrap()
            .write_image_data(&vec![255; usize::try_from(width * height * 3).unwrap()])
            .unwrap();
    }
    RenderedPage {
        n,
        png,
        width_px: width,
        height_px: height,
        dpi: 150,
        stats: worker::records::Stats {
            n,
            chars: 4,
            unmapped: 0,
            image_area: 0.,
            shading_area: 0.0,
            fonts_substituted: vec!["FixtureFont".into()],
            fonts_skipped: 0,
            incomplete: false,
            dropped_lines: 0,
        },
        lines: vec![worker::records::TextLine {
            text: "test".into(),
            bounds: [1., 1., 3., 3.],
        }],
    }
}
#[derive(Default)]
struct Fake {
    calls: AtomicUsize,
    hold: AtomicBool,
    entered: AtomicBool,
    fail: AtomicBool,
    inspect_failure: AtomicBool,
    panic: AtomicBool,
    /// Panics inside `render` after the first page was published.
    panic_after_page: AtomicBool,
    large: AtomicBool,
    /// When nonzero, inspection reports this many pages (a stand-in for a very large PDF).
    inspected_pages: std::sync::atomic::AtomicU32,
    /// A second repeated text ("second mark") on every rendered page, at these bounds.
    extra_line: Mutex<Option<exclusions::BoxPoints>>,
    /// Runs inside the worker call, after it has produced its output: a stand-in for a change
    /// to the staged file while a worker has it open.
    during_run: Mutex<Option<Box<dyn Fn() + Send>>>,
}
impl Fake {
    fn change_during_run(&self) {
        if let Some(change) = self.during_run.lock().unwrap().as_ref() {
            change();
        }
    }
}
impl Worker for Fake {
    fn inspect(&self, _: &std::path::Path, _: &Pins, _: &Cancel) -> Result<Inspection, Reason> {
        self.change_during_run();
        if self.inspect_failure.load(Ordering::Acquire) {
            return Err(worker::Error::Unsupported.into());
        }
        let count = self.inspected_pages.load(Ordering::Acquire);
        if count > 0 {
            return Ok(Inspection {
                document: worker::records::Document {
                    pages: count,
                    encrypted: false,
                    mupdf: "1.27.2".into(),
                },
                pages: (1..=count).map(page).collect(),
            });
        }
        Ok(inspection())
    }
    fn render(
        &self,
        _: &std::path::Path,
        _: &Pins,
        pages: &[Page],
        cancel: &Cancel,
        publish: &mut (dyn FnMut(RenderedPage) -> Result<(), worker::Error> + Send),
    ) -> Result<(), Reason> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        assert!(!self.panic.load(Ordering::Acquire), "injected worker panic");
        self.entered.store(true, Ordering::Release);
        while self.hold.load(Ordering::Acquire) {
            if cancel.cancelled() {
                return Err(Reason::Cancelled);
            }
            std::thread::sleep(Duration::from_millis(1));
        }
        for p in pages {
            if cancel.cancelled() {
                return Err(Reason::Cancelled);
            }
            let mut rendered = rendered(p.n);
            if let Some(bounds) = *self.extra_line.lock().unwrap() {
                rendered.lines.push(worker::records::TextLine {
                    text: "second mark".into(),
                    bounds,
                });
            }
            if self.large.load(Ordering::Acquire) {
                let mut data = vec![0; 1800 * 1800 * 3];
                getrandom::fill(&mut data).unwrap();
                rendered.png.clear();
                let mut encoder = png::Encoder::new(&mut rendered.png, 1800, 1800);
                encoder.set_color(png::ColorType::Rgb);
                encoder.set_depth(png::BitDepth::Eight);
                encoder
                    .write_header()
                    .unwrap()
                    .write_image_data(&data)
                    .unwrap();
                rendered.width_px = 1800;
                rendered.height_px = 1800;
            }
            publish(rendered)?;
            assert!(
                !self.panic_after_page.load(Ordering::Acquire),
                "injected panic after a published page"
            );
            if self.fail.load(Ordering::Acquire) {
                self.change_during_run();
                return Err(worker::Error::Unreadable.into());
            }
        }
        self.change_during_run();
        Ok(())
    }
}
fn fixture() -> (tempfile::TempDir, Uploads, OwnerWrite, Arc<Fake>) {
    let fake = Arc::new(Fake::default());
    let (dir, manager, owner) = fixture_worker(fake.clone());
    (dir, manager, owner, fake)
}
fn fixture_worker(worker: Arc<dyn Worker>) -> (tempfile::TempDir, Uploads, OwnerWrite) {
    fixture_worker_data(worker, None)
}
fn fixture_worker_data(
    worker: Arc<dyn Worker>,
    data: Option<&Path>,
) -> (tempfile::TempDir, Uploads, OwnerWrite) {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::open(&dir.path().join("db")).unwrap();
    let session = tower_sessions::session::Id::default();
    store.upload_storage(|db| {
        crate::store::register_auth_clock(db,Arc::new(||Ok(100)))?;
        db.execute("INSERT INTO accounts(id,login,display_name,state,epoch,verifier) VALUES(1,'owner','Owner','enabled',1,?1)",["$argon2id$v=19$m=19456,t=2,p=1$AAAAAAAAAAAAAAAAAAAAAA$AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA"])?;
        db.execute("INSERT INTO account_owner VALUES(1,1)",[])?;
        db.execute("INSERT INTO auth_sessions VALUES(?1,100,1000,'active',?2,1,1)",params![session.to_string(),serde_json::json!({"csrf":"a".repeat(64),"principal":{"id":1,"epoch":1}}).to_string()])?;Ok(())
    }).unwrap();
    let auth = RequestAuth::fixture(1, session, 1000);
    let owner = OwnerWrite {
        csrf: auth.csrf_fixture(),
        auth,
    };
    let manager = Uploads::initialize(store, data.unwrap_or(dir.path()), worker, false).unwrap();
    (dir, manager, owner)
}
fn create(m: &Uploads, o: &OwnerWrite) -> String {
    let mut phase = m.begin_upload().unwrap();
    phase.reserve_body(Some(PDF.len() as u64)).unwrap();
    phase.write(PDF).unwrap();
    let id = m
        .create(o, phase, &hash(PDF), PDF.len() as u64, &metadata())
        .unwrap();
    wait(m);
    id
}
fn wait(m: &Uploads) {
    let mut control = m.0.control.lock().unwrap();
    if let Some(r) = control.take() {
        r.handle.join().unwrap();
    }
}
fn entered(f: &Fake) {
    let until = Instant::now() + Duration::from_secs(5);
    while !f.entered.load(Ordering::Acquire) {
        assert!(Instant::now() < until, "fake never started");
        std::thread::sleep(Duration::from_millis(1));
    }
}
fn selected(m: &Uploads, o: &OwnerWrite, id: &str, pages: &[u32]) {
    let revision = m.read(id).unwrap().revision;
    m.set(o, id, revision, &metadata(), pages).unwrap();
}
fn start(m: &Uploads, o: &OwnerWrite, id: &str) {
    m.start(o, id, m.read(id).unwrap().revision, false).unwrap();
}
fn seed_queued(m: &Uploads, o: &OwnerWrite, id: &str, kind: &str) {
    m.0.files.create(id).unwrap();
    let mut r = m.0.ledger.reserve(MIB).unwrap();
    m.0.files.write(id, "document.pdf", PDF, &mut r).unwrap();
    m.write(o,|tx| {let t=now()?;
        tx.execute("INSERT INTO source_uploads(id,pdf_sha256,pdf_bytes,actor,actor_id,actor_epoch,session_digest,created_at,updated_at,state,system_id,edition,language) VALUES(?1,?2,?3,'account:1',1,1,?4,?5,?5,'uploaded','mork-borg','Synthetic','en')",params![id,hash(PDF),PDF.len(),o.auth.digest(),t])?;queue(tx,id,1,kind,0)
    }).unwrap();
}

#[test]
fn metadata_can_change_before_inspection() {
    let (_dir, m, o, _) = fixture();
    seed_queued(&m, &o, ID, "inspect");
    m.set(&o, ID, 1, &metadata(), &[]).unwrap();
    assert_eq!(m.read(ID).unwrap().revision, 2);
}

#[test]
fn an_open_upload_phase_keeps_the_process_ledger_alive() {
    let (_dir, manager, _owner, _fake) = fixture();
    let weak = Arc::downgrade(&manager.0);
    let phase = manager.begin_upload().unwrap();
    drop(manager);
    assert!(
        weak.upgrade().is_some(),
        "an active reservation must keep its manager alive"
    );
    drop(phase);
    assert!(weak.upgrade().is_none());
}

#[test]
fn failed_inspection_can_be_inspected_again_without_losing_the_pdf() {
    let (_dir, m, o, f) = fixture();
    seed_queued(&m, &o, ID, "inspect");
    f.inspect_failure.store(true, Ordering::Release);
    m.launch(ID).unwrap();
    wait(&m);
    let failed = m.read(ID).unwrap();
    assert_eq!(failed.state, "failed");
    assert_eq!(failed.job.reason.as_deref(), Some("unsupported"));
    assert!(failed.inspection.is_none());
    assert_eq!(m.0.ledger.usage().unwrap(), (PDF.len() as u64, 0));
    f.inspect_failure.store(false, Ordering::Release);
    m.start(&o, ID, 1, true).unwrap();
    wait(&m);
    assert_eq!(m.read(ID).unwrap().state, "inspected");
    assert_eq!(m.read(ID).unwrap().job.state, "succeeded");
}

// SRC-15 slice 2: the stored bound is the worker's 20,000 pages (migration 036).
#[test]
fn a_document_over_the_old_two_thousand_page_cap_is_stored_and_the_upload_proceeds() {
    for pages in [2001, worker::records::MAX_PAGES] {
        let (_dir, m, o, f) = fixture();
        f.inspected_pages.store(pages, Ordering::Release);
        seed_queued(&m, &o, ID, "inspect");
        m.launch(ID).unwrap();
        wait(&m);
        let u = m.read(ID).unwrap();
        assert_eq!(u.state, "inspected", "{pages}");
        assert_eq!(u.job.state, "succeeded", "{pages}");
        assert_eq!(u.inspection.unwrap().document.pages, pages);
        // The upload proceeds: any page of the document can be selected and rendered.
        m.set(&o, ID, u.revision, &metadata(), &[pages]).unwrap();
        start(&m, &o, ID);
        wait(&m);
        let u = m.read(ID).unwrap();
        assert_eq!(u.state, "rendered", "{pages}");
        assert_eq!(u.pages[0].pdf_page, pages);
    }
}

#[test]
fn a_document_over_twenty_thousand_pages_fails_as_too_many_pages() {
    let (_dir, m, o, f) = fixture();
    f.inspected_pages
        .store(worker::records::MAX_PAGES + 1, Ordering::Release);
    seed_queued(&m, &o, ID, "inspect");
    m.launch(ID).unwrap();
    wait(&m);
    let u = m.read(ID).unwrap();
    assert_eq!(u.state, "failed");
    assert_eq!(u.job.reason.as_deref(), Some("too_many_pages"));
    assert!(u.inspection.is_none());
}

#[test]
fn files_of_rows_dropped_by_migration_036_are_removed_at_the_next_start() {
    let (dir, m, o, f) = fixture();
    let id = create(&m, &o);
    selected(&m, &o, &id, &[1]);
    start(&m, &o, &id);
    wait(&m);
    let staged = dir.path().join("staging").join(&id);
    assert!(staged.join("document.pdf").exists());
    assert!(staged.join("page-1.png").exists());
    std::fs::write(dir.path().join("outside"), b"keep").unwrap();
    std::os::unix::fs::symlink(dir.path().join("outside"), staged.join("link")).unwrap();
    // The migration drops the four tables (children first) and recreates them.
    m.0.store
        .upload_storage(|db| {
            db.execute_batch(include_str!("../../migrations/036_upload_limits.sql"))?;
            assert_eq!(
                db.query_row("SELECT count(*) FROM source_uploads", [], |r| r
                    .get::<_, i64>(0))?,
                0
            );
            Ok(())
        })
        .unwrap();
    let store = m.0.store.clone();
    drop(m);
    let restarted = Uploads::initialize(store, dir.path(), f, false).unwrap();
    assert!(!staged.exists());
    assert_eq!(std::fs::read(dir.path().join("outside")).unwrap(), b"keep");
    assert_eq!(restarted.0.ledger.usage().unwrap(), (0, 0));
    assert!(restarted.read(&id).is_err());
}

#[test]
fn startup_removes_arbitrarily_named_orphan_folders_without_following_links() {
    let dir = tempfile::tempdir().unwrap();
    let files = staging::Files::open(dir.path()).unwrap();
    std::fs::create_dir(dir.path().join("staging/junk")).unwrap();
    std::fs::write(dir.path().join("outside"), b"keep").unwrap();
    std::os::unix::fs::symlink(
        dir.path().join("outside"),
        dir.path().join("staging/junk/link"),
    )
    .unwrap();
    assert_eq!(files.reconcile(|_, _| false).unwrap().bytes, 0);
    assert!(!dir.path().join("staging/junk").exists());
    assert_eq!(std::fs::read(dir.path().join("outside")).unwrap(), b"keep");
}

#[test]
fn startup_cleanup_continues_after_a_failure_and_counts_retained_bytes() {
    let dir = tempfile::tempdir().unwrap();
    let files = staging::Files::open(dir.path()).unwrap();
    for name in ["failed", "removable"] {
        std::fs::create_dir(dir.path().join("staging").join(name)).unwrap();
        std::fs::write(dir.path().join("staging").join(name).join("data"), PDF).unwrap();
    }
    *files.fail_cleanup.lock().unwrap() = Some("failed".into());
    let usage = files.reconcile(|_, _| false).unwrap();
    assert_eq!(usage.bytes, PDF.len() as u64);
    assert!(!usage.unknown, "every entry was measured");
    assert!(!dir.path().join("staging/removable").exists());
    assert!(dir.path().join("staging/failed/data").exists());
    *files.fail_cleanup.lock().unwrap() = None;
    assert_eq!(files.reconcile(|_, _| false).unwrap().bytes, 0);
}

#[test]
fn ledger_reservations_total_margin_and_failure_release() {
    let free = Arc::new(std::sync::atomic::AtomicU64::new(
        staging::MARGIN + staging::RENDER_RESERVATION,
    ));
    let injected = free.clone();
    let ledger = staging::Ledger::new(0, move || Ok(injected.load(Ordering::SeqCst)));
    let mut first = ledger.reserve(staging::RENDER_RESERVATION).unwrap();
    assert!(ledger.reserve(1).is_err());
    first.written(20 * MIB).unwrap();
    assert_eq!(ledger.usage().unwrap(), (20 * MIB, 140 * MIB));
    assert!(first.written(141 * MIB).is_err());
    drop(first);
    assert_eq!(ledger.usage().unwrap(), (20 * MIB, 0));
    free.store(staging::MARGIN, Ordering::SeqCst);
    assert!(ledger.reserve(1).is_err());
    free.store(staging::MARGIN + 1, Ordering::SeqCst);
    drop(ledger.reserve(1).unwrap());
    ledger.deleted(20 * MIB).unwrap();
    assert_eq!(ledger.usage().unwrap(), (0, 0));
    // Used bytes do not bound a reservation: only free space does (there is no total).
    let ledger = staging::Ledger::new(100 * 1024 * MIB, || Ok(u64::MAX));
    drop(ledger.reserve(8 * 1024 * MIB).unwrap());
    let ledger = staging::Ledger::new(0, || Err(AppError::Invalid("injected statvfs failure")));
    assert!(ledger.reserve(1).is_err());
    assert_eq!(ledger.usage().unwrap(), (0, 0));
}

#[test]
fn a_reserve_right_after_a_large_write_is_not_refused_by_a_double_count() {
    // The filesystem's free space already excludes written bytes, so they must leave the
    // reservation: counting them in both would refuse this reserve.
    let free = Arc::new(std::sync::atomic::AtomicU64::new(
        staging::MARGIN + 200 * MIB,
    ));
    let injected = free.clone();
    let ledger = staging::Ledger::new(0, move || Ok(injected.load(Ordering::SeqCst)));
    let mut first = ledger.reserve(100 * MIB).unwrap();
    first.written(80 * MIB).unwrap();
    free.fetch_sub(80 * MIB, Ordering::SeqCst);
    // 20 MiB still reserved and 120 MiB above the margin is free: exactly 100 MiB more fits.
    assert!(ledger.reserve(100 * MIB + 1).is_err());
    let second = ledger.reserve(100 * MIB).unwrap();
    assert_eq!(ledger.usage().unwrap(), (80 * MIB, 120 * MIB));
    drop((first, second));
    assert_eq!(ledger.usage().unwrap(), (80 * MIB, 0));
}

#[test]
fn unknown_usage_refuses_reserve_until_a_reconcile_succeeds() {
    let unknown = staging::Usage {
        bytes: 5,
        unknown: true,
    };
    let ledger = staging::Ledger::with_usage(unknown, || Ok(u64::MAX));
    assert!(ledger.reserve(1).is_err());
    assert!(ledger.empty_reservation().top_up(1).is_err());
    // An incomplete scan keeps (or puts) the ledger in the unknown state.
    ledger.reconcile(unknown).unwrap();
    assert!(ledger.reserve(1).is_err());
    ledger
        .reconcile(staging::Usage {
            bytes: 7,
            unknown: false,
        })
        .unwrap();
    drop(ledger.reserve(1).unwrap());
    assert_eq!(ledger.usage().unwrap(), (7, 0));
}

#[test]
fn upload_slot_spool_drop_hash_and_owner_admission() {
    let (dir, m, o, _) = fixture();
    let mut phase = m.begin_upload().unwrap();
    phase.file().write_all(PDF).unwrap();
    assert!(matches!(m.begin_upload(), Err(AppError::Conflict)));
    drop(phase);
    assert_eq!(m.0.ledger.usage().unwrap(), (0, 0));
    assert_eq!(
        std::fs::read_dir(dir.path().join("staging"))
            .unwrap()
            .count(),
        0
    );
    let mut phase = m.begin_upload().unwrap();
    phase.file().write_all(PDF).unwrap();
    // `create` checks size, magic and the hash's form; the content hash is the first job's.
    assert!(
        m.create(&o, phase, &"0".repeat(63), PDF.len() as u64, &metadata())
            .is_err(),
        "a malformed hash"
    );
    let mut phase = m.begin_upload().unwrap();
    phase.file().write_all(PDF).unwrap();
    assert!(
        m.create(&o, phase, &hash(PDF), PDF.len() as u64 + 1, &metadata())
            .is_err(),
        "a size that is not the file's"
    );
    let mut phase = m.begin_upload().unwrap();
    phase.file().write_all(b"not a pdf at all").unwrap();
    assert!(
        m.create(&o, phase, &hash(b"not a pdf at all"), 16, &metadata())
            .is_err(),
        "no %PDF- magic"
    );
    let mut phase = m.begin_upload().unwrap();
    phase.file().write_all(PDF).unwrap();
    let invalid = OwnerWrite {
        auth: o.auth.clone(),
        csrf: o.auth.stale_csrf_fixture(),
    };
    assert!(
        m.create(&invalid, phase, &hash(PDF), PDF.len() as u64, &metadata())
            .is_err()
    );
    assert_eq!(m.0.ledger.usage().unwrap(), (0, 0));
    let id = create(&m, &o);
    assert!(m.begin_upload().is_err());
    assert_eq!(m.0.ledger.usage().unwrap(), (PDF.len() as u64, 0));
    assert_eq!(m.read(&id).unwrap().state, "inspected");
    assert!(m.set(&invalid, &id, 1, &metadata(), &[1]).is_err());
    assert!(m.start(&invalid, &id, 1, true).is_err());
    assert!(m.cancel(&invalid, &id).is_err());
    assert!(m.discard(&invalid, &id).is_err());
    assert_eq!(m.read(&id).unwrap().revision, 1);
    m.discard(&o, &id).unwrap();
    assert_eq!(m.0.ledger.usage().unwrap(), (0, 0));
}

#[test]
fn inspection_render_batches_contract_and_revision_independent_currency() {
    let (_dir, m, o, f) = fixture();
    let id = create(&m, &o);
    assert!(m.contract(&id).is_err());
    assert!(m.start(&o, &id, 1, false).is_err());
    selected(&m, &o, &id, &(1..=17).collect::<Vec<_>>());
    start(&m, &o, &id);
    wait(&m);
    let u = m.read(&id).unwrap();
    assert_eq!(u.state, "rendered");
    assert_eq!(u.job.state, "succeeded");
    assert_eq!(u.job.done, 17);
    assert_eq!(u.job.total, 17);
    assert_eq!(f.calls.load(Ordering::SeqCst), 3);
    let contract = m.contract(&id).unwrap();
    assert_eq!(contract.contract_version, "mimik-staged-v1");
    assert_eq!(contract.renderer.mupdf_version, "1.27.2");
    assert_eq!(contract.renderer.settings, RenderSettings::default());
    assert_eq!(contract.pages.len(), 17);
    assert!(contract.exclusion.is_none());
    // No per-upload totals: the contract carries no limits.
    assert!(
        !serde_json::to_string(&contract)
            .unwrap()
            .contains("\"limits\"")
    );
    let rendered_page = &contract.pages[0];
    assert_eq!(rendered_page.transform, page(1).transform);
    assert_eq!(rendered_page.stats.fonts_substituted, ["FixtureFont"]);
    assert_eq!(rendered_page.png_sha256, hash(&rendered(1).png));
    assert_eq!(
        (rendered_page.width_points, rendered_page.height_points),
        (10., 10.)
    );
    assert_eq!(rendered_page.rendered_revision, 2);
    selected(&m, &o, &id, &[1, 2]);
    assert_eq!(
        m.read(&id).unwrap().state,
        "rendered",
        "retained current pages stay rendered after selection changes"
    );
    assert_eq!(m.contract(&id).unwrap().revision, 3);
    assert_eq!(m.contract(&id).unwrap().pages[0].rendered_revision, 2);
    start(&m, &o, &id);
    wait(&m);
    assert_eq!(
        f.calls.load(Ordering::SeqCst),
        3,
        "current pages are not rerendered"
    );
    assert_eq!(m.read(&id).unwrap().job.total, 0);
    assert_eq!(m.read(&id).unwrap().pages.len(), 2);
    assert!(
        m.0.files
            .path(&id)
            .unwrap()
            .parent()
            .unwrap()
            .join("page-3.png")
            .try_exists()
            .is_ok_and(|e| !e)
    );
}

#[test]
fn selection_limits_and_metadata_import_rules() {
    let (_dir, m, o, _) = fixture();
    let id = create(&m, &o);
    for selection in [vec![0], vec![71], vec![1, 1], vec![2, 1]] {
        assert!(m.set(&o, &id, 1, &metadata(), &selection).is_err());
    }
    assert!(matches!(
        m.set(&o, &id, 2, &metadata(), &[1]),
        Err(AppError::Conflict)
    ));
    let mut invalid = metadata();
    invalid.system_id = "not-registered".into();
    assert!(m.set(&o, &id, 1, &invalid, &[1]).is_err());
    invalid = metadata();
    invalid.language = "x".repeat(31);
    assert!(m.set(&o, &id, 1, &invalid, &[1]).is_err());
    // Every inspected page is a valid selection (the fake document has 70).
    selected(&m, &o, &id, &(1..=70).collect::<Vec<_>>());
    assert_eq!(m.read(&id).unwrap().selected_pages.len(), 70);
    let mut unicode = metadata();
    unicode.edition = "é".repeat(100);
    m.set(&o, &id, 2, &unicode, &[1]).unwrap();
    unicode.edition.push('é');
    assert!(m.set(&o, &id, 3, &unicode, &[1]).is_err());
}

#[test]
fn metadata_only_edit_preserves_running_render() {
    let (_dir, m, o, f) = fixture();
    let id = create(&m, &o);
    selected(&m, &o, &id, &[1, 2]);
    f.hold.store(true, Ordering::Release);
    start(&m, &o, &id);
    entered(&f);
    let mut meta = metadata();
    meta.edition = "Updated".into();
    m.set(&o, &id, 2, &meta, &[1, 2]).unwrap();
    assert_eq!(m.read(&id).unwrap().job.state, "running");
    f.hold.store(false, Ordering::Release);
    wait(&m);
    let u = m.read(&id).unwrap();
    assert_eq!(u.job.state, "succeeded");
    assert_eq!(u.pages.len(), 2);
    assert_eq!(u.metadata.edition, "Updated");
}

#[test]
fn worker_panic_fails_job_and_gate_remains_usable() {
    let (_dir, m, o, f) = fixture();
    let id = create(&m, &o);
    selected(&m, &o, &id, &[1]);
    f.panic.store(true, Ordering::Release);
    start(&m, &o, &id);
    wait(&m);
    assert_eq!(m.read(&id).unwrap().job.state, "failed");
    assert_eq!(
        m.read(&id).unwrap().job.reason.as_deref(),
        Some("unavailable")
    );
    f.panic.store(false, Ordering::Release);
    start(&m, &o, &id);
    wait(&m);
    assert_eq!(m.read(&id).unwrap().state, "rendered");
}

#[test]
fn concurrent_starts_launch_exactly_one_worker() {
    let (_dir, m, o, f) = fixture();
    let id = create(&m, &o);
    selected(&m, &o, &id, &[1]);
    f.hold.store(true, Ordering::Release);
    let barrier = std::sync::Barrier::new(3);
    std::thread::scope(|scope| {
        let handles = (0..2)
            .map(|_| {
                scope.spawn(|| {
                    barrier.wait();
                    m.start(&o, &id, 2, false)
                })
            })
            .collect::<Vec<_>>();
        barrier.wait();
        assert_eq!(
            handles.into_iter().flat_map(|h| h.join().unwrap()).count(),
            1
        );
    });
    entered(&f);
    m.cancel(&o, &id).unwrap();
    assert_eq!(f.calls.load(Ordering::Acquire), 1);
}

#[test]
fn publication_panic_poisoned_gate_is_recovered_and_retry_works() {
    let (_dir, m, o, _) = fixture();
    let id = create(&m, &o);
    selected(&m, &o, &id, &[1]);
    *m.0.before_publish.lock().unwrap() = Some(Box::new(|_| panic!("under publication gate")));
    start(&m, &o, &id);
    wait(&m);
    assert!(m.0.gate.is_poisoned());
    let u = m.read(&id).unwrap();
    assert_eq!(u.job.state, "failed");
    assert_eq!(u.job.reason.as_deref(), Some("unavailable"));
    start(&m, &o, &id);
    wait(&m);
    assert_eq!(m.read(&id).unwrap().state, "rendered");
    m.discard(&o, &id).unwrap();
    assert!(m.begin_upload().is_ok());
}

#[test]
fn atomic_revision_disagreement_settles_running_job_as_cancelled() {
    let (_dir, m, o, _) = fixture();
    let id = create(&m, &o);
    selected(&m, &o, &id, &[1]);
    *m.0.before_finish.lock().unwrap() = Some(Box::new(|revision| {
        revision.fetch_add(1, Ordering::Release);
    }));
    start(&m, &o, &id);
    wait(&m);
    let u = m.read(&id).unwrap();
    assert_eq!(u.job.state, "cancelled");
    assert_eq!(u.job.reason.as_deref(), Some("cancelled"));
    assert_eq!(u.state, "rendered");
}

#[test]
fn sweep_recovers_queued_job_after_spawn_and_settlement_both_fail() {
    let (_dir, m, o, _) = fixture();
    seed_queued(&m, &o, ID, "inspect");
    m.0.fail_spawn.store(true, Ordering::Release);
    m.0.fail_settlement.store(true, Ordering::Release);
    assert!(m.start(&o, ID, 1, true).is_err());
    assert_eq!(m.read(ID).unwrap().job.state, "queued");
    assert!(m.0.control.lock().unwrap().is_none());
    m.sweep().unwrap();
    let u = m.read(ID).unwrap();
    assert_eq!(u.job.state, "failed");
    assert_eq!(u.job.reason.as_deref(), Some("unavailable"));
    assert_eq!(u.state, "failed");
    m.start(&o, ID, 1, true).unwrap();
    wait(&m);
    assert_eq!(m.read(ID).unwrap().state, "inspected");
}

#[test]
fn launch_does_not_deserialize_inspection_before_spawning() {
    let (_dir, m, o, _) = fixture();
    let id = create(&m, &o);
    m.0.store.upload_storage(|db| {
        let tx = db.transaction()?;
        tx.execute("UPDATE source_uploads SET inspection=json_set(inspection,'$.unknown',1) WHERE id=?1", [&id])?;
        queue(&tx, &id, 1, "inspect", 0)?;
        tx.commit()?;
        Ok(())
    }).unwrap();
    assert!(
        m.read(&id).is_err(),
        "fixture must reject inspection deserialization"
    );
    m.0.fail_spawn.store(true, Ordering::Release);
    assert!(m.launch(&id).is_err());
    m.0.store
        .upload_storage(|db| {
            let (state, reason): (String, Option<String>) = db.query_row(
                "SELECT state,reason FROM source_jobs WHERE upload_id=?1",
                [&id],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )?;
            assert_eq!(
                (state.as_str(), reason.as_deref()),
                ("failed", Some("unavailable"))
            );
            Ok(())
        })
        .unwrap();
}

#[test]
fn create_returns_committed_upload_id_when_launch_fails() {
    let (_dir, m, o, _) = fixture();
    m.0.fail_spawn.store(true, Ordering::Release);
    let mut phase = m.begin_upload().unwrap();
    phase.reserve_body(Some(PDF.len() as u64)).unwrap();
    phase.write(PDF).unwrap();
    let id = m
        .create(&o, phase, &hash(PDF), PDF.len() as u64, &metadata())
        .unwrap();
    let u = m.read(&id).unwrap();
    assert_eq!(u.state, "failed");
    assert_eq!(u.job.state, "failed");
    assert_eq!(u.job.reason.as_deref(), Some("unavailable"));
    assert!(m.begin_upload().is_err());
    m.start(&o, &id, 1, true).unwrap();
    wait(&m);
    assert_eq!(m.read(&id).unwrap().state, "inspected");
    m.discard(&o, &id).unwrap();
    assert!(m.begin_upload().is_ok());
}

#[test]
fn private_files_survive_restrictive_umask() {
    const CHILD: &str = "MIMIK_TEST_UPLOAD_UMASK_CHILD";
    if std::env::var_os(CHILD).is_none() {
        for mask in ["0477", "0277"] {
            let output = std::process::Command::new("sh")
                .args([
                    "-c",
                    "umask \"$1\" && shift && exec \"$@\"",
                    "umask-child",
                    mask,
                ])
                .arg(std::env::current_exe().unwrap())
                .args([
                    "--exact",
                    "sources::uploads::tests::private_files_survive_restrictive_umask",
                    "--nocapture",
                ])
                .env(CHILD, "1")
                .output()
                .unwrap();
            let stdout = String::from_utf8(output.stdout).unwrap();
            assert!(
                output.status.success(),
                "{stdout}\n{}",
                String::from_utf8_lossy(&output.stderr)
            );
            assert!(
                stdout
                    .lines()
                    .any(|line| line.starts_with("test result: ok. 1 passed;")),
                "{stdout}"
            );
        }
        return;
    }
    // A separate test process avoids changing any parallel test's process-global umask.
    let dir = tempfile::tempdir().unwrap();
    std::fs::set_permissions(
        dir.path(),
        std::os::unix::fs::PermissionsExt::from_mode(0o700),
    )
    .unwrap();
    let files = staging::Files::open(dir.path()).unwrap();
    files.create(ID).unwrap();
    assert_eq!(
        std::fs::metadata(dir.path().join("staging").join(ID))
            .unwrap()
            .mode()
            & 0o777,
        0o700
    );
    let ledger = staging::Ledger::new(0, || Ok(u64::MAX));
    let mut reservation = ledger.reserve(4096).unwrap();
    let (name, mut spool) = files.spool().unwrap();
    spool.write_all(PDF).unwrap();
    files.publish_spool(&name, &spool, ID).unwrap();
    files.sync_published(ID).unwrap();
    files
        .write(ID, "page-1.png", b"image", &mut reservation)
        .unwrap();
    assert!(files.private_file(ID, "document.pdf"));
    assert!(files.private_file(ID, "page-1.png"));
}

#[test]
fn spool_chmod_failure_removes_temporary_file() {
    let (dir, m, _o, _) = fixture();
    m.0.files.fail_spool_chmod.store(true, Ordering::Release);
    assert!(m.begin_upload().is_err());
    assert_eq!(
        std::fs::read_dir(dir.path().join("staging"))
            .unwrap()
            .count(),
        0
    );
    assert_eq!(m.0.ledger.usage().unwrap(), (0, 0));
    m.0.files.fail_spool_chmod.store(false, Ordering::Release);
    assert!(m.begin_upload().is_ok());
}

#[test]
fn spawn_failure_fails_job_and_releases_worker_slot() {
    let (_dir, m, o, _) = fixture();
    seed_queued(&m, &o, ID, "inspect");
    m.0.fail_spawn.store(true, Ordering::Release);
    assert!(m.start(&o, ID, 1, true).is_err());
    assert_eq!(m.read(ID).unwrap().job.state, "failed");
    assert_eq!(
        m.read(ID).unwrap().job.reason.as_deref(),
        Some("unavailable")
    );
    assert!(m.0.control.lock().unwrap().is_none());
    m.start(&o, ID, 1, true).unwrap();
    wait(&m);
    assert_eq!(m.read(ID).unwrap().job.state, "succeeded");
    m.discard(&o, ID).unwrap();
    m.0.fail_spawn.store(true, Ordering::Release);
    let mut phase = m.begin_upload().unwrap();
    phase.reserve_body(Some(PDF.len() as u64)).unwrap();
    phase.write(PDF).unwrap();
    let id = m
        .create(&o, phase, &hash(PDF), PDF.len() as u64, &metadata())
        .unwrap();
    assert_eq!(m.read(&id).unwrap().job.state, "failed");
    assert_eq!(
        m.read(&id).unwrap().job.reason.as_deref(),
        Some("unavailable")
    );
    m.discard(&o, &id).unwrap();
    assert!(m.begin_upload().is_ok());
}

#[test]
fn failed_discard_is_retried_by_sweep_without_touching_active_spool() {
    let (dir, m, o, _) = fixture();
    let id = create(&m, &o);
    m.0.files.fail_remove.store(true, Ordering::Release);
    assert!(m.discard(&o, &id).is_err());
    assert!(m.read(&id).is_err());
    assert_eq!(m.0.ledger.usage().unwrap().0, PDF.len() as u64);
    m.0.files.fail_remove.store(false, Ordering::Release);
    let mut phase = m.begin_upload().unwrap();
    phase.file().write_all(PDF).unwrap();
    m.sweep().unwrap();
    assert!(!dir.path().join("staging").join(id).exists());
    assert_eq!(phase.file().metadata().unwrap().nlink(), 1);
    assert_eq!(m.0.ledger.usage().unwrap().0, 0);
}

#[test]
fn restart_drops_pages_with_missing_or_non_private_files() {
    use std::os::unix::fs::PermissionsExt;
    for damaged in ["missing", "mode", "symlink"] {
        let (dir, m, o, f) = fixture();
        let id = create(&m, &o);
        selected(&m, &o, &id, &[1]);
        start(&m, &o, &id);
        wait(&m);
        let png = dir.path().join("staging").join(&id).join("page-1.png");
        if damaged == "mode" {
            std::fs::set_permissions(&png, std::fs::Permissions::from_mode(0o644)).unwrap();
        } else {
            std::fs::remove_file(&png).unwrap();
            if damaged == "symlink" {
                std::os::unix::fs::symlink("document.pdf", &png).unwrap();
            }
        }
        let store = m.0.store.clone();
        drop(m);
        let restarted = Uploads::initialize(store, dir.path(), f, false).unwrap();
        assert!(restarted.read(&id).unwrap().pages.is_empty(), "{damaged}");
        assert_eq!(restarted.read(&id).unwrap().state, "inspected", "{damaged}");
        assert_eq!(restarted.read(&id).unwrap().job.done, 0, "{damaged}");
        assert!(restarted.contract(&id).is_err());
        assert_eq!(restarted.0.ledger.usage().unwrap().0, PDF.len() as u64);
    }
}

#[test]
fn failure_keeps_published_pages_and_retry_renders_the_rest() {
    let (_dir, m, o, f) = fixture();
    let id = create(&m, &o);
    selected(&m, &o, &id, &[1, 2, 3]);
    f.fail.store(true, Ordering::SeqCst);
    start(&m, &o, &id);
    wait(&m);
    let u = m.read(&id).unwrap();
    assert_eq!(u.state, "failed");
    assert_eq!(u.job.state, "failed");
    assert_eq!(u.job.reason.as_deref(), Some("unreadable"));
    assert_eq!(u.pages.len(), 1);
    assert_eq!(m.0.ledger.usage().unwrap().1, 0);
    f.fail.store(false, Ordering::SeqCst);
    start(&m, &o, &id);
    wait(&m);
    assert_eq!(m.read(&id).unwrap().state, "rendered");
    assert_eq!(m.read(&id).unwrap().job.total, 2);
}

#[test]
fn sweep_drops_a_retry_page_and_settles_upload_and_progress_together() {
    let (dir, m, o, f) = fixture();
    let id = create(&m, &o);
    selected(&m, &o, &id, &[1, 2]);
    f.fail.store(true, Ordering::Release);
    start(&m, &o, &id);
    wait(&m);
    assert_eq!(m.read(&id).unwrap().pages.len(), 1);
    f.fail.store(false, Ordering::Release);
    start(&m, &o, &id);
    wait(&m);
    let u = m.read(&id).unwrap();
    assert_eq!(u.state, "rendered");
    assert_eq!((u.job.done, u.job.total), (1, 1));
    std::fs::remove_file(dir.path().join("staging").join(&id).join("page-2.png")).unwrap();
    m.sweep().unwrap();
    let u = m.read(&id).unwrap();
    assert_eq!(u.state, "inspected");
    assert_eq!(u.pages.len(), 1);
    assert_eq!(u.job.done, 0);
    assert!(m.contract(&id).is_err());
}

#[test]
fn reconciliation_preserves_failed_and_active_upload_states() {
    for (state, job) in [
        ("failed", "failed"),
        ("rendering", "queued"),
        ("rendering", "running"),
    ] {
        let (dir, m, o, _) = fixture();
        let id = create(&m, &o);
        selected(&m, &o, &id, &[1]);
        start(&m, &o, &id);
        wait(&m);
        std::fs::remove_file(dir.path().join("staging").join(&id).join("page-1.png")).unwrap();
        m.0.store
            .upload_storage(|db| {
                db.execute(
                    "UPDATE source_uploads SET state=?2 WHERE id=?1",
                    params![id, state],
                )?;
                db.execute(
                    "UPDATE source_jobs SET state=?2 WHERE upload_id=?1",
                    params![id, job],
                )?;
                retained_files(db, &m.0.files)?;
                Ok(())
            })
            .unwrap();
        let u = m.read(&id).unwrap();
        assert_eq!(u.state, state, "{job}");
        assert_eq!(u.job.state, job);
        assert_eq!(u.job.done, 0);
        assert!(u.pages.is_empty());
    }
}

#[test]
fn corrupt_inspection_does_not_block_sweep_or_startup_accounting() {
    for restart in [false, true] {
        let (dir, m, o, f) = fixture();
        let id = create(&m, &o);
        selected(&m, &o, &id, &[1]);
        start(&m, &o, &id);
        wait(&m);
        std::fs::remove_file(dir.path().join("staging").join(&id).join("page-1.png")).unwrap();
        m.0.store.upload_storage(|db| {
            db.execute("UPDATE source_uploads SET inspection=json_set(inspection,'$.unknown',1) WHERE id=?1", [&id])?;
            Ok(())
        }).unwrap();
        assert!(m.read(&id).is_err());
        let m = if restart {
            let store = m.0.store.clone();
            drop(m);
            Uploads::initialize(store, dir.path(), f, false).unwrap()
        } else {
            m.sweep().unwrap();
            m
        };
        assert_eq!(m.0.ledger.usage().unwrap(), (PDF.len() as u64, 0));
        m.0.store.upload_storage(|db| {
            let (state, done, pages): (String, u32, u32) = db.query_row(
                "SELECT u.state,j.pages_done,(SELECT count(*) FROM source_upload_pages WHERE upload_id=u.id) FROM source_uploads u JOIN source_jobs j ON j.upload_id=u.id WHERE u.id=?1",
                [&id], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))?;
            assert_ne!(state, "rendered");
            assert_eq!((done, pages), (0, 0));
            Ok(())
        }).unwrap();
    }
}

#[test]
fn cancel_queued_running_and_discard_reap_before_deleting() {
    let (_dir, m, o, f) = fixture();
    seed_queued(&m, &o, ID, "inspect");
    m.cancel(&o, ID).unwrap();
    assert_eq!(m.read(ID).unwrap().job.state, "cancelled");
    m.launch(ID).unwrap();
    wait(&m);
    assert_eq!(m.read(ID).unwrap().job.state, "cancelled");
    assert_eq!(
        m.read(ID).unwrap().state,
        "uploaded",
        "a cancelled queued job must never fail the upload"
    );
    m.start(&o, ID, 1, true).unwrap();
    wait(&m);
    selected(&m, &o, ID, &[1, 2]);
    f.hold.store(true, Ordering::Release);
    start(&m, &o, ID);
    entered(&f);
    assert!(m.start(&o, ID, 2, false).is_err());
    m.cancel(&o, ID).unwrap();
    assert_eq!(m.read(ID).unwrap().job.state, "cancelled");
    assert!(m.0.control.lock().unwrap().is_none());
    assert_eq!(m.0.ledger.usage().unwrap().1, 0);
    f.entered.store(false, Ordering::Release);
    start(&m, &o, ID);
    entered(&f);
    m.discard(&o, ID).unwrap();
    assert!(m.read(ID).is_err());
    assert!(!m.0.files.path(ID).unwrap().parent().unwrap().exists());
    assert_eq!(m.0.ledger.usage().unwrap(), (0, 0));
}

#[test]
fn migration_refuses_invalid_embedded_facts() {
    let (_dir, m, o, _) = fixture();
    let id = create(&m, &o);
    selected(&m, &o, &id, &[1]);
    start(&m, &o, &id);
    wait(&m);
    m.0.store
        .upload_storage(|db| {
            for assignment in [
                "stats='{}'",
                "stats=json_set(stats,'$.chars',1000001)",
                "stats=json_set(stats,'$.unmapped',5)",
                "stats=json_set(stats,'$.image_area',1.1)",
                "stats=json_set(stats,'$.fonts_skipped',10001)",
                "stats=json_set(stats,'$.dropped_lines',1000001)",
                "transform='[1,0,0,0,0,0]'",
                "transform='[100001,0,0,1,0,0]'",
                "transform='[\"x\",0,0,1,0,0]'",
                "settings='{}'",
            ] {
                assert!(
                    db.execute(&format!("UPDATE source_upload_pages SET {assignment}"), [])
                        .is_err(),
                    "{assignment}"
                );
            }
            assert!(
                db.execute("UPDATE source_uploads SET inspection='{}'", [])
                    .is_err()
            );
            Ok(())
        })
        .unwrap();
}

#[test]
fn selection_cancels_a_running_render_and_old_results_never_publish() {
    let (_dir, m, o, f) = fixture();
    let id = create(&m, &o);
    selected(&m, &o, &id, &[1, 2]);
    f.hold.store(true, Ordering::Release);
    start(&m, &o, &id);
    entered(&f);
    selected(&m, &o, &id, &[2]);
    let u = m.read(&id).unwrap();
    assert_eq!(u.revision, 3);
    assert_eq!(u.job.state, "cancelled");
    assert!(u.pages.is_empty());
    let mut reservation = m.0.ledger.reserve(staging::RENDER_RESERVATION).unwrap();
    assert!(
        m.publish(
            &id,
            2,
            &rendered(1),
            &mut reservation,
            &AtomicBool::new(false)
        )
        .is_err()
    );
    assert_eq!(m.0.ledger.usage().unwrap().0, PDF.len() as u64);
    assert!(m.read(&id).unwrap().pages.is_empty());
}

#[test]
fn sweep_never_touches_a_running_publication_then_expires_idle_upload() {
    let (_dir, m, o, f) = fixture();
    let id = create(&m, &o);
    selected(&m, &o, &id, &[1]);
    f.hold.store(true, Ordering::Release);
    start(&m, &o, &id);
    entered(&f);
    m.0.store
        .upload_storage(|db| {
            db.execute("UPDATE source_uploads SET created_at=1,updated_at=1", [])?;
            Ok(())
        })
        .unwrap();
    let sweeper = m.clone();
    let thread = std::thread::spawn(move || sweeper.sweep());
    thread.join().unwrap().unwrap();
    assert!(m.read(&id).is_ok());
    f.hold.store(false, Ordering::Release);
    wait(&m);
    assert_eq!(m.read(&id).unwrap().pages.len(), 1);
    m.0.store
        .upload_storage(|db| {
            db.execute("UPDATE source_uploads SET updated_at=1", [])?;
            Ok(())
        })
        .unwrap();
    m.sweep().unwrap();
    assert!(m.read(&id).is_err());
    assert_eq!(m.0.ledger.usage().unwrap(), (0, 0));
    let mut r = m.0.ledger.reserve(staging::RENDER_RESERVATION).unwrap();
    assert!(
        m.publish(&id, 2, &rendered(1), &mut r, &AtomicBool::new(false))
            .is_err()
    );
}

#[test]
fn restart_reconciliation_fails_queued_and_running_and_removes_orphans() {
    for state in ["queued", "running"] {
        let (dir, m, o, f) = fixture();
        seed_queued(&m, &o, ID, "inspect");
        m.0.store
            .upload_storage(|db| {
                db.execute("UPDATE source_jobs SET state=?1", [state])?;
                Ok(())
            })
            .unwrap();
        let folder = m.0.files.path(ID).unwrap().parent().unwrap().to_path_buf();
        std::fs::write(folder.join("page-1.png"), b"orphan").unwrap();
        std::fs::write(folder.join("text-1.json.part"), b"partial").unwrap();
        std::fs::write(dir.path().join("staging/upload-orphan.part"), b"partial").unwrap();
        let restarted = Uploads::initialize(m.0.store.clone(), dir.path(), f, false).unwrap();
        let u = restarted.read(ID).unwrap();
        assert_eq!(u.state, "failed");
        assert_eq!(u.job.state, "failed");
        assert_eq!(u.job.reason.as_deref(), Some("interrupted"));
        assert_eq!(std::fs::read_dir(folder).unwrap().count(), 1);
        assert_eq!(restarted.0.ledger.usage().unwrap(), (PDF.len() as u64, 0));
        restarted.start(&o, ID, 1, true).unwrap();
        wait(&restarted);
        assert_eq!(restarted.read(ID).unwrap().state, "inspected");
    }
}

#[test]
fn a_render_that_panics_after_publishing_a_page_leaves_no_page() {
    let (_dir, m, o, f) = fixture();
    let id = create(&m, &o);
    selected(&m, &o, &id, &[1, 2]);
    f.panic_after_page.store(true, Ordering::Release);
    start(&m, &o, &id);
    wait(&m);
    let u = m.read(&id).unwrap();
    assert_eq!(
        (u.job.state.as_str(), u.job.reason.as_deref()),
        ("failed", Some("unavailable"))
    );
    assert!(
        u.pages.is_empty(),
        "pages of a run without pins are dropped"
    );
    assert_eq!(u.job.done, 0);
    assert!(!u.document_changed);
    let folder = m.0.files.path(&id).unwrap();
    assert_eq!(
        std::fs::read_dir(folder.parent().unwrap()).unwrap().count(),
        1
    );
    assert_eq!(m.0.ledger.usage().unwrap(), (PDF.len() as u64, 0));
}

#[test]
fn restart_drops_the_pages_of_an_interrupted_render() {
    let (dir, m, o, f) = fixture();
    let id = create(&m, &o);
    selected(&m, &o, &id, &[1, 2]);
    start(&m, &o, &id);
    wait(&m);
    assert_eq!(m.read(&id).unwrap().pages.len(), 2);
    m.0.store
        .upload_storage(|db| {
            db.execute(
                "UPDATE source_jobs SET state='running', reason=NULL, finished_at=NULL",
                [],
            )?;
            Ok(())
        })
        .unwrap();
    let restarted = Uploads::initialize(m.0.store.clone(), dir.path(), f, false).unwrap();
    let u = restarted.read(&id).unwrap();
    assert_eq!(u.job.reason.as_deref(), Some("interrupted"));
    assert_eq!(u.job.done, 0);
    assert!(u.pages.is_empty(), "pages of an unpinned run are not kept");
    let folder = restarted.0.files.path(&id).unwrap();
    assert_eq!(
        std::fs::read_dir(folder.parent().unwrap()).unwrap().count(),
        1
    );
    assert!(restarted.contract(&id).is_err());
}

#[test]
fn private_files_no_follow_and_atomic_rename_failure_release() {
    use std::os::unix::fs::{PermissionsExt, symlink};
    let dir = tempfile::tempdir().unwrap();
    let files = staging::Files::open(dir.path()).unwrap();
    files.create(ID).unwrap();
    let root = dir.path().join("staging");
    let folder = root.join(ID);
    let outside = dir.path().join("outside");
    std::fs::write(&outside, b"keep").unwrap();
    assert_eq!(
        std::fs::metadata(&root).unwrap().permissions().mode() & 0o777,
        0o700
    );
    assert_eq!(
        std::fs::metadata(&folder).unwrap().permissions().mode() & 0o777,
        0o700
    );
    let ledger = staging::Ledger::new(0, || Ok(u64::MAX));
    let mut r = ledger.reserve(1024).unwrap();
    symlink(&outside, folder.join("document.pdf.part")).unwrap();
    assert!(files.write(ID, "document.pdf", PDF, &mut r).is_err());
    assert_eq!(ledger.usage().unwrap().0, 0);
    assert_eq!(std::fs::read(&outside).unwrap(), b"keep");
    std::fs::remove_file(folder.join("document.pdf.part")).unwrap();
    files.write(ID, "document.pdf", PDF, &mut r).unwrap();
    assert_eq!(
        std::fs::metadata(folder.join("document.pdf"))
            .unwrap()
            .permissions()
            .mode()
            & 0o777,
        0o600
    );
    assert!(!folder.join("document.pdf.part").exists());
    std::fs::create_dir(folder.join("page-1.png")).unwrap();
    assert!(files.write(ID, "page-1.png", b"new", &mut r).is_err());
    assert!(!folder.join("page-1.png.part").exists());
    assert_eq!(ledger.usage().unwrap().0, PDF.len() as u64);
    drop(r);
    assert_eq!(ledger.usage().unwrap().1, 0);
    symlink(&outside, root.join("22222222222222222222222222222222")).unwrap();
    assert!(
        files
            .write(
                "22222222222222222222222222222222",
                "document.pdf",
                PDF,
                &mut ledger.reserve(1024).unwrap()
            )
            .is_err()
    );
    assert!(files.path("../outside").is_err());
}

#[test]
fn no_per_upload_png_or_text_total_remains_but_the_per_page_text_bound_does() {
    let (_dir, m, o, _) = fixture();
    let id = create(&m, &o);
    selected(&m, &o, &id, &[1, 2]);
    start(&m, &o, &id);
    wait(&m);
    m.0.store
        .upload_storage(|db| {
            db.execute(
                "UPDATE source_upload_pages SET png_bytes=12582912 WHERE upload_id=?1",
                [&id],
            )?;
            let tx = db.transaction()?;
            queue(&tx, &id, 2, "render", 2)?;
            tx.execute("UPDATE source_jobs SET state='running'", [])?;
            tx.commit()?;
            Ok(())
        })
        .unwrap();
    // Artificial metadata stands for 128 MiB of PNG and more than 512 MiB of text on the other
    // pages without allocating them: neither total is a limit any more.
    m.0.store.upload_storage(|db| {
        for n in 3..=12 {db.execute("INSERT INTO source_upload_pages SELECT upload_id,?1,rendered_revision,png_sha256,png_bytes,width_px,height_px,dpi,width_points,height_points,transform,json_set(stats,'$.n',?1),settings,text_sha256,text_bytes,renderer_crate_version,mupdf_version FROM source_upload_pages WHERE pdf_page=1",[n])?;}Ok(())
    }).unwrap();
    let mut r = m.0.ledger.reserve(staging::RENDER_RESERVATION).unwrap();
    m.publish(&id, 2, &rendered(2), &mut r, &AtomicBool::new(false))
        .unwrap();
    drop(r);
    assert_eq!(m.0.ledger.usage().unwrap().1, 0);
    m.0.store.upload_storage(|db| {
        db.execute("UPDATE source_upload_pages SET png_bytes=1,text_bytes=8388608",[])?;
        for n in 13..=65 {db.execute("INSERT INTO source_upload_pages SELECT upload_id,?1,rendered_revision,png_sha256,png_bytes,width_px,height_px,dpi,width_points,height_points,transform,json_set(stats,'$.n',?1),settings,text_sha256,text_bytes,renderer_crate_version,mupdf_version FROM source_upload_pages WHERE pdf_page=1",[n])?;}
        Ok(())
    }).unwrap();
    let mut r = m.0.ledger.reserve(staging::RENDER_RESERVATION).unwrap();
    m.publish(&id, 2, &rendered(2), &mut r, &AtomicBool::new(false))
        .unwrap();
    drop(r);
    assert_eq!(m.0.ledger.usage().unwrap().1, 0);
    // The per-page text bound (8 MiB) stays: one page's text over it is refused, nothing leaks.
    let mut big = rendered(2);
    big.lines = (0..200_000)
        .map(|_| worker::records::TextLine {
            text: "x".repeat(40),
            bounds: [1., 1., 3., 3.],
        })
        .collect();
    let mut r = m.0.ledger.reserve(staging::RENDER_RESERVATION).unwrap();
    assert!(matches!(
        m.publish(&id, 2, &big, &mut r, &AtomicBool::new(false)),
        Err(AppError::Invalid("too_large"))
    ));
    drop(r);
    assert_eq!(m.0.ledger.usage().unwrap().1, 0);
}

#[test]
fn conditional_publication_deletes_both_stale_files_and_preserves_new_state() {
    let (_dir, m, o, _) = fixture();
    let id = create(&m, &o);
    selected(&m, &o, &id, &[1]);
    let changed = id.clone();
    *m.0.before_publish.lock().unwrap() = Some(Box::new(move |store| {
        store
            .upload_storage(|db| {
                db.execute(
                    "UPDATE source_uploads SET revision=revision+1,state='inspected' WHERE id=?1",
                    [&changed],
                )?;
                Ok(())
            })
            .unwrap();
    }));
    start(&m, &o, &id);
    wait(&m);
    let u = m.read(&id).unwrap();
    assert_eq!(u.revision, 3);
    assert_eq!(u.state, "inspected");
    assert_eq!(u.job.state, "cancelled");
    assert!(u.pages.is_empty());
    let folder = m.0.files.path(&id).unwrap().parent().unwrap().to_path_buf();
    assert_eq!(std::fs::read_dir(folder).unwrap().count(), 1);
    assert_eq!(m.0.ledger.usage().unwrap(), (PDF.len() as u64, 0));
}

#[test]
fn replacing_files_charges_only_the_replacement_and_never_follows_the_old_link() {
    let dir = tempfile::tempdir().unwrap();
    let files = staging::Files::open(dir.path()).unwrap();
    files.create(ID).unwrap();
    let ledger = staging::Ledger::new(0, || Ok(u64::MAX));
    let mut reservation = ledger.reserve(4096).unwrap();
    files
        .write(ID, "document.pdf", PDF, &mut reservation)
        .unwrap();
    files
        .write(ID, "document.pdf", b"replacement", &mut reservation)
        .unwrap();
    assert_eq!(ledger.usage().unwrap().0, 11);
    files.remove(ID, "document.pdf", &ledger).unwrap();
    assert_eq!(ledger.usage().unwrap().0, 0);
    let outside = dir.path().join("outside");
    std::fs::write(&outside, b"keep").unwrap();
    std::os::unix::fs::symlink(
        &outside,
        dir.path().join("staging").join(ID).join("document.pdf"),
    )
    .unwrap();
    files
        .write(ID, "document.pdf", b"new", &mut reservation)
        .unwrap();
    assert_eq!(std::fs::read(outside).unwrap(), b"keep");
    assert_eq!(ledger.usage().unwrap().0, 3);
}

#[test]
fn duplicate_document_and_settings_currency() {
    let (_dir, m, o, f) = fixture();
    m.0.store
        .upload_storage(|db| {
            db.execute(
                "INSERT INTO source_documents(pdf_sha256) VALUES(?1)",
                [hash(PDF)],
            )?;
            Ok(())
        })
        .unwrap();
    let id = create(&m, &o);
    assert!(m.read(&id).unwrap().already_document.is_some());
    selected(&m, &o, &id, &[1]);
    start(&m, &o, &id);
    wait(&m);
    assert!(m.contract(&id).is_ok());
    m.0.store
        .upload_storage(|db| {
            db.execute("UPDATE source_upload_pages SET mupdf_version='older'", [])?;
            Ok(())
        })
        .unwrap();
    assert!(m.contract(&id).is_err());
    start(&m, &o, &id);
    wait(&m);
    assert!(m.contract(&id).is_ok());
    assert_eq!(f.calls.load(Ordering::SeqCst), 2);
    let invalid = OwnerWrite {
        auth: o.auth.clone(),
        csrf: o.auth.stale_csrf_fixture(),
    };
    assert!(m.read_for_owner(&invalid.auth, &id).is_ok());
    m.0.store
        .upload_storage(|db| {
            db.execute("UPDATE auth_sessions SET state='revoked',payload=NULL", [])?;
            Ok(())
        })
        .unwrap();
    assert!(m.read_for_owner(&o.auth, &id).is_err());
    assert!(m.discard(&o, &id).is_err());
}

#[test]
#[ignore = "coordinator supplies MIMIK_STAGING_ENOSPC_DIR: an empty private filesystem with less than 8 MiB available"]
fn live_staging_enospc_cleans_temporary_file_and_releases_reservation() {
    let path = std::path::PathBuf::from(
        std::env::var_os("MIMIK_STAGING_ENOSPC_DIR").expect("small filesystem directory required"),
    );
    let fake = Arc::new(Fake::default());
    fake.large.store(true, Ordering::Release);
    let (_dir, m, o) = fixture_worker_data(fake, Some(&path));
    assert!(
        m.0.files.free().unwrap() < 8 * MIB,
        "test requires a small filesystem"
    );
    // Deliberately bypass only the free-space preflight to exercise a real write ENOSPC.
    m.0.ledger.bypass_free.store(true, Ordering::Release);
    let id = create(&m, &o);
    selected(&m, &o, &id, &[1]);
    start(&m, &o, &id);
    wait(&m);
    let u = m.read(&id).unwrap();
    assert_eq!(u.job.state, "failed", "{:?}", u.job);
    assert_eq!(u.job.reason.as_deref(), Some("space"));
    assert!(u.pages.is_empty());
    assert_eq!(m.0.ledger.usage().unwrap(), (PDF.len() as u64, 0));
    assert_eq!(
        std::fs::read_dir(path.join("staging").join(&id))
            .unwrap()
            .count(),
        1
    );
    m.discard(&o, &id).unwrap();
    assert_eq!(m.0.ledger.usage().unwrap(), (0, 0));
}

#[test]
#[ignore = "host confinement: MIMIK_LIVE_RUNNER opt-in, coordinator live loop"]
fn live_staged_synthetic_pdf_inspect_render_and_discard() {
    use lopdf::{Document, Object, dictionary};
    assert!(
        std::env::var_os("MIMIK_LIVE_RUNNER").is_some(),
        "built application runner required"
    );
    let (_dir, m, o) = fixture_worker(Arc::new(PdfWorker));
    let mut pdf = Document::with_version("1.7");
    let pages = pdf.new_object_id();
    let page_id=pdf.add_object(dictionary! {"Type"=>"Page","Parent"=>pages,"MediaBox"=>vec![0.into(),0.into(),1000.into(),1000.into()]});
    pdf.objects.insert(
        pages,
        Object::Dictionary(dictionary! {"Type"=>"Pages","Kids"=>vec![page_id.into()],"Count"=>1}),
    );
    let catalog = pdf.add_object(dictionary! {"Type"=>"Catalog","Pages"=>pages});
    pdf.trailer.set("Root", catalog);
    let mut bytes = Vec::new();
    pdf.save_to(&mut bytes).unwrap();
    let mut phase = m.begin_upload().unwrap();
    phase.file().write_all(&bytes).unwrap();
    let id = m
        .create(&o, phase, &hash(&bytes), bytes.len() as u64, &metadata())
        .unwrap();
    wait(&m);
    assert_eq!(m.read(&id).unwrap().state, "inspected");
    selected(&m, &o, &id, &[1]);
    start(&m, &o, &id);
    wait(&m);
    let upload = m.read(&id).unwrap();
    assert_eq!(upload.state, "rendered", "{:?}", upload.job);
    assert_eq!(m.contract(&id).unwrap().pages.len(), 1);
    // Force a real rerender, then cancel after the real worker starts writing output.
    m.0.store
        .upload_storage(|db| {
            db.execute("UPDATE source_upload_pages SET mupdf_version='older'", [])?;
            Ok(())
        })
        .unwrap();
    let hook = Arc::new(worker::RenderHook::default());
    *m.0.render_hook.lock().unwrap() = Some(hook.clone());
    start(&m, &o, &id);
    let deadline = Instant::now() + Duration::from_secs(30);
    while !hook.entered.load(Ordering::Acquire) {
        assert!(Instant::now() < deadline, "real render did not start");
        std::thread::sleep(Duration::from_millis(1));
    }
    m.cancel(&o, &id).unwrap();
    assert_eq!(m.read(&id).unwrap().job.state, "cancelled");
    assert!(m.0.control.lock().unwrap().is_none());
    assert_eq!(m.0.ledger.usage().unwrap().1, 0);
    m.discard(&o, &id).unwrap();
    assert_eq!(m.0.ledger.usage().unwrap(), (0, 0));
}

#[test]
fn migration_constraints_and_cascade_refuse_bad_rows() {
    let (_dir, m, o, _) = fixture();
    let id = create(&m, &o);
    selected(&m, &o, &id, &[1]);
    start(&m, &o, &id);
    wait(&m);
    m.0.store
        .upload_storage(|db| {
            for assignment in [
                "state='bogus'",
                "revision=0",
                "pdf_bytes=0",
                "pdf_sha256='bad'",
                "selected_pages='[1,1]'",
                "selected_pages='[0]'",
                "selected_pages='[1.5]'",
                "language=''",
                "inspection='[]'",
            ] {
                assert!(
                    db.execute(&format!("UPDATE source_uploads SET {assignment}"), [])
                        .is_err(),
                    "{assignment}"
                );
            }
            for assignment in [
                "state='bogus'",
                "kind='other'",
                "reason='raw diagnostics'",
                "pages_done=20001",
                "pages_total=-1",
                "pages_done=2",
            ] {
                assert!(
                    db.execute(&format!("UPDATE source_jobs SET {assignment}"), [])
                        .is_err(),
                    "{assignment}"
                );
            }
            for assignment in [
                "pdf_page=0",
                "rendered_revision=0",
                "png_bytes=12582913",
                "width_px=4097",
                "height_px=8193",
                "dpi=151",
                "width_points=4001",
                "transform='[]'",
                "stats='[]'",
                "text_bytes=8388609",
                "text_sha256='bad'",
            ] {
                assert!(
                    db.execute(&format!("UPDATE source_upload_pages SET {assignment}"), [])
                        .is_err(),
                    "{assignment}"
                );
            }
            db.execute("INSERT INTO source_upload_exclusions(upload_id,revision,actor,actor_id,actor_epoch,session_digest,rationale,pages,boxes,created_at) VALUES(?1,1,'account:1',1,1,?2,'Synthetic rationale','[1]','{\"1\":[[1,1,2,2]]}',?3)",params![id,o.auth.digest(),now()?])?;
            for assignment in ["revision=0","actor='forged'","actor_epoch=0","session_digest='bad'","rationale=''","pages='[1,1]'","pages='[0]'","boxes='{}'","boxes='{\"1\":[[1,1,5000,2]]}'","boxes='{\"1\":[]}'","boxes='{\"1\":[1,1,2,2]}'","removed=2","proposal_fingerprints='[\"raw text\"]'"] {
                assert!(db.execute(&format!("UPDATE source_upload_exclusions SET {assignment}"),[]).is_err(),"{assignment}");
            }
            db.execute("DELETE FROM source_uploads", [])?;
            assert_eq!(db.query_row("SELECT count(*) FROM source_upload_exclusions",[],|r|r.get::<_,i64>(0))?,0);
            assert_eq!(
                db.query_row("SELECT count(*) FROM source_jobs", [], |r| r
                    .get::<_, i64>(0))?,
                0
            );
            assert_eq!(
                db.query_row("SELECT count(*) FROM source_upload_pages", [], |r| r
                    .get::<_, i64>(0))?,
                0
            );
            Ok(())
        })
        .unwrap();
}

// SRC-15 slice 3: job pins, per-job verification and `document_changed`.
fn tamper(path: &Path, change: impl FnOnce(&std::fs::File)) {
    use std::os::unix::fs::PermissionsExt;
    // A finished job leaves the file 0400; the test plays a writer that can still reach it.
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600)).unwrap();
    let file = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .open(path)
        .unwrap();
    change(&file);
    file.sync_all().unwrap();
}
fn append_byte(path: &Path) {
    use std::os::unix::fs::FileExt;
    tamper(path, |f| f.write_all_at(b"x", PDF.len() as u64).unwrap());
}
/// A different file with the same bytes (or other bytes) renamed over the staged name.
fn replace_by_rename(path: &Path, bytes: &[u8]) {
    let new = path.with_file_name("replacement.tmp");
    std::fs::write(&new, bytes).unwrap();
    std::fs::rename(&new, path).unwrap();
}
fn assert_document_changed(m: &Uploads, id: &str) {
    let u = m.read(id).unwrap();
    assert!(u.document_changed);
    assert_eq!(u.state, "failed");
    assert_eq!(
        (u.job.state.as_str(), u.job.reason.as_deref()),
        ("failed", Some("document_changed"))
    );
    assert!(u.pages.is_empty(), "no page of a changed file remains");
    let directory = m.0.files.path(id).unwrap();
    let leftovers = std::fs::read_dir(directory.parent().unwrap())
        .unwrap()
        .map(|e| e.unwrap().file_name().into_string().unwrap())
        .filter(|n| n.starts_with("page-") || n.starts_with("text-"))
        .collect::<Vec<_>>();
    assert!(leftovers.is_empty(), "page files remain: {leftovers:?}");
}

#[test]
fn a_staged_file_that_is_not_the_admitted_one_fails_the_first_job_as_document_changed() {
    use std::os::unix::fs::FileExt;
    type Change = Box<dyn Fn(&Path)>;
    let cases: Vec<(&str, Change)> = vec![
        (
            "one byte",
            Box::new(|p: &Path| tamper(p, |f| f.write_all_at(b"X", 10).unwrap())),
        ),
        (
            "truncation",
            Box::new(|p: &Path| tamper(p, |f| f.set_len(PDF.len() as u64 - 3).unwrap())),
        ),
        ("append", Box::new(append_byte)),
        (
            "replacement by rename",
            Box::new(|p: &Path| replace_by_rename(p, &PDF.to_ascii_uppercase())),
        ),
        (
            "symlink at the final component",
            Box::new(|p: &Path| {
                let real = p.with_file_name("real.pdf");
                std::fs::rename(p, &real).unwrap();
                std::os::unix::fs::symlink(&real, p).unwrap();
            }),
        ),
        (
            "hard link",
            Box::new(|p: &Path| std::fs::hard_link(p, p.with_file_name("alias.pdf")).unwrap()),
        ),
    ];
    for (name, change) in cases {
        let (_dir, m, o, _) = fixture();
        seed_queued(&m, &o, ID, "inspect");
        change(&m.0.files.path(ID).unwrap());
        m.launch(ID).unwrap();
        wait(&m);
        assert_document_changed(&m, ID);
        assert!(m.read(ID).unwrap().inspection.is_none(), "{name}");
    }
}

#[test]
fn a_replacement_with_the_same_bytes_is_caught_by_the_pins_not_the_hash() {
    let (_dir, m, o, f) = fixture();
    let id = create(&m, &o);
    selected(&m, &o, &id, &[1, 2]);
    let path = m.0.files.path(&id).unwrap();
    *f.during_run.lock().unwrap() = Some(Box::new(move || replace_by_rename(&path, PDF)));
    start(&m, &o, &id);
    wait(&m);
    assert_document_changed(&m, &id);
}

#[test]
fn a_change_during_the_last_batch_that_the_worker_survives_leaves_no_page() {
    for worker_fails in [false, true] {
        let (_dir, m, o, f) = fixture();
        let id = create(&m, &o);
        selected(&m, &o, &id, &[1, 2, 3]);
        let path = m.0.files.path(&id).unwrap();
        *f.during_run.lock().unwrap() = Some(Box::new(move || append_byte(&path)));
        f.fail.store(worker_fails, Ordering::Release);
        start(&m, &o, &id);
        wait(&m);
        assert_eq!(f.calls.load(Ordering::SeqCst), 1);
        assert_document_changed(&m, &id);
        assert_eq!(
            m.0.ledger.usage().unwrap(),
            (PDF.len() as u64, 0),
            "the bytes of the removed pages are released"
        );
    }
}

#[test]
fn a_change_between_batches_stops_the_next_batch_and_removes_every_page() {
    let (_dir, m, o, f) = fixture();
    let id = create(&m, &o);
    selected(&m, &o, &id, &(1..=17).collect::<Vec<_>>());
    let path = m.0.files.path(&id).unwrap();
    let once = AtomicBool::new(false);
    *f.during_run.lock().unwrap() = Some(Box::new(move || {
        if !once.swap(true, Ordering::AcqRel) {
            append_byte(&path);
        }
    }));
    start(&m, &o, &id);
    wait(&m);
    assert_eq!(
        f.calls.load(Ordering::SeqCst),
        1,
        "the next batch is never reserved"
    );
    assert_document_changed(&m, &id);
}

#[test]
fn a_change_during_inspection_is_document_changed_and_stores_nothing() {
    let (_dir, m, o, f) = fixture();
    seed_queued(&m, &o, ID, "inspect");
    let path = m.0.files.path(ID).unwrap();
    *f.during_run.lock().unwrap() = Some(Box::new(move || append_byte(&path)));
    m.launch(ID).unwrap();
    wait(&m);
    assert_document_changed(&m, ID);
    assert!(m.read(ID).unwrap().inspection.is_none());
}

/// Makes the file writable and opens the writer now; the returned call only writes (so the
/// chmod's own change-time update is not what the pins see).
fn same_size_writer(path: &Path) -> Box<dyn Fn() + Send> {
    use std::os::unix::fs::{FileExt, PermissionsExt};
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600)).unwrap();
    let writer = std::fs::OpenOptions::new().write(true).open(path).unwrap();
    let path = path.to_path_buf();
    Box::new(move || {
        std::thread::sleep(Duration::from_millis(20));
        writer.write_all_at(b"X", 10).unwrap();
        assert_eq!(std::fs::metadata(&path).unwrap().len(), PDF.len() as u64);
    })
}

#[test]
fn a_same_size_in_place_write_during_inspection_is_document_changed() {
    let (_dir, m, o, f) = fixture();
    seed_queued(&m, &o, ID, "inspect");
    let path = m.0.files.path(ID).unwrap();
    *f.during_run.lock().unwrap() = Some(same_size_writer(&path));
    m.launch(ID).unwrap();
    wait(&m);
    assert_document_changed(&m, ID);
}

#[test]
fn a_same_size_in_place_write_during_a_render_is_document_changed() {
    let (_dir, m, o, f) = fixture();
    let id = create(&m, &o);
    selected(&m, &o, &id, &[1, 2]);
    let path = m.0.files.path(&id).unwrap();
    *f.during_run.lock().unwrap() = Some(same_size_writer(&path));
    start(&m, &o, &id);
    wait(&m);
    assert_document_changed(&m, &id);
}

#[test]
fn a_changed_upload_refuses_everything_but_discard() {
    let (_dir, m, o, f) = fixture();
    let id = create(&m, &o);
    selected(&m, &o, &id, &[1]);
    start(&m, &o, &id);
    wait(&m);
    assert!(m.contract(&id).is_ok());
    // A render job that is not `succeeded`, and an inspection job, are not a basis.
    for (kind, state) in [
        ("render", "failed"),
        ("render", "cancelled"),
        ("inspect", "succeeded"),
    ] {
        m.0.store
            .upload_storage(|db| {
                db.execute(
                    "UPDATE source_jobs SET kind=?2, state=?3, reason=NULL WHERE upload_id=?1",
                    params![id, kind, state],
                )?;
                Ok(())
            })
            .unwrap();
        assert!(m.contract(&id).is_err(), "{kind} {state}");
    }
    m.0.store
        .upload_storage(|db| {
            db.execute(
                "UPDATE source_jobs SET kind='render', state='succeeded' WHERE upload_id=?1",
                [&id],
            )?;
            Ok(())
        })
        .unwrap();
    assert!(m.contract(&id).is_ok());
    // The flag alone refuses the contract, however good the rows look.
    m.0.store
        .upload_storage(|db| {
            db.execute("UPDATE source_uploads SET document_changed=1", [])?;
            Ok(())
        })
        .unwrap();
    assert!(m.contract(&id).is_err());
    let revision = m.read(&id).unwrap().revision;
    assert!(m.start(&o, &id, revision, true).is_err(), "inspect");
    assert!(m.start(&o, &id, revision, false).is_err(), "render / retry");
    assert!(m.set(&o, &id, revision, &metadata(), &[1]).is_err(), "set");
    assert!(m.read(&id).unwrap().document_changed);
    assert_eq!(f.calls.load(Ordering::SeqCst), 1);
    m.discard(&o, &id).unwrap();
    assert_eq!(m.0.ledger.usage().unwrap(), (0, 0));
}

#[test]
fn create_does_not_hash_the_spool_and_the_first_job_catches_a_wrong_hash() {
    let (_dir, m, o, _) = fixture();
    let mut phase = m.begin_upload().unwrap();
    phase.file().write_all(PDF).unwrap();
    // A well-formed hash that is not the file's: the route's streaming hash is trusted for the
    // stream, so `create` accepts it and the first job's hash verifies the stored bytes.
    let id = m
        .create(
            &o,
            phase,
            &hash(b"another file"),
            PDF.len() as u64,
            &metadata(),
        )
        .unwrap();
    wait(&m);
    assert_document_changed(&m, &id);
}

#[test]
fn the_job_hash_is_cancel_aware() {
    let (_dir, m, o, _) = fixture();
    seed_queued(&m, &o, ID, "inspect");
    let cancel = Cancel::default();
    cancel.cancel();
    assert!(matches!(
        m.hold_document(ID, &hash(PDF), &cancel),
        Err(AppError::Invalid("cancelled"))
    ));
    assert!(m.hold_document(ID, &hash(PDF), &Cancel::default()).is_ok());
    assert!(matches!(
        m.hold_document(ID, &hash(b"other"), &Cancel::default()),
        Err(AppError::Invalid("document_changed"))
    ));
}

#[test]
fn a_legacy_private_upload_without_pins_is_hashed_run_and_then_hardened() {
    use std::os::unix::fs::PermissionsExt;
    let (_dir, m, o, _) = fixture();
    seed_queued(&m, &o, ID, "inspect");
    let path = m.0.files.path(ID).unwrap();
    let mode = || std::fs::metadata(&path).unwrap().permissions().mode() & 0o777;
    assert_eq!(mode(), 0o600);
    m.launch(ID).unwrap();
    wait(&m);
    let u = m.read(ID).unwrap();
    assert_eq!((u.state.as_str(), u.document_changed), ("inspected", false));
    assert_eq!(mode(), 0o400, "hardened at the job end");
    selected(&m, &o, ID, &[1, 2]);
    start(&m, &o, ID);
    wait(&m);
    assert_eq!(m.read(ID).unwrap().state, "rendered");
    assert_eq!(mode(), 0o400);
    assert!(m.contract(ID).is_ok());
}

#[test]
fn a_document_that_is_not_a_private_regular_file_is_refused_by_open_document() {
    let (_dir, m, o, _) = fixture();
    seed_queued(&m, &o, ID, "inspect");
    let path = m.0.files.path(ID).unwrap();
    std::fs::remove_file(&path).unwrap();
    std::fs::create_dir(&path).unwrap();
    assert_eq!(
        m.0.files.open_document(ID).err(),
        Some(Reason::DocumentChanged),
        "a directory"
    );
    std::fs::remove_dir(&path).unwrap();
    assert_eq!(
        m.0.files.open_document(ID).err(),
        Some(Reason::Document(worker::Error::Unreadable)),
        "a missing file is unreadable, not changed"
    );
}

fn spool_names(dir: &tempfile::TempDir) -> usize {
    std::fs::read_dir(dir.path().join("staging"))
        .unwrap()
        .count()
}

#[test]
fn a_pdf_over_128_mib_is_staged_and_the_ledger_follows_the_written_bytes() {
    let (dir, m, o, _) = fixture();
    let free = Arc::new(std::sync::atomic::AtomicU64::new(1 << 40));
    m.0.ledger.inject_free(free);
    let size = 129 * MIB;
    let mut phase = m.begin_upload().unwrap();
    phase.reserve_body(Some(size)).unwrap();
    assert_eq!(m.0.ledger.usage().unwrap(), (0, size));
    phase.write(PDF).unwrap();
    assert_eq!(
        m.0.ledger.usage().unwrap(),
        (PDF.len() as u64, size - PDF.len() as u64)
    );
    // The rest of the body is sparse: nothing here allocates 129 MiB.
    phase.file().set_len(size).unwrap();
    let id = m
        .create(&o, phase, &"0".repeat(64), size, &metadata())
        .unwrap();
    wait(&m);
    assert_eq!(m.read(&id).unwrap().pdf_bytes, size);
    assert_eq!(m.0.ledger.usage().unwrap().1, 0);
    // The staged document is the 129 MiB file (sparse here).
    let staged = std::fs::metadata(m.0.files.path(&id).unwrap()).unwrap();
    assert_eq!(staged.len(), size);
    assert_eq!(spool_names(&dir), 1);
}

#[test]
fn free_space_exhausted_mid_spool_refuses_and_dropping_the_phase_frees_everything() {
    let (dir, m, _o, _) = fixture();
    let free = Arc::new(std::sync::atomic::AtomicU64::new(
        staging::MARGIN + 10 * MIB,
    ));
    m.0.ledger.inject_free(free);
    let mut phase = m.begin_upload().unwrap();
    // The floor of a body without a declared length.
    phase.reserve_body(None).unwrap();
    phase.write(&vec![b'x'; 1 << 20]).unwrap();
    assert_eq!(m.0.ledger.usage().unwrap(), (MIB, 0));
    // The next byte needs a 64 MiB top-up that free space cannot cover.
    assert!(matches!(
        phase.write(b"x"),
        Err(AppError::Invalid("Not enough space."))
    ));
    assert_eq!(spool_names(&dir), 1);
    drop(phase);
    assert_eq!(m.0.ledger.usage().unwrap(), (0, 0));
    assert_eq!(spool_names(&dir), 0);
    assert!(m.begin_upload().is_ok(), "the slot is free again");
}

#[test]
fn enospc_mid_spool_unlinks_the_spool_and_releases_the_reservation_and_the_slot() {
    let (dir, m, _o, _) = fixture();
    let mut phase = m.begin_upload().unwrap();
    phase.reserve_body(None).unwrap();
    // A file that always fails with ENOSPC stands in for a full disk under the spool.
    phase.file = std::fs::OpenOptions::new()
        .write(true)
        .open("/dev/full")
        .unwrap();
    let error = phase.write(b"x").unwrap_err();
    assert!(
        matches!(&error, AppError::SourceIo(e) if e.raw_os_error() == Some(libc::ENOSPC)),
        "{error:?}"
    );
    assert_eq!(error_code(&error), "space");
    // The failed write was not charged; the reservation is held until the drop.
    assert_eq!(m.0.ledger.usage().unwrap(), (0, MIB));
    drop(phase);
    assert_eq!(m.0.ledger.usage().unwrap(), (0, 0));
    assert_eq!(spool_names(&dir), 0);
    assert!(m.begin_upload().is_ok());
}

#[test]
fn enospc_mid_render_fails_the_job_as_space_and_leaves_no_partial_file() {
    let (dir, m, o, _) = fixture();
    let id = create(&m, &o);
    selected(&m, &o, &id, &[1]);
    // The page's PNG is written, its text sidecar hits ENOSPC.
    m.0.files.enospc_after.store(1, Ordering::SeqCst);
    start(&m, &o, &id);
    wait(&m);
    let u = m.read(&id).unwrap();
    assert_eq!(u.job.state, "failed", "{:?}", u.job);
    assert_eq!(u.job.reason.as_deref(), Some("space"));
    assert!(u.pages.is_empty());
    let folder = dir.path().join("staging").join(&id);
    let names: Vec<_> = std::fs::read_dir(folder)
        .unwrap()
        .map(|e| e.unwrap().file_name())
        .collect();
    assert_eq!(names, ["document.pdf"], "no png, text or .part file");
    assert_eq!(m.0.ledger.usage().unwrap(), (PDF.len() as u64, 0));
}

#[test]
fn publishing_a_page_does_not_rewrite_the_upload_row_that_holds_the_inspection() {
    let (_dir, m, o, _) = fixture();
    let id = create(&m, &o);
    selected(&m, &o, &id, &[1]);
    let set = |store: &Store| {
        store
            .upload_storage(|db| {
                db.execute("UPDATE source_uploads SET updated_at=created_at+12345", [])?;
                Ok(())
            })
            .unwrap();
    };
    // Marked just before the page is published; read again once the job has published it,
    // before settlement.
    *m.0.before_publish.lock().unwrap() = Some(Box::new(move |store| set(store)));
    let store = m.0.store.clone();
    let seen = Arc::new(std::sync::atomic::AtomicI64::new(0));
    let observed = seen.clone();
    *m.0.before_finish.lock().unwrap() = Some(Box::new(move |_| {
        let value = store
            .upload_storage(|db| {
                Ok(db.query_row(
                    "SELECT updated_at-created_at FROM source_uploads",
                    [],
                    |r| r.get(0),
                )?)
            })
            .unwrap();
        observed.store(value, Ordering::SeqCst);
    }));
    start(&m, &o, &id);
    wait(&m);
    assert_eq!(m.read(&id).unwrap().pages.len(), 1);
    assert_eq!(seen.load(Ordering::SeqCst), 12345);
}

#[test]
fn a_progress_poll_reads_the_job_of_an_existing_upload_only() {
    let (_dir, m, o, _) = fixture();
    let id = create(&m, &o);
    selected(&m, &o, &id, &[1, 2, 3]);
    start(&m, &o, &id);
    wait(&m);
    let progress = m.progress_for_owner(&o.auth, &id).unwrap();
    assert_eq!(
        (progress.job.done, progress.job.state.as_str()),
        (3, "succeeded")
    );
    assert!(matches!(
        m.progress_for_owner(&o.auth, &"0".repeat(32)),
        Err(AppError::SourceNotFound)
    ));
}

#[test]
fn a_job_that_died_unsettled_after_seven_days_is_failed_not_deleted_with_its_upload() {
    let (dir, m, o, _) = fixture();
    seed_queued(&m, &o, ID, "inspect");
    // Started long ago: only page publication used to refresh `updated_at`, and it no longer
    // does, so the failure transition itself must start the seven days.
    let age = || {
        m.0.store
            .upload_storage(|db| {
                db.execute(
                    "UPDATE source_uploads SET created_at=1000, updated_at=1000",
                    [],
                )?;
                db.execute("UPDATE source_jobs SET state='running'", [])?;
                Ok(())
            })
            .unwrap();
    };
    age();
    m.sweep().unwrap();
    let u = m.read(ID).unwrap();
    assert_eq!(
        (u.state.as_str(), u.job.state.as_str()),
        ("failed", "failed")
    );
    // The same at startup, from a fresh manager over the same data.
    age();
    let again = Uploads::initialize(
        m.0.store.clone(),
        dir.path(),
        Arc::new(Fake::default()),
        false,
    )
    .unwrap();
    let u = again.read(ID).unwrap();
    assert_eq!(
        (u.state.as_str(), u.job.state.as_str()),
        ("failed", "failed")
    );
}

#[test]
fn a_sync_failure_after_the_spool_rename_fails_create_with_a_clean_ledger_and_staging() {
    let (dir, m, o, _) = fixture();
    let mut phase = m.begin_upload().unwrap();
    phase.reserve_body(Some(PDF.len() as u64)).unwrap();
    phase.write(PDF).unwrap();
    m.0.files.fail_publish_sync.store(true, Ordering::Release);
    let error = m
        .create(&o, phase, &hash(PDF), PDF.len() as u64, &metadata())
        .unwrap_err();
    // The real I/O error, not an accounting mismatch from the cleanup.
    assert!(
        matches!(&error, AppError::SourceIo(e) if e.raw_os_error() == Some(libc::EIO)),
        "{error:?}"
    );
    assert_eq!(m.0.ledger.usage().unwrap(), (0, 0));
    assert_eq!(spool_names(&dir), 0);
    m.0.files.fail_publish_sync.store(false, Ordering::Release);
    assert!(m.begin_upload().is_ok());
}

#[test]
fn a_spool_that_cannot_be_unlinked_stays_charged_until_a_sweep_removes_it() {
    let (dir, m, _o, _) = fixture();
    let mut phase = m.begin_upload().unwrap();
    phase.reserve_body(None).unwrap();
    phase.write(PDF).unwrap();
    m.0.files.fail_remove_spool.store(true, Ordering::Release);
    drop(phase);
    m.0.files.fail_remove_spool.store(false, Ordering::Release);
    assert_eq!(m.0.ledger.usage().unwrap(), (PDF.len() as u64, 0));
    assert_eq!(spool_names(&dir), 1);
    m.sweep().unwrap();
    assert_eq!(m.0.ledger.usage().unwrap(), (0, 0));
    assert_eq!(spool_names(&dir), 0);
}

#[test]
fn a_launch_failure_starts_the_seven_days_of_the_failed_upload() {
    let (_dir, m, o, _) = fixture();
    seed_queued(&m, &o, ID, "inspect");
    m.0.store
        .upload_storage(|db| {
            db.execute(
                "UPDATE source_uploads SET created_at=1000, updated_at=1000",
                [],
            )?;
            Ok(())
        })
        .unwrap();
    m.fail_job(ID).unwrap();
    m.sweep().unwrap();
    assert_eq!(m.read(ID).unwrap().state, "failed");
}

#[test]
fn a_sweep_during_the_spool_write_keeps_the_pdf_charged_through_create_and_discard() {
    let (dir, m, o, _) = fixture();
    let mut phase = m.begin_upload().unwrap();
    phase.reserve_body(Some(PDF.len() as u64)).unwrap();
    phase.write(PDF).unwrap();
    // The hourly sweep scans the staging directory while the spool is the active upload.
    m.sweep().unwrap();
    assert_eq!(m.0.ledger.usage().unwrap().0, PDF.len() as u64);
    let id = m
        .create(&o, phase, &hash(PDF), PDF.len() as u64, &metadata())
        .unwrap();
    wait(&m);
    assert_eq!(m.0.ledger.usage().unwrap(), (PDF.len() as u64, 0));
    m.discard(&o, &id).unwrap();
    assert_eq!(m.0.ledger.usage().unwrap(), (0, 0));
    assert_eq!(spool_names(&dir), 0);
}

#[test]
fn a_sweep_during_the_spool_write_then_a_dropped_upload_returns_usage_to_zero() {
    let (dir, m, _o, _) = fixture();
    let mut phase = m.begin_upload().unwrap();
    phase.reserve_body(None).unwrap();
    phase.write(PDF).unwrap();
    m.sweep().unwrap();
    phase.write(PDF).unwrap();
    assert_eq!(m.0.ledger.usage().unwrap().0, 2 * PDF.len() as u64);
    drop(phase);
    assert_eq!(m.0.ledger.usage().unwrap(), (0, 0));
    assert_eq!(spool_names(&dir), 0);
}

/// An upload of `pages` fake pages with every page selected and rendered.
fn rendered_upload(pages: u32) -> (tempfile::TempDir, Uploads, OwnerWrite, Arc<Fake>, String) {
    let (dir, m, o, f) = fixture();
    f.inspected_pages.store(pages, Ordering::Release);
    let id = create(&m, &o);
    selected(&m, &o, &id, &(1..=pages).collect::<Vec<_>>());
    start(&m, &o, &id);
    wait(&m);
    (dir, m, o, f, id)
}

#[test]
fn five_hundred_pages_are_selected_rendered_proposed_and_excluded() {
    let (_dir, m, o, f, id) = rendered_upload(500);
    let u = m.read(&id).unwrap();
    assert_eq!(
        (u.state.as_str(), u.job.done, u.job.total),
        ("rendered", 500, 500)
    );
    assert_eq!(f.calls.load(Ordering::SeqCst), 63);
    assert_eq!(m.contract(&id).unwrap().pages.len(), 500);
    // The streamed proposal sees all 500 text pages; confirming it excludes all of them.
    let proposals = m.propose_exclusions(&o.auth, &id, u.revision).unwrap();
    assert_eq!(proposals[0].boxes.len(), 500);
    m.set_exclusion(
        &o,
        &id,
        u.revision,
        0,
        &confirming(&proposals[0].fingerprint),
    )
    .unwrap();
    let stored = m.contract(&id).unwrap().exclusion.unwrap();
    assert_eq!(stored.pages.len(), 500);
    assert!(stored.boxes.values().all(|b| b.len() == 1));
}

/// One page is rendered for real; the other rows are copies of its row (the files of 20,000
/// pages would only measure the disk). The state is that of a finished 20,000-page render.
fn rendered_twenty_thousand() -> (tempfile::TempDir, Uploads, OwnerWrite, String) {
    let (dir, m, o, f) = fixture();
    f.inspected_pages.store(20_000, Ordering::Release);
    let id = create(&m, &o);
    selected(&m, &o, &id, &[1]);
    start(&m, &o, &id);
    wait(&m);
    let all: Vec<u32> = (1..=20_000).collect();
    let revision = m.read(&id).unwrap().revision;
    m.set(&o, &id, revision, &metadata(), &all).unwrap();
    m.0.store.upload_storage(|db| {
        db.execute("INSERT INTO source_upload_pages SELECT upload_id,n.value,rendered_revision,png_sha256,png_bytes,width_px,height_px,dpi,width_points,height_points,transform,json_set(stats,'$.n',n.value),settings,text_sha256,text_bytes,renderer_crate_version,mupdf_version FROM source_upload_pages, (WITH RECURSIVE n(value) AS (SELECT 2 UNION ALL SELECT value+1 FROM n WHERE value<20000) SELECT value FROM n) n WHERE pdf_page=1", [])?;
        db.execute("UPDATE source_jobs SET pages_done=20000,pages_total=20000", [])?;
        Ok(())
    }).unwrap();
    (dir, m, o, id)
}

/// Run `f` and return its value with the seconds it took.
fn timed<T>(f: impl FnOnce() -> T) -> (T, f64) {
    let started = Instant::now();
    let value = f();
    (value, started.elapsed().as_secs_f64())
}

#[test]
fn at_twenty_thousand_pages_the_poll_the_screen_and_the_contract_stay_fast() {
    // SRC-15 slice 6. The bounds are generous: a quadratic step at this size (4 x 10^8
    // comparisons) misses them by an order of magnitude, a loaded machine does not.
    let (_dir, m, o, id) = rendered_twenty_thousand();
    let (progress, poll) = timed(|| m.progress_for_owner(&o.auth, &id).unwrap());
    assert_eq!((progress.job.done, progress.job.total), (20_000, 20_000));
    let (upload, read) = timed(|| m.read_for_owner(&o.auth, &id).unwrap());
    assert_eq!(upload.pages.len(), 20_000);
    let (html, screen) = timed(|| crate::auth::screen_html_for_test(upload, 100));
    // One screen lists 100 pages (about 2.6 KB each with the rendered rows), never 20,000.
    assert!(html.len() < 1_000_000, "{} bytes", html.len());
    let (contract, contract_seconds) = timed(|| m.contract(&id).unwrap());
    assert_eq!(contract.pages.len(), 20_000);
    println!(
        "SCALE 20,000 pages: progress {poll:.4} s, read_for_owner {read:.3} s, screen (build and render) {screen:.3} s, contract {contract_seconds:.3} s"
    );
    for (name, seconds, bound) in [
        ("progress poll", poll, 2.),
        ("read_for_owner", read, 10.),
        ("page screen", screen, 10.),
        ("contract", contract_seconds, 10.),
    ] {
        assert!(seconds < bound, "{name} took {seconds} s (bound {bound} s)");
    }
}

#[test]
fn twenty_thousand_pages_are_selected_and_rendered_without_totals() {
    let (_dir, m, _o, id) = rendered_twenty_thousand();
    let started = Instant::now();
    let u = m.read(&id).unwrap();
    assert_eq!(u.selected_pages.len(), 20_000);
    assert!(remaining(&u).is_empty());
    let contract = m.contract(&id).unwrap();
    assert_eq!(contract.pages.len(), 20_000);
    assert_eq!(contract.selected_pages.len(), 20_000);
    assert!(
        started.elapsed() < Duration::from_secs(30),
        "{:?}",
        started.elapsed()
    );
}

#[test]
fn selection_checks_are_linear_in_the_pages() {
    // 20,000 selected of 20,000 inspected: the quadratic checks took 4 x 10^8 comparisons here.
    let (_dir, m, o, f) = fixture();
    f.inspected_pages.store(20_000, Ordering::Release);
    let id = create(&m, &o);
    let started = Instant::now();
    let all: Vec<u32> = (1..=20_000).collect();
    m.set(&o, &id, 1, &metadata(), &all).unwrap();
    let u = m.read(&id).unwrap();
    assert_eq!(remaining(&u).len(), 20_000);
    assert!(
        started.elapsed() < Duration::from_secs(5),
        "{:?}",
        started.elapsed()
    );
    // A page outside the document, a repeat and a descending pair are still refused.
    for bad in [vec![20_001], vec![1, 1], vec![2, 1]] {
        assert!(m.set(&o, &id, 2, &metadata(), &bad).is_err());
    }
}

#[test]
fn range_lists_round_trip_and_the_worst_case_fits_the_action_body_limit() {
    assert_eq!(parse_page_ranges("1-500, 520").unwrap().len(), 501);
    assert_eq!(
        parse_page_ranges(" 3,1-2 ,2-4\n7").unwrap(),
        [1, 2, 3, 4, 7]
    );
    assert_eq!(parse_page_ranges("").unwrap(), Vec::<u32>::new());
    for bad in ["0", "5-3", "1-20001", "a", "1-", "-2", "1--2", "1.5"] {
        assert!(parse_page_ranges(bad).is_err(), "{bad}");
    }
    assert_eq!(format_page_ranges(&[1, 2, 3, 7, 9, 10]), "1-3,7,9-10");
    assert_eq!(format_page_ranges(&[]), "");
    // Alternate pages are the longest range list of an upload: 10,000 entries.
    let odd: Vec<u32> = (1..=20_000).step_by(2).collect();
    let text = format_page_ranges(&odd);
    assert_eq!(parse_page_ranges(&text).unwrap(), odd);
    let body = format!(
        "csrf={}&revision=1&system=mork-borg&edition=Synthetic&language=en&pages={}",
        "a".repeat(64),
        text.replace(',', "%2C")
    );
    assert!(body.len() < 131_072 - 16_384, "{} bytes", body.len());
}

#[test]
fn an_exclusion_covers_two_hundred_and_fifty_pages_with_several_proposals() {
    let (_dir, m, o, f) = fixture();
    // Two repeated texts at different places; the second also recurs on every page.
    *f.extra_line.lock().unwrap() = Some([5., 6., 8., 8.]);
    f.inspected_pages.store(250, Ordering::Release);
    let id = create(&m, &o);
    selected(&m, &o, &id, &(1..=250).collect::<Vec<_>>());
    start(&m, &o, &id);
    wait(&m);
    let revision = m.read(&id).unwrap().revision;
    let proposals = m.propose_exclusions(&o.auth, &id, revision).unwrap();
    assert_eq!(proposals.len(), 2);
    let mut request = exclusions::ExclusionRequest {
        confirmed: proposals
            .iter()
            .map(|p| exclusions::Confirmation {
                fingerprint: p.fingerprint.clone(),
                skip: vec![7],
                ..Default::default()
            })
            .collect(),
        rationale: "two proposals".into(),
        ..Default::default()
    };
    // Page 7 has no box left from either proposal: a manual box keeps the page in the list.
    request.manual.insert(7, vec![[1., 1., 2., 2.]]);
    m.set_exclusion(&o, &id, revision, 0, &request).unwrap();
    let stored = m.read(&id).unwrap().exclusion.unwrap();
    assert_eq!(stored.pages.len(), 250);
    assert_eq!(stored.proposal_fingerprints.len(), 2);
    assert!(
        stored
            .boxes
            .iter()
            .all(|(n, b)| b.len() == if *n == 7 { 1 } else { 2 })
    );
    assert_eq!(m.contract(&id).unwrap().exclusion.unwrap().pages.len(), 250);
    // Both fingerprints are in the row the migration view checks; an unknown one is refused.
    request.confirmed[0].fingerprint = "f".repeat(64);
    assert!(m.set_exclusion(&o, &id, revision, 1, &request).is_err());
    // Skipping a page the proposal never found is refused, not ignored.
    request.confirmed[0].fingerprint = proposals[0].fingerprint.clone();
    request.confirmed[0].skip = vec![251];
    assert!(m.set_exclusion(&o, &id, revision, 1, &request).is_err());
}

#[test]
fn confirmed_proposals_whose_rounded_boxes_coincide_are_deduplicated_not_refused() {
    let (_dir, m, o, f) = fixture();
    // The second text sits exactly where the first does: equal padded boxes on every page.
    *f.extra_line.lock().unwrap() = Some([1., 1., 3., 3.]);
    let id = create(&m, &o);
    selected(&m, &o, &id, &[1, 2, 3]);
    start(&m, &o, &id);
    wait(&m);
    let revision = m.read(&id).unwrap().revision;
    let proposals = m.propose_exclusions(&o.auth, &id, revision).unwrap();
    assert_eq!(proposals.len(), 2);
    let request = exclusions::ExclusionRequest {
        confirmed: proposals
            .iter()
            .map(|p| exclusions::Confirmation {
                fingerprint: p.fingerprint.clone(),
                ..Default::default()
            })
            .collect(),
        rationale: "same place".into(),
        ..Default::default()
    };
    m.set_exclusion(&o, &id, revision, 0, &request).unwrap();
    let stored = m.read(&id).unwrap().exclusion.unwrap();
    assert!(stored.boxes.values().all(|b| b.len() == 1));
    assert_eq!(stored.proposal_fingerprints.len(), 2);
}

#[test]
fn a_windowed_manual_edit_keeps_the_fingerprints_of_the_boxes_it_keeps() {
    let (_dir, m, o, _, id) = rendered_upload(150);
    let revision = m.read(&id).unwrap().revision;
    let proposals = m.propose_exclusions(&o.auth, &id, revision).unwrap();
    let fingerprint = proposals[0].fingerprint.clone();
    m.set_exclusion(&o, &id, revision, 0, &confirming(&fingerprint))
        .unwrap();
    // Another screen: pages 101-200 are edited by hand; pages 1-100 keep their proposal boxes.
    let edit = exclusions::ExclusionRequest {
        manual: [(120, vec![[1., 1., 2., 2.]])].into(),
        window: Some((101, 200)),
        rationale: "edit".into(),
        ..Default::default()
    };
    m.set_exclusion(&o, &id, revision, 1, &edit).unwrap();
    let stored = m.read(&id).unwrap().exclusion.unwrap();
    assert_eq!(stored.pages.len(), 101);
    assert_eq!(
        stored.proposal_fingerprints,
        std::slice::from_ref(&fingerprint)
    );
    // Confirming it again from a window adds nothing twice.
    let again = exclusions::ExclusionRequest {
        window: Some((101, 200)),
        ..confirming(&fingerprint)
    };
    m.set_exclusion(&o, &id, revision, 2, &again).unwrap();
    let stored = m.read(&id).unwrap().exclusion.unwrap();
    assert_eq!(stored.proposal_fingerprints, [fingerprint]);
}

#[test]
fn unticking_a_page_with_saved_further_boxes_drops_it_but_edited_further_boxes_are_refused() {
    let (_dir, m, o, _, id) = rendered_upload(3);
    let revision = m.read(&id).unwrap().revision;
    let mut first = exclusion();
    first
        .manual
        .insert(1, vec![[1., 1., 2., 2.], [3., 3., 4., 4.]]);
    m.set_exclusion(&o, &id, revision, 0, &first).unwrap();
    assert_eq!(m.read(&id).unwrap().exclusion.unwrap().boxes[&1].len(), 2);
    // Page 1 is unticked with its prefilled further box unchanged: the page is dropped.
    let mut drop_page = exclusions::ExclusionRequest {
        manual: [(2, vec![[1., 1., 2., 2.]])].into(),
        unticked_extra: [(1, vec![[3., 3., 4., 4.]])].into(),
        window: Some((1, 100)),
        rationale: "untick".into(),
        ..Default::default()
    };
    // The same request with the further box edited is refused, not silently dropped.
    drop_page.unticked_extra.insert(1, vec![[3., 3., 4., 5.]]);
    assert!(m.set_exclusion(&o, &id, revision, 1, &drop_page).is_err());
    drop_page.unticked_extra.insert(1, vec![[3., 3., 4., 4.]]);
    m.set_exclusion(&o, &id, revision, 1, &drop_page).unwrap();
    assert_eq!(m.read(&id).unwrap().exclusion.unwrap().pages, [2]);
    // Further boxes for a page that never had any are refused too.
    drop_page.unticked_extra = [(3, vec![[5., 5., 6., 6.]])].into();
    assert!(m.set_exclusion(&o, &id, revision, 2, &drop_page).is_err());
}

/// The stored text of the boxes of `page` in the latest revision.
fn stored_boxes(m: &Uploads, id: &str, page: u32) -> String {
    m.0.store
        .upload_storage(|db| {
            Ok(db.query_row(
                "SELECT json_extract(boxes,'$.\"'||?2||'\"') FROM source_upload_exclusions
                 WHERE upload_id=?1 ORDER BY revision DESC LIMIT 1",
                params![id, page],
                |r| r.get::<_, String>(0),
            )?)
        })
        .unwrap()
}

#[test]
fn eight_boxes_a_page_are_stored_nine_are_refused_and_equal_boxes_collapse() {
    let (_dir, m, o, _, id) = rendered_upload(3);
    let revision = m.read(&id).unwrap().revision;
    let eight: Vec<_> = (0..8)
        .map(|i| [f64::from(i), 1., f64::from(i) + 0.5, 2.])
        .collect();
    let mut request = exclusion();
    request.manual.insert(1, eight.clone());
    m.set_exclusion(&o, &id, revision, 0, &request).unwrap();
    assert_eq!(m.read(&id).unwrap().exclusion.unwrap().boxes[&1].len(), 8);
    let mut nine = eight.clone();
    nine.push([8., 1., 8.5, 2.]);
    request.manual.insert(1, nine.clone());
    assert!(matches!(
        m.set_exclusion(&o, &id, revision, 1, &request),
        Err(AppError::Invalid("At most eight exclusion boxes per page."))
    ));
    // Nine boxes of which two are equal once rounded outward to 0.01 pt are eight.
    nine[8] = [0.0004, 1.0001, 0.5, 2.];
    request.manual.insert(1, nine);
    m.set_exclusion(&o, &id, revision, 1, &request).unwrap();
    let stored = m.read(&id).unwrap().exclusion.unwrap().boxes[&1].clone();
    assert_eq!(stored.len(), 8);
    // Equal boxes are bit-identical text: no "1.0" beside "1" for the migration's distinct check.
    assert_eq!(
        stored_boxes(&m, &id, 1),
        "[[0.0,1.0,0.5,2.0],[1.0,1.0,1.5,2.0],[2.0,1.0,2.5,2.0],[3.0,1.0,3.5,2.0],[4.0,1.0,4.5,2.0],[5.0,1.0,5.5,2.0],[6.0,1.0,6.5,2.0],[7.0,1.0,7.5,2.0]]"
    );
}

/// Replace the inspected page size of page 1 (the rows of later renders follow it).
fn resize_page_one(m: &Uploads, id: &str, width: f64, height: f64) {
    let mut u = m.read(id).unwrap();
    let data = u.inspection.as_mut().unwrap();
    data.pages[0].width = width;
    data.pages[0].height = height;
    m.0.store
        .upload_storage(|db| {
            db.execute(
                "UPDATE source_uploads SET inspection=?2 WHERE id=?1",
                params![id, json(data)?],
            )?;
            Ok(())
        })
        .unwrap();
}

#[test]
fn worst_case_digit_boxes_fit_the_migration_view_and_an_edge_box_on_a_595_2756_page_is_stored() {
    let (_dir, m, o, f) = fixture();
    let id = create(&m, &o);
    resize_page_one(&m, &id, 4000., 4000.);
    selected(&m, &o, &id, &[1]);
    start(&m, &o, &id);
    wait(&m);
    assert!(f.calls.load(Ordering::SeqCst) > 0);
    let revision = m.read(&id).unwrap().revision;
    // Eight boxes of seven characters a number (1111.11-style) after rounding outward.
    let worst: Vec<_> = (0..8)
        .map(|i| {
            let i = f64::from(i);
            [
                1111.1111 + i * 100.123,
                1212.1212 + i * 100.123,
                3200.5555 + i * 100.0111,
                3100.5555 + i * 100.0111,
            ]
        })
        .collect();
    let request = exclusions::ExclusionRequest {
        manual: [(1, worst)].into(),
        rationale: "worst case".into(),
        ..Default::default()
    };
    assert_eq!(m.read(&id).unwrap().pages[0].width_points, 4000.);
    // The migration's trigger refuses a page over 384 bytes: the write succeeding is the check.
    m.set_exclusion(&o, &id, revision, 0, &request).unwrap();
    let text = stored_boxes(&m, &id, 1);
    assert!(text.len() <= 384, "{} bytes", text.len());
    assert_eq!(m.read(&id).unwrap().exclusion.unwrap().boxes[&1].len(), 8);
    // A box ending at the page edge: 595.2756 rounds outward to 595.28, which stays valid.
    m.remove_exclusion(&o, &id, revision, 1, "reset").unwrap();
    resize_page_one(&m, &id, 595.2756, 841.89);
    m.0.store
        .upload_storage(|db| {
            db.execute("UPDATE source_upload_pages SET width_points=595.2756,height_points=841.89 WHERE pdf_page=1", [])?;
            Ok(())
        })
        .unwrap();
    let edge = exclusions::ExclusionRequest {
        manual: [(1, vec![[500.1234, 800.0001, 595.2756, 841.89]])].into(),
        rationale: "edge".into(),
        ..Default::default()
    };
    m.set_exclusion(&o, &id, revision, 2, &edge).unwrap();
    assert_eq!(
        m.read(&id).unwrap().exclusion.unwrap().boxes[&1],
        [[500.12, 800., 595.28, 841.89]]
    );
    // Beyond the rounded page edge is refused.
    let mut outside = edge;
    outside.manual.get_mut(&1).unwrap()[0][2] = 595.2801;
    assert!(m.set_exclusion(&o, &id, revision, 3, &outside).is_err());
    // The inclusive rule over the stored union: the page-wide header is contained, a unit
    // reaching past it crosses and one touching its edge does not.
    let stored = &m.read(&id).unwrap().exclusion.unwrap().boxes[&1];
    assert_eq!(
        exclusions::masking([500.5, 810., 595.2756, 840.], stored),
        exclusions::Masking::Contained
    );
    assert_eq!(
        exclusions::masking([500.5, 790., 595.2756, 840.], stored),
        exclusions::Masking::Crossing
    );
    assert_eq!(
        exclusions::masking([500.5, 700., 595.2756, 800.], stored),
        exclusions::Masking::Clear
    );
}

/// The production worker, observed: counts the pages a render publishes and can hold the render
/// just before one of them (a stand-in for the moment a process dies mid-render).
#[derive(Default)]
struct Observed {
    published: AtomicUsize,
    stall_at: AtomicUsize,
    stalled: AtomicBool,
    release: AtomicBool,
    abort: AtomicBool,
}
impl Observed {
    fn hold_before_page(&self, n: usize) {
        self.published.store(0, Ordering::SeqCst);
        self.stall_at.store(n, Ordering::SeqCst);
        self.stalled.store(false, Ordering::SeqCst);
        self.release.store(false, Ordering::SeqCst);
        self.abort.store(false, Ordering::SeqCst);
    }
    fn wait_until_held(&self) {
        let until = Instant::now() + Duration::from_secs(600);
        while !self.stalled.load(Ordering::Acquire) {
            assert!(Instant::now() < until, "the render never reached the hold");
            std::thread::sleep(Duration::from_millis(20));
        }
    }
}
impl Worker for Observed {
    fn inspect(&self, path: &Path, pins: &Pins, cancel: &Cancel) -> Result<Inspection, Reason> {
        PdfWorker.inspect(path, pins, cancel)
    }
    fn render(
        &self,
        path: &Path,
        pins: &Pins,
        pages: &[Page],
        cancel: &Cancel,
        publish: &mut (dyn FnMut(RenderedPage) -> Result<(), worker::Error> + Send),
    ) -> Result<(), Reason> {
        PdfWorker.render(path, pins, pages, cancel, &mut |page| {
            let n = self.published.fetch_add(1, Ordering::SeqCst) + 1;
            if n == self.stall_at.load(Ordering::Acquire) {
                self.stalled.store(true, Ordering::Release);
                while !self.release.load(Ordering::Acquire) {
                    if cancel.cancelled() {
                        return Err(worker::Error::Unreadable);
                    }
                    std::thread::sleep(Duration::from_millis(5));
                }
                if self.abort.load(Ordering::Acquire) {
                    return Err(worker::Error::Unreadable);
                }
            }
            publish(page)
        })
    }
}

fn stage_pdf(m: &Uploads, o: &OwnerWrite, bytes: &[u8]) -> String {
    let mut phase = m.begin_upload().unwrap();
    phase.reserve_body(Some(bytes.len() as u64)).unwrap();
    phase.write(bytes).unwrap();
    let id = m
        .create(o, phase, &hash(bytes), bytes.len() as u64, &metadata())
        .unwrap();
    wait(m);
    id
}

/// File names of an upload's staging folder.
fn staged_names(m: &Uploads, id: &str) -> Vec<String> {
    let path = m.0.files.path(id).unwrap();
    std::fs::read_dir(path.parent().unwrap())
        .unwrap()
        .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
        .collect()
}

/// The folder holds the document and, for each published page row, exactly its two files.
fn assert_no_partial_pages(m: &Uploads, id: &str) {
    let names = staged_names(m, id);
    let rows = m.read(id).unwrap().pages.len();
    assert!(
        names.iter().all(|n| {
            Path::new(n)
                .extension()
                .is_none_or(|e| !e.eq_ignore_ascii_case("part"))
        }),
        "a partial file remains: {names:?}"
    );
    let count = |prefix: &str, suffix: &str| {
        names
            .iter()
            .filter(|n| n.starts_with(prefix) && n.ends_with(suffix))
            .count()
    };
    assert_eq!(
        (count("page-", ".png"), count("text-", ".json"), names.len()),
        (rows, rows, 1 + 2 * rows),
        "{names:?}"
    );
}

#[test]
#[ignore = "host confinement: MIMIK_LIVE_RUNNER and MIMIK_LIVE_LARGE_DIR opt-in"]
#[expect(
    clippy::too_many_lines,
    reason = "one linear live scenario sharing one manager"
)]
fn live_large_manager_stages_renders_cancels_and_reconciles_on_500_pages() {
    use crate::sources::worker::live_tests::large_documents::{
        Kind, assert_gone, descendants, ensure_pdf, large_dir, report, serial,
    };
    let large = large_dir();
    let _serial = serial();
    let (pdf, sha256) = ensure_pdf(&large, "manual-500.pdf", Kind::Manual(500));
    let bytes = std::fs::read(&pdf).unwrap();
    let data = tempfile::tempdir_in(&large).unwrap();
    let observed = Arc::new(Observed::default());
    let (_db, m, o) = fixture_worker_data(observed.clone(), Some(data.path()));
    let all: Vec<u32> = (1..=500).collect();

    // Stage, inspect (the production worker), select all, render with polls, view, propose.
    let (id, stage_and_inspect) = timed(|| stage_pdf(&m, &o, &bytes));
    let upload = m.read(&id).unwrap();
    assert_eq!(upload.state, "inspected", "{:?}", upload.job);
    assert_eq!(upload.inspection.as_ref().unwrap().pages.len(), 500);
    selected(&m, &o, &id, &all);
    let started = Instant::now();
    start(&m, &o, &id);
    let mut polls = Vec::new();
    loop {
        let (progress, seconds) = timed(|| m.progress_for_owner(&o.auth, &id).unwrap());
        polls.push(seconds);
        if !matches!(progress.job.state.as_str(), "queued" | "running") {
            break;
        }
        std::thread::sleep(Duration::from_millis(250));
    }
    wait(&m);
    let render = started.elapsed().as_secs_f64();
    let upload = m.read(&id).unwrap();
    assert_eq!(
        (upload.state.as_str(), upload.job.done, upload.job.total),
        ("rendered", 500, 500),
        "{:?}",
        upload.job
    );
    assert_no_partial_pages(&m, &id);
    let (_, poll) = timed(|| m.progress_for_owner(&o.auth, &id).unwrap());
    let (upload, read) = timed(|| m.read_for_owner(&o.auth, &id).unwrap());
    let revision = upload.revision;
    let (html, screen) = timed(|| crate::auth::screen_html_for_test(upload, 1));
    let (contract, contract_seconds) = timed(|| m.contract(&id).unwrap());
    assert_eq!(contract.pages.len(), 500);
    let (proposals, propose) = timed(|| m.propose_exclusions(&o.auth, &id, revision).unwrap());
    assert!(!proposals.is_empty(), "the running header must be proposed");
    report(&serde_json::json!({
        // Every time here is warm-cache: the staged copy was just written (not cold-disk figures).
        "test": "manager_500", "sha256": sha256, "cache": "warm",
        "stage_and_inspect_s": stage_and_inspect, "render_s": render,
        "poll_during_render_max_s": polls.iter().copied().fold(0., f64::max),
        "polls_during_render": polls.len(), "progress_after_s": poll,
        "read_for_owner_s": read, "screen_s": screen, "screen_html_bytes": html.len(),
        "contract_s": contract_seconds, "propose_s": propose, "proposals": proposals.len(),
    }));
    m.discard(&o, &id).unwrap();

    // Cancel mid-render: the tree is reaped and every published page is whole.
    let id = stage_pdf(&m, &o, &bytes);
    selected(&m, &o, &id, &all);
    observed.hold_before_page(60);
    start(&m, &o, &id);
    observed.wait_until_held();
    let tree = descendants();
    assert!(!tree.is_empty(), "a sandbox tree must be running");
    let (cancelled, cancel_seconds) = timed(|| m.cancel(&o, &id));
    cancelled.unwrap();
    assert_gone(&tree);
    let upload = m.read(&id).unwrap();
    assert_eq!(upload.job.state, "cancelled", "{:?}", upload.job);
    assert!(
        !upload.pages.is_empty() && upload.pages.len() < 60,
        "{} pages",
        upload.pages.len()
    );
    assert_no_partial_pages(&m, &id);
    assert_eq!(m.0.ledger.usage().unwrap().1, 0, "no reservation is left");
    report(&serde_json::json!({
        "test": "manager_cancel", "cancel_s": cancel_seconds, "kept_pages": upload.pages.len(),
    }));
    m.discard(&o, &id).unwrap();

    // Reconciliation only, not a crash: a new manager over the same store and files reconciles
    // while the old render is held (its process is not killed); the job is interrupted, the
    // dropped pages leave no file, and a retry renders everything again.
    let id = stage_pdf(&m, &o, &bytes);
    selected(&m, &o, &id, &all);
    observed.hold_before_page(60);
    start(&m, &o, &id);
    observed.wait_until_held();
    let before = m.read(&id).unwrap().pages.len();
    let old_tree = descendants();
    assert!(!old_tree.is_empty(), "a sandbox tree must be running");
    let restarted =
        Uploads::initialize(m.0.store.clone(), data.path(), Arc::new(PdfWorker), false).unwrap();
    let after = restarted.read(&id).unwrap();
    assert_eq!(after.job.state, "failed");
    assert_eq!(after.job.reason.as_deref(), Some("interrupted"));
    // An interrupted render keeps no page (its pins died with it; `initialize`).
    assert!(after.pages.is_empty());
    assert_eq!(staged_names(&restarted, &id), ["document.pdf"]);
    observed.abort.store(true, Ordering::SeqCst);
    observed.release.store(true, Ordering::SeqCst);
    wait(&m);
    assert_gone(&old_tree);
    assert_no_partial_pages(&restarted, &id);
    let revision = restarted.read(&id).unwrap().revision;
    let started = Instant::now();
    restarted.start(&o, &id, revision, false).unwrap();
    wait(&restarted);
    let retry = started.elapsed().as_secs_f64();
    let upload = restarted.read(&id).unwrap();
    assert_eq!(
        (upload.state.as_str(), upload.job.done, upload.job.total),
        ("rendered", 500, 500),
        "{:?}",
        upload.job
    );
    assert_no_partial_pages(&restarted, &id);
    assert_eq!(restarted.contract(&id).unwrap().pages.len(), 500);
    report(&serde_json::json!({
        "test": "manager_reconcile_only", "cache": "warm", "published_before_restart": before,
        "kept_after_restart": after.pages.len(), "retry_render_s": retry,
    }));
    restarted.discard(&o, &id).unwrap();
}

// Boxes over a page range (SRC-15 follow-up).
use exclusions::{RangeMode, RangeOutcome, RangeRefusal, RangeRequest};

type Boxes = std::collections::BTreeMap<u32, Vec<[f64; 4]>>;

/// Sets the rendered size (displayed points) of the listed pages.
fn page_sizes(m: &Uploads, sizes: &[(u32, f64, f64)]) {
    m.0.store
        .upload_storage(|db| {
            for (n, w, h) in sizes {
                db.execute(
                    "UPDATE source_upload_pages SET width_points=?2,height_points=?3 WHERE pdf_page=?1",
                    params![n, w, h],
                )?;
            }
            Ok(())
        })
        .unwrap();
}
fn range(mode: RangeMode, pages: Option<&[u32]>, source: u32, boxes: &[[f64; 4]]) -> RangeRequest {
    RangeRequest {
        pages: pages.map(<[u32]>::to_vec),
        mode,
        source_page: source,
        boxes: boxes.to_vec(),
        rationale: "range".into(),
    }
}
fn edit(
    m: &Uploads,
    o: &OwnerWrite,
    id: &str,
    request: &RangeRequest,
) -> Result<Result<RangeOutcome, RangeRefusal>, AppError> {
    let u = m.read(id).unwrap();
    m.set_exclusion_range(o, id, u.revision, u.exclusion_revision, request)
}
fn saved(m: &Uploads, id: &str) -> Boxes {
    m.read(id)
        .unwrap()
        .exclusion
        .map(|e| e.boxes)
        .unwrap_or_default()
}
fn near(got: [f64; 4], want: [f64; 4]) {
    assert!(
        got.iter().zip(want).all(|(g, w)| (g - w).abs() < 0.0101),
        "{got:?} vs {want:?}"
    );
}

#[test]
fn a_range_box_is_scaled_between_a4_letter_and_a_rotated_page_and_equal_sizes_keep_their_points() {
    let (_dir, m, o, _f, id) = rendered_upload(4);
    page_sizes(
        &m,
        &[
            (1, 595., 842.),
            (2, 595., 842.),
            (3, 612., 792.),
            (4, 842., 595.),
        ],
    );
    let drawn = [59.5, 84.2, 535.5, 126.3];
    assert_eq!(
        edit(&m, &o, &id, &range(RangeMode::Add, None, 1, &[drawn])).unwrap(),
        Ok(RangeOutcome {
            changed: 4,
            sizes: 3,
            unmatched: 0
        })
    );
    let stored = saved(&m, &id);
    assert_eq!(stored[&1], [drawn], "the source keeps its points");
    assert_eq!(
        stored[&2],
        [drawn],
        "an equal size keeps its points exactly"
    );
    near(stored[&3][0], [61.2, 79.2, 550.8, 118.8]);
    near(stored[&4][0], [84.2, 59.5, 757.8, 89.25]);
    // Rounded outward, never inward, of the exact fractions.
    assert!(stored[&3][0][0] <= 61.2 + 1e-9 && stored[&3][0][2] >= 550.8 - 1e-9);
    assert_eq!(m.read(&id).unwrap().exclusion_revision, 1);
}

#[test]
fn a_box_touching_the_edge_of_a_595_2756_pt_page_stays_valid_on_every_target() {
    let (_dir, m, o, _f, id) = rendered_upload(3);
    page_sizes(
        &m,
        &[
            (1, 595.2756, 841.89),
            (2, 595.2756, 841.89),
            (3, 600., 800.),
        ],
    );
    let edge = [500.1234, 800.0001, 595.2756, 841.89];
    edit(&m, &o, &id, &range(RangeMode::Add, None, 1, &[edge]))
        .unwrap()
        .unwrap();
    let stored = saved(&m, &id);
    assert_eq!(stored[&1], [[500.12, 800., 595.28, 841.89]]);
    assert_eq!(stored[&2], stored[&1]);
    assert_eq!(stored[&3][0][2], 600., "clamped to the page's room");
    assert_eq!(stored[&3][0][3], 800.);
}

#[test]
fn invalid_boxes_and_modes_are_refused_before_scaling_and_append_nothing() {
    let (_dir, m, o, _f, id) = rendered_upload(2);
    page_sizes(&m, &[(1, 600., 800.), (2, 300., 400.)]);
    for bad in [
        [f64::NAN, 1., 5., 5.],
        [1., 1., f64::INFINITY, 5.],
        [-1., 1., 5., 5.],
        [5., 1., 5., 5.],
        [1., 7., 5., 3.],
        [1., 1., 601., 5.],
        [1., 1., 5., 800.5],
    ] {
        let request = range(RangeMode::Add, None, 1, &[bad]);
        assert!(edit(&m, &o, &id, &request).is_err(), "{bad:?}");
    }
    assert!(edit(&m, &o, &id, &range(RangeMode::Add, None, 1, &[])).is_err());
    let nine = [[1., 1., 2., 2.]; 9];
    assert!(edit(&m, &o, &id, &range(RangeMode::Add, None, 1, &nine)).is_err());
    assert_eq!(RangeMode::parse("add"), Some(RangeMode::Add));
    assert_eq!(RangeMode::parse("remove"), Some(RangeMode::Remove));
    assert_eq!(RangeMode::parse("clear"), Some(RangeMode::Clear));
    for unknown in ["", "Add", "replace", "add "] {
        assert_eq!(RangeMode::parse(unknown), None, "{unknown:?}");
    }
    let mut blank = range(RangeMode::Add, None, 1, &[[1., 1., 5., 5.]]);
    blank.rationale = " ".into();
    assert!(edit(&m, &o, &id, &blank).is_err());
    assert_eq!(m.read(&id).unwrap().exclusion_revision, 0);
}

#[test]
fn a_page_that_is_not_rendered_a_ninth_box_and_a_collapsing_box_refuse_naming_the_page() {
    let (_dir, m, o, _f, id) = rendered_upload(3);
    page_sizes(&m, &[(1, 600., 800.), (2, 600., 800.), (3, 100.001, 100.)]);
    let one = [[1., 1., 5., 5.]];
    assert_eq!(
        edit(
            &m,
            &o,
            &id,
            &range(RangeMode::Add, Some(&[2, 7, 9]), 1, &one)
        )
        .unwrap(),
        Err(RangeRefusal::NotRendered(7))
    );
    assert_eq!(
        edit(&m, &o, &id, &range(RangeMode::Add, None, 7, &one)).unwrap(),
        Err(RangeRefusal::SourceNotRendered(7))
    );
    // Page 1 has eight boxes: a ninth refuses the whole request, page 2 stays untouched.
    let eight: Vec<[f64; 4]> = (0..8)
        .map(|i| [f64::from(i), 1., f64::from(i) + 0.5, 2.])
        .collect();
    let u = m.read(&id).unwrap();
    let request = exclusions::ExclusionRequest {
        manual: [(1, eight)].into(),
        rationale: "eight".into(),
        ..Default::default()
    };
    m.set_exclusion(&o, &id, u.revision, 0, &request).unwrap();
    assert_eq!(
        edit(
            &m,
            &o,
            &id,
            &range(RangeMode::Add, Some(&[2, 1]), 1, &[[50., 50., 60., 60.]])
        )
        .unwrap(),
        Err(RangeRefusal::TooManyBoxes(1))
    );
    // A box valid on a 100.001 pt source page (its room is 100.01) collapses on a 50 pt one.
    page_sizes(&m, &[(2, 50., 100.)]);
    assert_eq!(
        edit(
            &m,
            &o,
            &id,
            &range(
                RangeMode::Add,
                Some(&[2]),
                3,
                &[[100.005, 10., 100.01, 20.]]
            )
        )
        .unwrap(),
        Err(RangeRefusal::Collapsed(2))
    );
    assert_eq!(m.read(&id).unwrap().exclusion_revision, 1);
    assert_eq!(saved(&m, &id).len(), 1);
}

#[test]
fn adding_then_removing_the_same_boxes_restores_the_exclusion_and_unmatched_pages_are_counted() {
    let (_dir, m, o, _f, id) = rendered_upload(3);
    page_sizes(&m, &[(1, 600., 800.), (2, 600., 800.), (3, 300., 400.)]);
    let u = m.read(&id).unwrap();
    let base = exclusions::ExclusionRequest {
        manual: [(1, vec![[1., 2., 4., 5.]]), (2, vec![[1., 2., 4., 5.]])].into(),
        rationale: "base".into(),
        ..Default::default()
    };
    m.set_exclusion(&o, &id, u.revision, 0, &base).unwrap();
    let before = saved(&m, &id);
    let drawn = [[10., 20., 40., 50.], [100., 700., 500., 790.]];
    edit(&m, &o, &id, &range(RangeMode::Add, None, 1, &drawn))
        .unwrap()
        .unwrap();
    assert_eq!(saved(&m, &id)[&3].len(), 2);
    assert_eq!(saved(&m, &id)[&1].len(), 3);
    // The same add again changes nothing.
    assert_eq!(
        edit(&m, &o, &id, &range(RangeMode::Add, None, 1, &drawn)).unwrap(),
        Err(RangeRefusal::NoChange)
    );
    let removed = edit(&m, &o, &id, &range(RangeMode::Remove, None, 1, &drawn))
        .unwrap()
        .unwrap();
    assert_eq!(removed.changed, 3);
    let after = saved(&m, &id);
    assert_eq!(after[&1], before[&1]);
    assert_eq!(after[&2], before[&2]);
    assert!(
        !after.contains_key(&3),
        "a page left without boxes leaves the list"
    );
    // Nothing matches any more.
    assert_eq!(
        edit(&m, &o, &id, &range(RangeMode::Remove, None, 1, &drawn)).unwrap(),
        Err(RangeRefusal::NoMatch)
    );
    // The base box exists on pages 1 and 2 only: page 3 is counted, not refused.
    let outcome = edit(
        &m,
        &o,
        &id,
        &range(RangeMode::Remove, None, 1, &[[1., 2., 4., 5.]]),
    )
    .unwrap()
    .unwrap();
    assert_eq!((outcome.changed, outcome.unmatched), (2, 1));
    assert!(
        m.read(&id).unwrap().exclusion.is_none(),
        "an empty set is never stored"
    );
}

#[test]
fn clear_removes_every_box_keeps_the_fingerprints_and_an_empty_result_is_a_removal_event() {
    let (_dir, m, o, _f, id) = rendered_upload(3);
    let u = m.read(&id).unwrap();
    assert_eq!(
        edit(&m, &o, &id, &range(RangeMode::Clear, None, 1, &[])).unwrap(),
        Err(RangeRefusal::NoExclusion)
    );
    let proposals = m.propose_exclusions(&o.auth, &id, u.revision).unwrap();
    m.set_exclusion(
        &o,
        &id,
        u.revision,
        0,
        &confirming(&proposals[0].fingerprint),
    )
    .unwrap();
    edit(&m, &o, &id, &range(RangeMode::Clear, Some(&[1]), 1, &[]))
        .unwrap()
        .unwrap();
    let kept = m.read(&id).unwrap().exclusion.unwrap();
    assert_eq!(kept.pages, [2, 3]);
    assert_eq!(
        kept.proposal_fingerprints,
        [proposals[0].fingerprint.clone()]
    );
    // Pages that have no box change nothing.
    assert_eq!(
        edit(&m, &o, &id, &range(RangeMode::Clear, Some(&[1]), 1, &[])).unwrap(),
        Err(RangeRefusal::NoChange)
    );
    let outcome = edit(&m, &o, &id, &range(RangeMode::Clear, None, 1, &[]))
        .unwrap()
        .unwrap();
    assert_eq!(outcome.changed, 2);
    let after = m.read(&id).unwrap();
    assert!(after.exclusion.is_none());
    assert_eq!(after.exclusion_revision, 3);
    let (removed, text): (bool, String) =
        m.0.store
            .upload_storage(|db| {
                Ok(db.query_row(
                "SELECT removed,pages FROM source_upload_exclusions ORDER BY revision DESC LIMIT 1",
                [],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )?)
            })
            .unwrap();
    assert!(removed);
    assert_eq!(text, "[2,3]", "the removal keeps the last boxes");
}

#[test]
fn a_stale_exclusion_revision_is_refused_and_a_windowed_edit_keeps_the_other_screens() {
    let (_dir, m, o, _f, id) = rendered_upload(150);
    page_sizes(&m, &(1..=150).map(|n| (n, 600., 800.)).collect::<Vec<_>>());
    let u = m.read(&id).unwrap();
    let drawn = [[10., 20., 40., 50.]];
    let all = range(RangeMode::Add, None, 1, &drawn);
    m.set_exclusion_range(&o, &id, u.revision, 0, &all)
        .unwrap()
        .unwrap();
    // A screen form opened before the range edit still carries revision 0.
    assert!(matches!(
        m.set_exclusion_range(&o, &id, u.revision, 0, &all),
        Err(AppError::Conflict)
    ));
    assert!(matches!(
        m.set_exclusion(&o, &id, u.revision, 0, &exclusion()),
        Err(AppError::Conflict)
    ));
    assert_eq!(saved(&m, &id).len(), 150);
    // A windowed edit of screen one keeps the boxes of page 101 and on.
    let screen = exclusions::ExclusionRequest {
        manual: [(5, vec![[1., 1., 3., 3.]])].into(),
        window: Some((1, 100)),
        rationale: "screen".into(),
        ..Default::default()
    };
    m.set_exclusion(&o, &id, u.revision, 1, &screen).unwrap();
    let stored = saved(&m, &id);
    assert_eq!(stored.len(), 51);
    assert_eq!(stored[&5], [[1., 1., 3., 3.]]);
    assert_eq!(stored[&101], [[10., 20., 40., 50.]]);
    assert_eq!(stored[&150], [[10., 20., 40., 50.]]);
}

#[test]
fn twenty_thousand_pages_take_eight_boxes_each_within_the_history_bound() {
    let (_dir, m, o, id) = rendered_twenty_thousand();
    let page = m.read(&id).unwrap().pages[0].clone();
    let (width, height) = (page.width_points, page.height_points);
    let boxes: Vec<[f64; 4]> = (0..8)
        .map(|i| {
            let i = f64::from(i);
            [
                width * (0.05 + i * 0.01),
                height * (0.1 + i * 0.01),
                width * (0.5 + i * 0.01),
                height * (0.2 + i * 0.01),
            ]
        })
        .collect();
    let outcome = edit(&m, &o, &id, &range(RangeMode::Add, None, 1, &boxes))
        .unwrap()
        .unwrap();
    assert_eq!(outcome.changed, 20_000);
    let stored = saved(&m, &id);
    assert_eq!(stored.len(), 20_000);
    assert!(stored.values().all(|b| b.len() == 8));
    assert_eq!(m.read(&id).unwrap().exclusion_revision, 1);
}

#[test]
fn after_a_removal_a_range_add_starts_a_fresh_exclusion_and_remove_and_clear_find_none() {
    let (_dir, m, o, _f, id) = rendered_upload(3);
    let u = m.read(&id).unwrap();
    let proposals = m.propose_exclusions(&o.auth, &id, u.revision).unwrap();
    m.set_exclusion(
        &o,
        &id,
        u.revision,
        0,
        &confirming(&proposals[0].fingerprint),
    )
    .unwrap();
    m.remove_exclusion(&o, &id, u.revision, 1, "removed")
        .unwrap();
    let box_ = [[1., 1., 5., 5.]];
    for mode in [RangeMode::Remove, RangeMode::Clear] {
        assert_eq!(
            edit(&m, &o, &id, &range(mode, None, 1, &box_)).unwrap(),
            Err(RangeRefusal::NoExclusion)
        );
    }
    assert_eq!(m.read(&id).unwrap().exclusion_revision, 2);
    edit(&m, &o, &id, &range(RangeMode::Add, None, 1, &box_))
        .unwrap()
        .unwrap();
    let after = m.read(&id).unwrap();
    assert_eq!(after.exclusion_revision, 3);
    let fresh = after.exclusion.unwrap();
    assert_eq!(fresh.proposal_fingerprints, Vec::<String>::new());
    assert_eq!(fresh.boxes.len(), 3);
    assert!(fresh.boxes.values().all(|b| b == &box_));
}

#[test]
fn distinct_sizes_are_counted_on_the_raw_page_size_not_the_rounded_room() {
    let (_dir, m, o, _f, id) = rendered_upload(2);
    // Both round up to the same 0.01 pt room (600.01), but they are two sizes.
    page_sizes(&m, &[(1, 600.001, 800.), (2, 600.002, 800.)]);
    let outcome = edit(
        &m,
        &o,
        &id,
        &range(RangeMode::Add, None, 1, &[[1., 1., 5., 5.]]),
    )
    .unwrap()
    .unwrap();
    assert_eq!((outcome.changed, outcome.sizes), (2, 2));
}
