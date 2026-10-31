//! Blocking staged-upload API. Call from a dedicated thread or `spawn_blocking`.
//! The manager is process-wide; job publication and deletion share one lock.
use super::{
    staging::{Files, Ledger, MIB, RENDER_RESERVATION, Reservation, UPLOAD_RESERVATION},
    worker::{self, Cancel, Inspection, Page, Reason, RenderSettings, RenderedPage},
};
use crate::{
    AppError, Store,
    auth::{CsrfAdmission, RequestAuth},
};
use rusqlite::{Connection, OptionalExtension, Transaction, TransactionBehavior, params};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::io::{Read, Seek};
use std::os::unix::fs::MetadataExt;
use std::{
    path::Path,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    },
    thread::JoinHandle,
    time::{Duration, SystemTime, UNIX_EPOCH},
};

static MANAGER_OPEN: AtomicBool = AtomicBool::new(false);

#[cfg(test)]
#[path = "upload_tests.rs"]
mod tests;

/// Constructed only by authenticated handlers; a caller cannot supply an actor label.
#[derive(Clone)]
pub struct OwnerWrite {
    auth: RequestAuth,
    csrf: CsrfAdmission,
}
impl OwnerWrite {
    #[expect(
        dead_code,
        reason = "Slice 2 constructs admissions in the HTTP handlers"
    )]
    pub(crate) fn new(auth: RequestAuth, csrf: CsrfAdmission) -> Self {
        Self { auth, csrf }
    }
    fn check(&self, db: &Connection) -> Result<(), AppError> {
        self.csrf.recheck_owner_write(db, &self.auth).map(|_| ())
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Metadata {
    pub system_id: String,
    pub edition: String,
    pub language: String,
}
impl Metadata {
    fn validate(&self, db: &Connection) -> Result<(), AppError> {
        if !crate::registry::valid_id(&self.system_id)
            || self.edition.trim().is_empty()
            || self.edition.len() > 100
            || self.language.trim().is_empty()
            || self.language.len() > 30
            || self.edition.contains('\0')
            || self.language.contains('\0')
        {
            return Err(AppError::Invalid("Invalid source metadata."));
        }
        crate::registry::source_identity(db, &self.system_id, &self.edition, None)?;
        Ok(())
    }
}

/// Only validated results may cross this interface. Production uses the 3a pipe validator.
pub trait Worker: Send + Sync + 'static {
    /// # Errors
    /// Returns bounded document, cancellation or confinement failures.
    fn inspect(&self, path: &Path, cancel: &Cancel) -> Result<Inspection, Reason>;
    /// # Errors
    /// Returns bounded document, cancellation, callback or confinement failures.
    fn render(
        &self,
        path: &Path,
        pages: &[Page],
        cancel: &Cancel,
        publish: &mut (dyn FnMut(RenderedPage) -> Result<(), worker::Error> + Send),
    ) -> Result<(), Reason>;
}
struct PdfWorker;
impl Worker for PdfWorker {
    fn inspect(&self, path: &Path, cancel: &Cancel) -> Result<Inspection, Reason> {
        worker::inspect_cancellable(path, cancel)
    }
    fn render(
        &self,
        path: &Path,
        pages: &[Page],
        cancel: &Cancel,
        publish: &mut (dyn FnMut(RenderedPage) -> Result<(), worker::Error> + Send),
    ) -> Result<(), Reason> {
        worker::render(path, pages.to_vec(), cancel, publish)
    }
}

struct Running {
    cancel: Cancel,
    handle: JoinHandle<()>,
    cancelled: Arc<AtomicBool>,
}
#[cfg(test)]
type PublishHook = Box<dyn FnOnce(&Store) + Send>;
struct Inner {
    store: Store,
    files: Arc<Files>,
    ledger: Arc<Ledger>,
    worker: Arc<dyn Worker>,
    gate: Mutex<()>,
    control: Mutex<Option<Running>>,
    streaming: Arc<AtomicBool>,
    singleton: bool,
    #[cfg(test)]
    before_publish: Mutex<Option<PublishHook>>,
}
impl Drop for Inner {
    fn drop(&mut self) {
        if self.singleton {
            MANAGER_OPEN.store(false, Ordering::Release);
        }
    }
}

#[derive(Clone)]
pub struct Uploads(Arc<Inner>);
pub struct UploadPhase {
    manager: Arc<Inner>,
    reservation: Reservation,
    slot: Arc<AtomicBool>,
    files: Arc<Files>,
    name: Option<String>,
    file: std::fs::File,
}
impl UploadPhase {
    /// The route writes its bounded counting stream here while calculating the hash.
    pub fn file(&mut self) -> &mut std::fs::File {
        &mut self.file
    }
}
impl Drop for UploadPhase {
    fn drop(&mut self) {
        if let Some(name) = self.name.take()
            && self.files.remove_spool(&name).is_err()
        {
            if let Ok(metadata) = self.file.metadata()
                && metadata.nlink() != 0
                && self.reservation.written(metadata.len()).is_err()
            {
                eprintln!("Source staging accounting requires reconciliation.");
            }
            eprintln!("Source upload partial-file cleanup failed.");
        }
        self.slot.store(false, Ordering::Release);
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct PageModel {
    pub renderer_crate_version: String,
    pub mupdf_version: String,
    pub pdf_page: u32,
    pub rendered_revision: i64,
    pub png_sha256: String,
    pub png_bytes: u64,
    pub width_px: u32,
    pub height_px: u32,
    pub dpi: u16,
    pub width_points: f64,
    pub height_points: f64,
    pub transform: [f64; 6],
    pub stats: worker::records::Stats,
    pub settings: RenderSettings,
    pub text_sha256: String,
    pub text_bytes: u64,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Job {
    pub revision: i64,
    pub kind: String,
    pub state: String,
    pub done: u32,
    pub total: u32,
    pub reason: Option<String>,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Upload {
    pub already_document: Option<i64>,
    pub id: String,
    pub pdf_sha256: String,
    pub pdf_bytes: u64,
    pub revision: i64,
    pub state: String,
    pub metadata: Metadata,
    pub inspection: Option<InspectionData>,
    pub selected_pages: Vec<u32>,
    pub pages: Vec<PageModel>,
    pub job: Job,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct InspectionData {
    pub document: worker::records::Document,
    pub pages: Vec<Page>,
}
impl From<Inspection> for InspectionData {
    fn from(i: Inspection) -> Self {
        Self {
            document: i.document,
            pages: i.pages,
        }
    }
}

#[derive(Debug, Serialize, Deserialize)]
pub struct StagedContract {
    pub contract_version: String,
    pub pdf_sha256: String,
    pub pdf_bytes: u64,
    pub renderer: Renderer,
    pub revision: i64,
    pub selected_pages: Vec<u32>,
    pub pages: Vec<PageModel>,
    pub metadata: Metadata,
    pub limits: SourceLimits,
    pub exclusion: Option<serde_json::Value>,
}
#[derive(Debug, Serialize, Deserialize)]
pub struct Renderer {
    pub crate_version: String,
    pub mupdf_version: String,
    pub settings: RenderSettings,
}
#[derive(Debug, Serialize, Deserialize)]
pub struct SourceLimits {
    pub pages: u32,
    pub png_bytes: u64,
    pub units: u32,
}

fn now() -> Result<i64, AppError> {
    i64::try_from(
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_err(|_| AppError::Worker)?
            .as_secs(),
    )
    .map_err(|_| AppError::Worker)
}
fn hash(bytes: &[u8]) -> String {
    hex::encode(Sha256::digest(bytes))
}
fn json<T: Serialize>(v: &T) -> Result<String, AppError> {
    Ok(serde_json::to_string(v)?)
}
fn parse<T: serde::de::DeserializeOwned>(s: &str) -> Result<T, AppError> {
    Ok(serde_json::from_str(s)?)
}

impl Uploads {
    /// Open once at startup, before accepting uploads. Reconciles interrupted work and files.
    /// # Errors
    /// Returns storage, reconciliation or duplicate-manager failures.
    pub fn open(store: Store, data: &Path) -> Result<Self, AppError> {
        if MANAGER_OPEN.swap(true, Ordering::AcqRel) {
            return Err(AppError::Conflict);
        }
        match Self::initialize(store, data, Arc::new(PdfWorker), true) {
            Ok(m) => Ok(m),
            Err(e) => {
                MANAGER_OPEN.store(false, Ordering::Release);
                Err(e)
            }
        }
    }
    fn initialize(
        store: Store,
        data: &Path,
        worker: Arc<dyn Worker>,
        singleton: bool,
    ) -> Result<Self, AppError> {
        let files = Arc::new(Files::open(data)?);
        let used=store.upload_storage(|db| {
            let tx=db.transaction_with_behavior(TransactionBehavior::Immediate)?;
            tx.execute("UPDATE source_uploads SET state='failed' WHERE id IN (SELECT upload_id FROM source_jobs WHERE state IN ('queued','running'))",[])?;
            tx.execute("UPDATE source_jobs SET state='failed',reason='interrupted',finished_at=?1 WHERE state IN ('queued','running')",[now()?])?;
            tx.execute("DELETE FROM source_uploads WHERE updated_at<?1",[now()?-7*86400])?;
            tx.commit()?;
            let mut keep=std::collections::HashSet::new();
            let mut q=db.prepare("SELECT id FROM source_uploads")?;
            for id in q.query_map([],|r|r.get::<_,String>(0))? { let id=id?; keep.insert((id.clone(),String::new())); keep.insert((id,"document.pdf".into())); }
            let mut q=db.prepare("SELECT upload_id,pdf_page FROM source_upload_pages")?;
            for p in q.query_map([],|r|Ok((r.get::<_,String>(0)?,r.get::<_,u32>(1)?)))? { let (id,n)=p?; keep.insert((id.clone(),format!("page-{n}.png"))); keep.insert((id,format!("text-{n}.json"))); }
            files.reconcile(|id,n|keep.contains(&(id.into(),n.into())))
        })?;
        let disk = files.clone();
        let ledger = Ledger::new(used, move || disk.free());
        let m = Self(Arc::new(Inner {
            store,
            files,
            ledger,
            worker,
            gate: Mutex::new(()),
            control: Mutex::new(None),
            streaming: Arc::new(AtomicBool::new(false)),
            singleton,
            #[cfg(test)]
            before_publish: Mutex::new(None),
        }));
        let weak = Arc::downgrade(&m.0);
        std::thread::Builder::new()
            .name("source-sweep".into())
            .spawn(move || {
                loop {
                    std::thread::sleep(Duration::from_secs(3600));
                    let Some(inner) = weak.upgrade() else { break };
                    if Self(inner).sweep().is_err() {
                        tracing_sweep_failure();
                    }
                }
            })?;
        Ok(m)
    }
    /// Reserve before polling an upload body. Dropping this guard releases its slot and bytes.
    /// # Errors
    /// Refuses a second stream/upload, insufficient free space, or a full staging budget.
    pub fn begin_upload(&self) -> Result<UploadPhase, AppError> {
        if self.0.streaming.swap(true, Ordering::AcqRel) {
            return Err(AppError::Conflict);
        }
        let result = (|| {
            self.0.store.upload_storage(|db| {
                if db.query_row("SELECT count(*) FROM source_uploads", [], |r| {
                    r.get::<_, i64>(0)
                })? != 0
                {
                    return Err(AppError::Conflict);
                }
                Ok(())
            })?;
            let reservation = self.0.ledger.reserve(UPLOAD_RESERVATION)?;
            let (name, file) = self.0.files.spool()?;
            Ok(UploadPhase {
                manager: self.0.clone(),
                reservation,
                slot: self.0.streaming.clone(),
                files: self.0.files.clone(),
                name: Some(name),
                file,
            })
        })();
        if result.is_err() {
            self.0.streaming.store(false, Ordering::Release);
        }
        result
    }
    fn write<T>(
        &self,
        owner: &OwnerWrite,
        f: impl FnOnce(&Transaction<'_>) -> Result<T, AppError>,
    ) -> Result<T, AppError> {
        self.0.store.upload_storage(|db| {
            let tx = db.transaction_with_behavior(TransactionBehavior::Immediate)?;
            owner.check(&tx)?;
            let value = f(&tx)?;
            owner.check(&tx)?;
            tx.commit()?;
            Ok(value)
        })
    }
    /// Publish an already spooled and hashed regular file; no PDF parsing occurs here.
    /// # Errors
    /// Refuses invalid metadata, stale authority, hash/size/magic mismatch, or storage failure.
    pub fn create(
        &self,
        owner: &OwnerWrite,
        mut phase: UploadPhase,
        sha256: &str,
        size: u64,
        metadata: &Metadata,
    ) -> Result<String, AppError> {
        let control = self.0.control.lock().map_err(|_| AppError::Worker)?;
        let gate = self.0.gate.lock().map_err(|_| AppError::Worker)?;
        if !Arc::ptr_eq(&phase.manager, &self.0) {
            return Err(AppError::Invalid("Wrong upload reservation."));
        }
        self.write(owner, |tx| metadata.validate(tx))?;
        verify_spool(&mut phase.file, sha256, size)?;
        let mut random = [0; 16];
        getrandom::fill(&mut random).map_err(|_| AppError::Worker)?;
        let id = hex::encode(random);
        self.0.files.create(&id)?;
        let result = (|| {
            self.0.files.publish_spool(
                phase.name.as_deref().ok_or(AppError::Worker)?,
                &phase.file,
                &id,
                &mut phase.reservation,
            )?;
            phase.name = None;
            self.write(owner,|tx| {
                metadata.validate(tx)?;
                let t=now()?;
                tx.execute("INSERT INTO source_uploads(id,pdf_sha256,pdf_bytes,actor,actor_id,actor_epoch,session_digest,created_at,updated_at,state,system_id,edition,language) VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?8,'uploaded',?9,?10,?11)",params![id,sha256,size,format!("account:{}",owner.auth.account_id()),owner.auth.account_id(),owner.auth.epoch(),owner.auth.digest(),t,metadata.system_id,metadata.edition,metadata.language])?;
                queue(tx,&id,1,"inspect",0)?; Ok(())
            })
        })();
        if let Err(e) = result {
            self.0.files.discard(&id, &self.0.ledger)?;
            return Err(e);
        }
        drop(gate);
        drop(control);
        self.launch(&id)?;
        Ok(id)
    }
    /// Read the staged page model. Handlers must establish Owner read admission separately.
    /// # Errors
    /// Missing/corrupt rows or storage failures are returned without truncation.
    pub fn read(&self, id: &str) -> Result<Upload, AppError> {
        self.0.store.upload_storage(|db| {
            let tx = db.transaction()?;
            let upload = load(&tx, id)?;
            tx.commit()?;
            Ok(upload)
        })
    }
    #[cfg_attr(
        not(test),
        expect(dead_code, reason = "Slice 2 wires the Owner-only GET handler")
    )]
    pub(crate) fn read_for_owner(&self, auth: &RequestAuth, id: &str) -> Result<Upload, AppError> {
        self.0.store.upload_storage(|db| {
            let tx = db.transaction()?;
            auth.recheck_owner(&tx)?;
            let upload = load(&tx, id)?;
            auth.recheck_owner(&tx)?;
            tx.commit()?;
            Ok(upload)
        })
    }
    /// Update metadata/selection with optimistic concurrency, retaining current selected renders.
    /// # Errors
    /// Invalid selection, stale revision/admission, cancellation/reaping or storage failure.
    pub fn set(
        &self,
        owner: &OwnerWrite,
        id: &str,
        revision: i64,
        metadata: &Metadata,
        selected: &[u32],
    ) -> Result<(), AppError> {
        let mut control = self.0.control.lock().map_err(|_| AppError::Worker)?;
        self.write(owner, |tx| {
            let u = load(tx, id)?;
            check_selection(&u, revision, selected)?;
            metadata.validate(tx)
        })?;
        self.stop_locked(&mut control, owner, id)?;
        let _gate = self.0.gate.lock().map_err(|_| AppError::Worker)?;
        let removed=self.write(owner,|tx| {
            let mut u=load(tx,id)?; check_selection(&u,revision,selected)?; metadata.validate(tx)?;
            let removed=u.pages.iter().filter(|p|!selected.contains(&p.pdf_page)).map(|p|p.pdf_page).collect::<Vec<_>>();
            for n in &removed {tx.execute("DELETE FROM source_upload_pages WHERE upload_id=?1 AND pdf_page=?2",params![id,n])?;}
            u.selected_pages=selected.to_vec();
            tx.execute("UPDATE source_uploads SET system_id=?2,edition=?3,language=?4,selected_pages=?5,revision=revision+1,updated_at=?6,state=?7 WHERE id=?1",params![id,metadata.system_id,metadata.edition,metadata.language,json(&selected)?,now()?,settled_state(&u)])?;
            Ok(removed)
        })?;
        for n in removed {
            self.remove_page(id, n)?;
        }
        Ok(())
    }
    /// Queue inspection again or render only selected pages lacking current settings.
    /// # Errors
    /// Invalid state/admission or busy/stale jobs and storage failures.
    pub fn start(
        &self,
        owner: &OwnerWrite,
        id: &str,
        revision: i64,
        inspect: bool,
    ) -> Result<(), AppError> {
        let mut control = self.0.control.lock().map_err(|_| AppError::Worker)?;
        if control.as_ref().is_some_and(|r| !r.handle.is_finished()) {
            return Err(AppError::Conflict);
        }
        if let Some(r) = control.take() {
            r.handle.join().map_err(|_| AppError::Worker)?;
        }
        let gate = self.0.gate.lock().map_err(|_| AppError::Worker)?;
        self.write(owner, |tx| {
            let u = load(tx, id)?;
            if u.revision != revision
                || (!inspect && (u.inspection.is_none() || u.selected_pages.is_empty()))
            {
                return Err(AppError::Conflict);
            }
            let total = if inspect { 0 } else { remaining(&u).len() };
            queue(
                tx,
                id,
                revision,
                if inspect { "inspect" } else { "render" },
                total,
            )?;
            tx.execute(
                "UPDATE source_uploads SET updated_at=?2,state=?3 WHERE id=?1",
                params![id, now()?, if inspect { "uploaded" } else { "rendering" }],
            )?;
            Ok(())
        })?;
        drop(gate);
        drop(control);
        self.launch(id)
    }
    /// Cancel and reap before returning. Published pages remain.
    /// # Errors
    /// Returns admission, reaping or storage failures.
    pub fn cancel(&self, owner: &OwnerWrite, id: &str) -> Result<(), AppError> {
        let mut control = self.0.control.lock().map_err(|_| AppError::Worker)?;
        self.stop_locked(&mut control, owner, id)
    }
    fn stop_locked(
        &self,
        control: &mut Option<Running>,
        owner: &OwnerWrite,
        id: &str,
    ) -> Result<(), AppError> {
        self.write(owner, |tx| {
            load(tx, id)?;
            tx.execute("UPDATE source_jobs SET state='cancelled',reason='cancelled',finished_at=?2 WHERE upload_id=?1 AND state='queued'",params![id,now()?])?;
            Ok(())
        })?;
        if let Some(r) = control.take() {
            r.cancelled.store(true, Ordering::Release);
            r.cancel.cancel();
            r.handle.join().map_err(|_| AppError::Worker)?;
        }
        self.write(owner,|tx| {
            tx.execute("UPDATE source_jobs SET state='cancelled',reason='cancelled',finished_at=?2 WHERE upload_id=?1 AND state IN ('queued','running')",params![id,now()?])?;
            tx.execute("UPDATE source_uploads SET updated_at=?2,state=CASE WHEN state='rendering' THEN CASE WHEN inspection IS NULL THEN 'uploaded' ELSE 'inspected' END ELSE state END WHERE id=?1",params![id,now()?])?;
            Ok(())
        })
    }
    /// Cancel/reap, delete rows (cascade), then remove all private files.
    /// # Errors
    /// Returns admission, reaping or storage failures.
    pub fn discard(&self, owner: &OwnerWrite, id: &str) -> Result<(), AppError> {
        let mut control = self.0.control.lock().map_err(|_| AppError::Worker)?;
        self.stop_locked(&mut control, owner, id)?;
        let _gate = self.0.gate.lock().map_err(|_| AppError::Worker)?;
        self.write(owner, |tx| {
            tx.execute("DELETE FROM source_uploads WHERE id=?1", [id])?;
            Ok(())
        })?;
        self.0.files.discard(id, &self.0.ledger)
    }
    /// Expire inactive uploads. A running worker is never swept.
    /// # Errors
    /// Returns storage or filesystem failures.
    pub fn sweep(&self) -> Result<(), AppError> {
        let _control = self.0.control.lock().map_err(|_| AppError::Worker)?;
        let _gate = self.0.gate.lock().map_err(|_| AppError::Worker)?;
        let ids=self.0.store.upload_storage(|db| {
            let tx=db.transaction_with_behavior(TransactionBehavior::Immediate)?;
            let ids=tx.prepare("SELECT id FROM source_uploads WHERE updated_at<?1 AND NOT EXISTS(SELECT 1 FROM source_jobs WHERE upload_id=id AND state='running')")?.query_map([now()?-7*86400],|r|r.get::<_,String>(0))?.collect::<Result<Vec<_>,_>>()?;
            for id in &ids {tx.execute("DELETE FROM source_uploads WHERE id=?1",[id])?;}
            tx.commit()?; Ok(ids)
        })?;
        for id in ids {
            self.0.files.discard(&id, &self.0.ledger)?;
        }
        Ok(())
    }
    /// Build the step-4 input solely from staging rows, without reparsing the PDF.
    /// # Errors
    /// Refuses incomplete selection, settings mismatch, over-budget images or corrupt rows.
    pub fn contract(&self, id: &str) -> Result<StagedContract, AppError> {
        let u = self.read(id)?;
        if u.selected_pages.is_empty()
            || !remaining(&u).is_empty()
            || u.pages.iter().map(|p| p.png_bytes).sum::<u64>() > 128 * MIB
        {
            return Err(AppError::Conflict);
        }
        u.inspection.as_ref().ok_or(AppError::Conflict)?;
        Ok(StagedContract {
            contract_version: "mimik-staged-v1".into(),
            pdf_sha256: u.pdf_sha256,
            pdf_bytes: u.pdf_bytes,
            renderer: Renderer {
                crate_version: worker::CRATE_VERSION.into(),
                mupdf_version: worker::MUPDF_VERSION.into(),
                settings: RenderSettings::default(),
            },
            revision: u.revision,
            selected_pages: u.selected_pages.clone(),
            pages: u
                .pages
                .into_iter()
                .filter(|p| u.selected_pages.contains(&p.pdf_page))
                .collect(),
            metadata: u.metadata,
            limits: SourceLimits {
                pages: 64,
                png_bytes: 128 * MIB,
                units: 20000,
            },
            exclusion: None,
        })
    }
    fn remove_page(&self, id: &str, n: u32) -> Result<(), AppError> {
        self.0
            .files
            .remove(id, &format!("page-{n}.png"), &self.0.ledger)?;
        self.0
            .files
            .remove(id, &format!("text-{n}.json"), &self.0.ledger)
    }
    fn launch(&self, id: &str) -> Result<(), AppError> {
        let mut control = self.0.control.lock().map_err(|_| AppError::Worker)?;
        if let Some(r) = control.take() {
            if !r.handle.is_finished() {
                *control = Some(r);
                return Err(AppError::Conflict);
            }
            r.handle.join().map_err(|_| AppError::Worker)?;
        }
        let manager = self.clone();
        let id = id.to_owned();
        let cancel = Cancel::default();
        let c = cancel.clone();
        let cancelled = Arc::new(AtomicBool::new(false));
        let flag = cancelled.clone();
        let handle = std::thread::Builder::new()
            .name("source-upload".into())
            .spawn(move || manager.run_job(&id, &c, &flag))?;
        *control = Some(Running {
            cancel,
            handle,
            cancelled,
        });
        Ok(())
    }
    fn run_job(&self, id: &str, cancel: &Cancel, cancelled: &AtomicBool) {
        let mut claimed_revision = None;
        let result = (|| {
            let u = {
                let _gate = self.0.gate.lock().map_err(|_| AppError::Worker)?;
                self.0.store.upload_storage(|db| {
                    let tx=db.transaction_with_behavior(TransactionBehavior::Immediate)?;
                    if tx.execute("UPDATE source_jobs SET state='running',started_at=?2 WHERE upload_id=?1 AND state='queued'",params![id,now()?])?!=1 {return Err(AppError::Conflict);}
                    let u=load(&tx,id)?; tx.commit()?; Ok(u)
                })?
            };
            claimed_revision = Some(u.revision);
            let path = self.0.files.path(id)?;
            if u.job.kind == "inspect" {
                let inspection: InspectionData = self
                    .0
                    .worker
                    .inspect(&path, cancel)
                    .map_err(|r| reason_error(&r))?
                    .into();
                if cancelled.load(Ordering::Acquire) {
                    return Err(AppError::Invalid("cancelled"));
                }
                let encoded = json(&inspection)?;
                if encoded.len() > worker::records::INSPECT_LIMIT {
                    return Err(AppError::Invalid("too_large"));
                }
                let _gate = self.0.gate.lock().map_err(|_| AppError::Worker)?;
                self.0.store.upload_storage(|db| {
                    db.execute("UPDATE source_uploads SET inspection=?3,state='inspected',updated_at=?4 WHERE id=?1 AND revision=?2 AND EXISTS(SELECT 1 FROM source_jobs WHERE upload_id=?1 AND state='running' AND upload_revision=?2)",params![id,u.revision,encoded,now()?])?; Ok(())
                })?;
            } else {
                let pages = remaining(&u);
                for batch in pages.chunks(8) {
                    if cancelled.load(Ordering::Acquire) {
                        return Err(AppError::Invalid("cancelled"));
                    }
                    let reservation = Mutex::new(self.0.ledger.reserve(RENDER_RESERVATION)?);
                    let callback_error = Mutex::new(None);
                    let result = self.0.worker.render(&path, batch, cancel, &mut |p| {
                        let mut reserved =
                            reservation.lock().map_err(|_| worker::Error::Unreadable)?;
                        let result = self.publish(id, u.revision, &p, &mut reserved, cancelled);
                        if let Err(e) = result {
                            *callback_error
                                .lock()
                                .map_err(|_| worker::Error::Unreadable)? = Some(e);
                            return Err(worker::Error::Unreadable);
                        }
                        Ok(())
                    });
                    if let Some(e) = callback_error.into_inner().map_err(|_| AppError::Worker)? {
                        return Err(e);
                    }
                    result.map_err(|r| reason_error(&r))?;
                }
            }
            Ok(())
        })();
        let finish = (|| {
            let _gate = self.0.gate.lock().map_err(|_| AppError::Worker)?;
            self.0.store.upload_storage(|db| {
                let tx=db.transaction_with_behavior(TransactionBehavior::Immediate)?;
                let u=load(&tx,id)?;
                if u.job.state != "running" { return Ok(()); }
                if claimed_revision != Some(u.job.revision) { return Ok(()); }
                if u.revision != u.job.revision {
                    tx.execute("UPDATE source_jobs SET state='cancelled',reason='cancelled',finished_at=?2 WHERE upload_id=?1",params![id,now()?])?;
                    tx.commit()?;
                    return Ok(());
                }
                let (state,reason)=if cancelled.load(Ordering::Acquire) {("cancelled",Some("cancelled"))} else if let Err(ref e)=result {("failed",Some(error_code(e)))} else {("succeeded",None)};
                tx.execute("UPDATE source_jobs SET state=?2,reason=?3,finished_at=?4 WHERE upload_id=?1 AND state='running'",params![id,state,reason,now()?])?;
                let upload_state=if state=="failed" {"failed"} else {settled_state(&u)};
                tx.execute("UPDATE source_uploads SET state=?2,updated_at=?3 WHERE id=?1",params![id,upload_state,now()?])?;
                tx.commit()?; Ok(())
            })
        })();
        if finish.is_err() {
            tracing_sweep_failure();
        }
    }
    fn publish(
        &self,
        id: &str,
        revision: i64,
        p: &RenderedPage,
        reservation: &mut Reservation,
        cancelled: &AtomicBool,
    ) -> Result<(), AppError> {
        let _gate = self.0.gate.lock().map_err(|_| AppError::Worker)?;
        let u = self.read(id)?;
        if cancelled.load(Ordering::Acquire)
            || u.revision != revision
            || u.job.state != "running"
            || u.job.revision != revision
            || !u.selected_pages.contains(&p.n)
        {
            return Err(AppError::Conflict);
        }
        let page = u
            .inspection
            .as_ref()
            .and_then(|i| i.pages.iter().find(|page| page.n == p.n))
            .ok_or(AppError::Conflict)?;
        let text = worker::records::json(&p.lines).map_err(|_| AppError::Invalid("too_large"))?;
        let png_total = u
            .pages
            .iter()
            .filter(|r| r.pdf_page != p.n)
            .map(|r| r.png_bytes)
            .sum::<u64>()
            + p.png.len() as u64;
        let text_total = u
            .pages
            .iter()
            .filter(|r| r.pdf_page != p.n)
            .map(|r| r.text_bytes)
            .sum::<u64>()
            + text.len() as u64;
        if png_total > 128 * MIB {
            return Err(AppError::Invalid(
                "too large for one source: select fewer pages",
            ));
        }
        if text_total > 512 * MIB {
            return Err(AppError::Invalid("Not enough space."));
        }
        if u.pages.iter().any(|r| r.pdf_page == p.n) {
            self.0.store.upload_storage(|db| {
                db.execute(
                    "DELETE FROM source_upload_pages WHERE upload_id=?1 AND pdf_page=?2",
                    params![id, p.n],
                )?;
                Ok(())
            })?;
            self.remove_page(id, p.n)?;
        }
        let result = (|| {
            self.0
                .files
                .write(id, &format!("page-{}.png", p.n), &p.png, reservation)?;
            self.0
                .files
                .write(id, &format!("text-{}.json", p.n), &text, reservation)?;
            #[cfg(test)]
            if let Some(hook) = self
                .0
                .before_publish
                .lock()
                .map_err(|_| AppError::Worker)?
                .take()
            {
                hook(&self.0.store);
            }
            self.0.store.upload_storage(|db| {
                let tx=db.transaction_with_behavior(TransactionBehavior::Immediate)?;
                let changed=tx.execute("INSERT INTO source_upload_pages(upload_id,pdf_page,rendered_revision,png_sha256,png_bytes,width_px,height_px,dpi,width_points,height_points,transform,stats,settings,text_sha256,text_bytes,renderer_crate_version,mupdf_version) SELECT ?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13,?14,?15,?16,?17 WHERE EXISTS(SELECT 1 FROM source_uploads u JOIN source_jobs j ON j.upload_id=u.id WHERE u.id=?1 AND u.revision=?3 AND j.upload_revision=?3 AND j.state='running')",params![id,p.n,revision,hash(&p.png),p.png.len() as u64,p.width_px,p.height_px,p.dpi,page.width,page.height,json(&page.transform)?,json(&p.stats)?,json(&RenderSettings::default())?,hash(&text),text.len() as u64,worker::CRATE_VERSION,worker::MUPDF_VERSION])?;
                if changed!=1 {return Err(AppError::Conflict);}
                tx.execute("UPDATE source_jobs SET pages_done=pages_done+1 WHERE upload_id=?1",[id])?;
                tx.execute("UPDATE source_uploads SET updated_at=?2 WHERE id=?1",params![id,now()?])?;
                tx.commit()?;Ok(())
            })
        })();
        if result.is_err() {
            self.remove_page(id, p.n)?;
        }
        result
    }
}

fn tracing_sweep_failure() {
    eprintln!("Source staging maintenance failed; inspect local storage.");
}
fn verify_spool(file: &mut std::fs::File, sha256: &str, size: u64) -> Result<(), AppError> {
    if !(1..=128 * MIB).contains(&size) || file.metadata()?.len() != size {
        return Err(AppError::Invalid("Invalid spooled PDF."));
    }
    file.rewind()?;
    let mut digest = Sha256::new();
    let mut header = Vec::with_capacity(1024);
    let mut total = 0_u64;
    let mut buffer = [0; 16384];
    loop {
        let count = file.read(&mut buffer)?;
        if count == 0 {
            break;
        }
        total += count as u64;
        if total > size {
            return Err(AppError::Invalid("Invalid spooled PDF."));
        }
        header.extend_from_slice(&buffer[..count.min(1024 - header.len())]);
        digest.update(&buffer[..count]);
    }
    if total != size
        || hex::encode(digest.finalize()) != sha256
        || !header.windows(5).any(|s| s == b"%PDF-")
    {
        return Err(AppError::Invalid("Invalid spooled PDF."));
    }
    Ok(())
}
fn reason_error(r: &Reason) -> AppError {
    AppError::Invalid(match r {
        Reason::Cancelled => "cancelled",
        Reason::Unavailable(_) => "unavailable",
        Reason::Document(e) => match e {
            worker::Error::Unreadable => "unreadable",
            worker::Error::Timeout => "timeout",
            worker::Error::TooLarge => "too_large",
            worker::Error::TooManyPages => "too_many_pages",
            worker::Error::PasswordProtected => "password_protected",
            worker::Error::Unsupported => "unsupported",
        },
    })
}
fn error_code(e: &AppError) -> &'static str {
    match e {
        AppError::SourceIo(e) if matches!(e.raw_os_error(), Some(libc::ENOSPC | libc::EDQUOT)) => {
            "space"
        }
        AppError::Invalid("Not enough space.") => "space",
        AppError::Invalid("too large for one source: select fewer pages") => "source_png_limit",
        AppError::Invalid(
            s @ ("cancelled" | "unavailable" | "unreadable" | "timeout" | "too_large"
            | "too_many_pages" | "password_protected" | "unsupported"),
        ) => s,
        _ => "unreadable",
    }
}
fn queue(
    tx: &Transaction<'_>,
    id: &str,
    revision: i64,
    kind: &str,
    total: usize,
) -> Result<(), AppError> {
    tx.execute("INSERT INTO source_jobs(upload_id,upload_revision,kind,state,pages_done,pages_total,created_at) VALUES(?1,?2,?3,'queued',0,?4,?5) ON CONFLICT(upload_id) DO UPDATE SET upload_revision=excluded.upload_revision,kind=excluded.kind,state='queued',pages_done=0,pages_total=excluded.pages_total,reason=NULL,created_at=excluded.created_at,started_at=NULL,finished_at=NULL",params![id,revision,kind,total,now()?])?;
    Ok(())
}
fn remaining(u: &Upload) -> Vec<Page> {
    u.inspection
        .as_ref()
        .map(|i| {
            i.pages
                .iter()
                .filter(|p| {
                    u.selected_pages.contains(&p.n)
                        && !u.pages.iter().any(|r| {
                            r.pdf_page == p.n
                                && r.settings == RenderSettings::default()
                                && r.renderer_crate_version == worker::CRATE_VERSION
                                && r.mupdf_version == worker::MUPDF_VERSION
                        })
                })
                .cloned()
                .collect()
        })
        .unwrap_or_default()
}
fn settled_state(upload: &Upload) -> &'static str {
    if upload.inspection.is_none() {
        "uploaded"
    } else if !upload.selected_pages.is_empty() && remaining(upload).is_empty() {
        "rendered"
    } else {
        "inspected"
    }
}
fn check_selection(u: &Upload, revision: i64, selected: &[u32]) -> Result<(), AppError> {
    if revision != u.revision {
        return Err(AppError::Conflict);
    }
    let Some(i) = u.inspection.as_ref() else {
        return if selected.is_empty() {
            Ok(())
        } else {
            Err(AppError::Conflict)
        };
    };
    if selected.len() > 64
        || selected.windows(2).any(|p| p[0] >= p[1])
        || selected.iter().any(|n| !i.pages.iter().any(|p| p.n == *n))
    {
        return Err(AppError::Invalid(
            "Select at most 64 distinct pages in ascending order.",
        ));
    }
    Ok(())
}
fn load(db: &Connection, id: &str) -> Result<Upload, AppError> {
    let mut u=db.query_row("SELECT id,pdf_sha256,pdf_bytes,revision,state,system_id,edition,language,(SELECT id FROM source_documents WHERE pdf_sha256=source_uploads.pdf_sha256) FROM source_uploads WHERE id=?1",[id],|r|Ok(Upload {already_document:r.get(8)?,id:r.get(0)?,pdf_sha256:r.get(1)?,pdf_bytes:r.get(2)?,revision:r.get(3)?,state:r.get(4)?,metadata:Metadata {system_id:r.get(5)?,edition:r.get(6)?,language:r.get(7)?},inspection:None,selected_pages:Vec::new(),pages:Vec::new(),job:Job {revision:0,kind:String::new(),state:String::new(),done:0,total:0,reason:None}})).optional()?.ok_or(AppError::SourceNotFound)?;
    let (inspection, selected): (Option<String>, String) = db.query_row(
        "SELECT inspection,selected_pages FROM source_uploads WHERE id=?1",
        [id],
        |r| Ok((r.get(0)?, r.get(1)?)),
    )?;
    u.inspection = inspection.as_deref().map(parse).transpose()?;
    u.selected_pages = parse(&selected)?;
    u.job=db.query_row("SELECT upload_revision,kind,state,pages_done,pages_total,reason FROM source_jobs WHERE upload_id=?1",[id],|r|Ok(Job {revision:r.get(0)?,kind:r.get(1)?,state:r.get(2)?,done:r.get(3)?,total:r.get(4)?,reason:r.get(5)?}))?;
    let mut query=db.prepare("SELECT pdf_page,rendered_revision,png_sha256,png_bytes,width_px,height_px,dpi,width_points,height_points,transform,stats,settings,text_sha256,text_bytes,renderer_crate_version,mupdf_version FROM source_upload_pages WHERE upload_id=?1 ORDER BY pdf_page")?;
    let rows = query.query_map([id], |r| {
        Ok((
            PageModel {
                renderer_crate_version: r.get(14)?,
                mupdf_version: r.get(15)?,
                pdf_page: r.get(0)?,
                rendered_revision: r.get(1)?,
                png_sha256: r.get(2)?,
                png_bytes: r.get(3)?,
                width_px: r.get(4)?,
                height_px: r.get(5)?,
                dpi: r.get(6)?,
                width_points: r.get(7)?,
                height_points: r.get(8)?,
                transform: [0.; 6],
                stats: worker::records::Stats {
                    n: 0,
                    chars: 0,
                    unmapped: 0,
                    image_area: 0.,
                    fonts_substituted: Vec::new(),
                    fonts_skipped: 0,
                    incomplete: false,
                    dropped_lines: 0,
                },
                settings: RenderSettings::default(),
                text_sha256: r.get(12)?,
                text_bytes: r.get(13)?,
            },
            r.get::<_, String>(9)?,
            r.get::<_, String>(10)?,
            r.get::<_, String>(11)?,
        ))
    })?;
    for row in rows {
        let (mut page, transform, stats, settings) = row?;
        page.transform = parse(&transform)?;
        page.stats = parse(&stats)?;
        page.settings = parse(&settings)?;
        u.pages.push(page);
    }
    Ok(u)
}
