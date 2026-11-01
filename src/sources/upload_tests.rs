//! Generated synthetic data; ordinary tests use fake workers, ignored live tests use the host runner.
use super::*;
use crate::sources::staging::{self, MIB};
use crate::{AppError, Store, auth::RequestAuth};
use rusqlite::params;
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
    large: AtomicBool,
}
impl Worker for Fake {
    fn inspect(&self, _: &std::path::Path, _: &Cancel) -> Result<Inspection, Reason> {
        if self.inspect_failure.load(Ordering::Acquire) {
            return Err(worker::Error::Unsupported.into());
        }
        Ok(inspection())
    }
    fn render(
        &self,
        _: &std::path::Path,
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
            if self.fail.load(Ordering::Acquire) {
                return Err(worker::Error::Unreadable.into());
            }
        }
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
    phase.file().write_all(PDF).unwrap();
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
    let mut r = m.0.ledger.reserve(staging::UPLOAD_RESERVATION).unwrap();
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
    assert_eq!(files.reconcile(|_, _| false).unwrap(), 0);
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
    assert_eq!(files.reconcile(|_, _| false).unwrap(), PDF.len() as u64);
    assert!(!dir.path().join("staging/removable").exists());
    assert!(dir.path().join("staging/failed/data").exists());
    *files.fail_cleanup.lock().unwrap() = None;
    assert_eq!(files.reconcile(|_, _| false).unwrap(), 0);
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
    let ledger = staging::Ledger::new(staging::TOTAL - staging::UPLOAD_RESERVATION, || {
        Ok(u64::MAX)
    });
    let r = ledger.reserve(staging::UPLOAD_RESERVATION).unwrap();
    assert!(ledger.reserve(1).is_err());
    drop(r);
    assert!(ledger.reserve(staging::UPLOAD_RESERVATION + 1).is_err());
    let ledger = staging::Ledger::new(0, || Err(AppError::Invalid("injected statvfs failure")));
    assert!(ledger.reserve(1).is_err());
    assert_eq!(ledger.usage().unwrap(), (0, 0));
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
    assert!(
        m.create(&o, phase, &"0".repeat(64), PDF.len() as u64, &metadata())
            .is_err()
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
    assert_eq!(contract.limits.pages, 64);
    assert_eq!(contract.limits.units, 20000);
    assert_eq!(contract.limits.png_bytes, 128 * MIB);
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
    for selection in [
        vec![0],
        vec![71],
        vec![1, 1],
        vec![2, 1],
        (1..=65).collect(),
    ] {
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
    selected(&m, &o, &id, &(1..=64).collect::<Vec<_>>());
    assert_eq!(m.read(&id).unwrap().selected_pages.len(), 64);
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
    phase.file().write_all(PDF).unwrap();
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
        let output = std::process::Command::new("sh")
            .args(["-c", "umask 0477 && exec \"$@\"", "umask-child"])
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
    files
        .publish_spool(&name, &spool, ID, &mut reservation)
        .unwrap();
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
    phase.file().write_all(PDF).unwrap();
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
fn per_upload_png_and_text_budgets_release_reservations() {
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
    // Artificial metadata isolates the cumulative check without allocating 128 MiB fixtures.
    m.0.store.upload_storage(|db| {
        for n in 3..=12 {db.execute("INSERT INTO source_upload_pages SELECT upload_id,?1,rendered_revision,png_sha256,png_bytes,width_px,height_px,dpi,width_points,height_points,transform,json_set(stats,'$.n',?1),settings,text_sha256,text_bytes,renderer_crate_version,mupdf_version FROM source_upload_pages WHERE pdf_page=1",[n])?;}Ok(())
    }).unwrap();
    let mut r = m.0.ledger.reserve(staging::RENDER_RESERVATION).unwrap();
    assert!(matches!(
        m.publish(&id, 2, &rendered(2), &mut r, &AtomicBool::new(false)),
        Err(AppError::Invalid(
            "too large for one source: select fewer pages"
        ))
    ));
    assert!(m.contract(&id).is_err());
    drop(r);
    assert_eq!(m.0.ledger.usage().unwrap().1, 0);
    m.0.store.upload_storage(|db| {
        db.execute("UPDATE source_upload_pages SET png_bytes=1,text_bytes=8388608",[])?;
        for n in 13..=65 {db.execute("INSERT INTO source_upload_pages SELECT upload_id,?1,rendered_revision,png_sha256,png_bytes,width_px,height_px,dpi,width_points,height_points,transform,json_set(stats,'$.n',?1),settings,text_sha256,text_bytes,renderer_crate_version,mupdf_version FROM source_upload_pages WHERE pdf_page=1",[n])?;}
        Ok(())
    }).unwrap();
    let mut r = m.0.ledger.reserve(staging::RENDER_RESERVATION).unwrap();
    assert!(matches!(
        m.publish(&id, 2, &rendered(2), &mut r, &AtomicBool::new(false)),
        Err(AppError::Invalid("Not enough space."))
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
fn pdf_limit_duplicate_document_and_settings_currency() {
    let (_dir, m, o, f) = fixture();
    let mut phase = m.begin_upload().unwrap();
    phase.file().set_len(128 * MIB + 1).unwrap();
    assert!(
        m.create(&o, phase, &"0".repeat(64), 128 * MIB + 1, &metadata())
            .is_err()
    );
    assert_eq!(m.0.ledger.usage().unwrap(), (0, 0));
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
                "pdf_bytes=134217729",
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
                "pages_done=65",
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
            db.execute("INSERT INTO source_upload_exclusions(upload_id,revision,actor,actor_id,actor_epoch,session_digest,rationale,pages,boxes,created_at) VALUES(?1,1,'account:1',1,1,?2,'Synthetic rationale','[1]','{\"1\":[1,1,2,2]}',?3)",params![id,o.auth.digest(),now()?])?;
            for assignment in ["revision=0","actor='forged'","actor_epoch=0","session_digest='bad'","rationale=''","pages='[1,1]'","pages='[0]'","boxes='{}'","boxes='{\"1\":[1,1,5000,2]}'","removed=2","proposal_fingerprint='raw text'"] {
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
