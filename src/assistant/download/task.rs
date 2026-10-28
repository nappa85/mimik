//! The download task (`docs/model-downloads.md`): one actor owns the single
//! transfer and every command (Cancel, Discard, Resume, completion, shutdown), so they
//! serialise. All blocking file work runs off the async executor and no lock is held across it;
//! a Cancel is accepted at once even while a write or a hash is blocked.
use super::client::{Body, Client};
use super::disk::{Disk, DiskError, Fault};
use super::rows::{Row, Rows, State};
use crate::assistant::model::{Spec, parse_spec};
use std::{
    fs::File,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    },
    time::Duration,
};
use tokio::{
    sync::{mpsc, oneshot},
    time::Instant,
};

/// Held for the duration of a transfer and dropped at its end (the model-work gate).
pub(crate) type Permit = Box<dyn std::any::Any + Send>;

/// Tunables; tests shorten them.
#[derive(Clone, Copy, Debug)]
pub(crate) struct Config {
    /// Received bytes are written in pieces of at least this size.
    pub flush: usize,
    /// A transfer slower than `floor_bytes` per `floor_window` stops and waits for Resume.
    pub floor_bytes: u64,
    pub floor_window: Duration,
    /// Free space kept after the download.
    pub reserve: u64,
    /// A resumed partial is cut back to a multiple of this minus one of it (power-loss tail).
    pub chunk: u64,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            flush: 1 << 20,
            floor_bytes: 1 << 20,
            floor_window: Duration::from_mins(5),
            reserve: 1 << 30,
            chunk: 1 << 20,
        }
    }
}

/// Why a download is not running.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum Stop {
    Cancelled,
    Stalled,
    Failed(String),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum Phase {
    Idle,
    Running { received: u64, total: u64 },
    Verifying,
    Stopped(Stop),
    Done,
}

/// What the page shows of the download.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Status {
    pub id: Option<i64>,
    pub spec: String,
    pub phase: Phase,
}

enum Command {
    Start(Row, Permit),
    Resume(i64, Permit),
    Cancel,
    Discard(i64, oneshot::Sender<Result<(), String>>),
    Shutdown(oneshot::Sender<()>),
}

/// The handle the routes hold.
#[derive(Clone)]
pub(crate) struct Downloader {
    commands: mpsc::UnboundedSender<Command>,
    status: Arc<Mutex<Status>>,
    disk: Arc<Disk>,
    rows: Arc<dyn Rows>,
}

fn lock(status: &Mutex<Status>) -> std::sync::MutexGuard<'_, Status> {
    status
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

impl Downloader {
    /// Reconciles the disk with the rows, then starts the actor. Nothing is accepted before
    /// the reconciliation is done.
    ///
    /// # Errors
    ///
    /// A failure to read the rows or the disk, by message.
    pub(crate) async fn spawn(
        client: Client,
        disk: Arc<Disk>,
        rows: Arc<dyn Rows>,
        config: Config,
    ) -> Result<Self, String> {
        reconcile(&disk, rows.as_ref()).await?;
        let (commands, receiver) = mpsc::unbounded_channel();
        let status = Arc::new(Mutex::new(Status {
            id: None,
            spec: String::new(),
            phase: Phase::Idle,
        }));
        let actor = Actor {
            client,
            disk: Arc::clone(&disk),
            rows: Arc::clone(&rows),
            config,
            status: Arc::clone(&status),
            receiver,
            stop: Arc::new(AtomicBool::new(false)),
            pending: None,
        };
        tokio::spawn(actor.run());
        Ok(Self {
            commands,
            status,
            disk,
            rows,
        })
    }

    pub(crate) fn status(&self) -> Status {
        lock(&self.status).clone()
    }

    /// Whether a transfer is running or verifying.
    #[cfg(test)]
    pub(crate) fn busy(&self) -> bool {
        matches!(
            lock(&self.status).phase,
            Phase::Running { .. } | Phase::Verifying
        )
    }

    /// Starts the download of a freshly confirmed row; `permit` is dropped when it ends.
    pub(crate) fn start(&self, row: Row, permit: Permit) {
        {
            let mut status = lock(&self.status);
            status.id = Some(row.id);
            status.spec.clone_from(&row.spec);
            status.phase = Phase::Running {
                received: 0,
                total: row.size,
            };
        }
        let _ = self.commands.send(Command::Start(row, permit));
    }

    /// Resumes the stopped download `id`.
    pub(crate) fn resume(&self, row: &Row, permit: Permit) {
        {
            let mut status = lock(&self.status);
            status.id = Some(row.id);
            status.spec.clone_from(&row.spec);
            status.phase = Phase::Running {
                received: 0,
                total: row.size,
            };
        }
        let _ = self.commands.send(Command::Resume(row.id, permit));
    }

    /// Stops the running download, keeping its partial; accepted at once.
    pub(crate) fn cancel(&self) {
        {
            let mut status = lock(&self.status);
            if matches!(status.phase, Phase::Running { .. } | Phase::Verifying) {
                status.phase = Phase::Stopped(Stop::Cancelled);
            }
        }
        let _ = self.commands.send(Command::Cancel);
    }

    /// Stops download `id` if it runs, deletes its partial and its row.
    ///
    /// # Errors
    ///
    /// A failure to remove the partial or the row, by message.
    pub(crate) async fn discard(&self, id: i64) -> Result<(), String> {
        let (reply, answer) = oneshot::channel();
        self.commands
            .send(Command::Discard(id, reply))
            .map_err(|_| "the download task has ended".to_owned())?;
        answer
            .await
            .map_err(|_| "the download task has ended".to_owned())?
    }

    /// Stops the task, keeping the partial; resolves once it has ended.
    pub(crate) async fn shutdown(&self) {
        let (reply, done) = oneshot::channel();
        if self.commands.send(Command::Shutdown(reply)).is_ok() {
            let _ = done.await;
        }
    }

    /// Names of the partials on disk.
    ///
    /// # Errors
    ///
    /// A failure to list the folder, by message.
    pub(crate) async fn partials(&self) -> Result<Vec<String>, String> {
        let disk = Arc::clone(&self.disk);
        tokio::task::spawn_blocking(move || disk.partials())
            .await
            .map_err(|_| "the partial folder cannot be listed".to_owned())?
            .map_err(|error| error.to_string())
    }

    /// Removes the completed download `id` and its file, unless something refers to the file.
    /// `protect` names files that must stay (the pinned defaults and the effective model).
    ///
    /// # Errors
    ///
    /// Why the file is kept, or a failure to remove it, by message.
    pub(crate) async fn delete(&self, id: i64, protect: &[String]) -> Result<(), String> {
        let rows = self.rows.list().await?;
        let row = rows
            .iter()
            .find(|row| row.id == id)
            .ok_or_else(|| "there is no such download".to_owned())?;
        if row.state != State::Complete {
            return Err("only a finished download can be deleted".to_owned());
        }
        let shared = rows
            .iter()
            .any(|other| other.id != id && other.file_name == row.file_name);
        if !shared {
            if protect.contains(&row.file_name) {
                return Err("this file is in use or is a pinned default".to_owned());
            }
            if self.rows.in_use(&row.file_name).await? {
                return Err("the model setting uses this file".to_owned());
            }
            let (disk, name) = (Arc::clone(&self.disk), row.file_name.clone());
            tokio::task::spawn_blocking(move || disk.remove_final(&name))
                .await
                .map_err(|_| "the file cannot be removed".to_owned())?
                .map_err(|error| error.to_string())?;
        }
        self.rows.delete(id).await
    }
}

/// The outcome of one transfer.
enum Outcome {
    Done,
    Stopped(Stop),
    Discard(i64, oneshot::Sender<Result<(), String>>),
    Shutdown(oneshot::Sender<()>),
}

/// A command that interrupted blocking work.
enum Interrupt {
    Cancel,
    Discard(i64, oneshot::Sender<Result<(), String>>),
    Shutdown(oneshot::Sender<()>),
}

impl From<Interrupt> for Outcome {
    fn from(interrupt: Interrupt) -> Self {
        match interrupt {
            Interrupt::Cancel => Self::Stopped(Stop::Cancelled),
            Interrupt::Discard(id, reply) => Self::Discard(id, reply),
            Interrupt::Shutdown(reply) => Self::Shutdown(reply),
        }
    }
}

fn failed(why: impl Into<String>) -> Outcome {
    Outcome::Stopped(Stop::Failed(why.into()))
}

fn disk_failed(error: &DiskError) -> Outcome {
    match error {
        DiskError::Cancelled => Outcome::Stopped(Stop::Cancelled),
        other => failed(other.to_string()),
    }
}

struct Actor {
    client: Client,
    disk: Arc<Disk>,
    rows: Arc<dyn Rows>,
    config: Config,
    status: Arc<Mutex<Status>>,
    receiver: mpsc::UnboundedReceiver<Command>,
    /// Asks a running hash to stop.
    stop: Arc<AtomicBool>,
    /// Blocking work abandoned by an interrupt; awaited before the disk is touched again.
    pending: Option<tokio::task::JoinHandle<()>>,
}

impl Actor {
    fn set(&self, id: i64, spec: &str, phase: Phase) {
        let mut status = lock(&self.status);
        status.id = Some(id);
        spec.clone_into(&mut status.spec);
        status.phase = phase;
    }

    fn phase(&self, phase: Phase) {
        lock(&self.status).phase = phase;
    }

    async fn run(mut self) {
        while let Some(command) = self.receiver.recv().await {
            match command {
                Command::Start(row, permit) => {
                    let outcome = self.transfer(&row).await;
                    drop(permit);
                    if self.settle(&row, outcome).await {
                        return;
                    }
                }
                Command::Resume(id, permit) => {
                    let found = self
                        .rows
                        .list()
                        .await
                        .ok()
                        .and_then(|rows| rows.into_iter().find(|row| row.id == id));
                    if let Some(row) = found.filter(|row| row.state == State::Confirmed) {
                        self.set(
                            row.id,
                            &row.spec,
                            Phase::Running {
                                received: 0,
                                total: row.size,
                            },
                        );
                        let outcome = self.transfer(&row).await;
                        drop(permit);
                        if self.settle(&row, outcome).await {
                            return;
                        }
                    }
                }
                Command::Cancel => {}
                Command::Discard(id, reply) => {
                    let _ = reply.send(self.discard(id).await);
                }
                Command::Shutdown(reply) => {
                    let _ = reply.send(());
                    return;
                }
            }
        }
    }

    /// Records an outcome; true when the task must end.
    async fn settle(&mut self, row: &Row, outcome: Outcome) -> bool {
        match outcome {
            Outcome::Done => self.set(row.id, &row.spec, Phase::Done),
            Outcome::Stopped(stop) => self.set(row.id, &row.spec, Phase::Stopped(stop)),
            Outcome::Discard(id, reply) => {
                let _ = reply.send(self.discard(id).await);
            }
            Outcome::Shutdown(reply) => {
                let _ = reply.send(());
                return true;
            }
        }
        false
    }

    async fn settle_pending(&mut self) {
        if let Some(handle) = self.pending.take() {
            let _ = handle.await;
        }
    }

    async fn discard(&mut self, id: i64) -> Result<(), String> {
        self.settle_pending().await;
        let rows = self.rows.list().await?;
        let Some(row) = rows.iter().find(|row| row.id == id) else {
            return Ok(());
        };
        if row.state == State::Complete {
            return Err("a finished download is deleted, not discarded".to_owned());
        }
        {
            let (disk, name) = (Arc::clone(&self.disk), row.sha256.clone());
            tokio::task::spawn_blocking(move || disk.remove_partial(&name))
                .await
                .map_err(|_| "the partial cannot be removed".to_owned())?
                .map_err(|error| error.to_string())?;
        }
        self.rows.delete(id).await?;
        let mut status = lock(&self.status);
        if status.id == Some(id) {
            *status = Status {
                id: None,
                spec: String::new(),
                phase: Phase::Idle,
            };
        }
        Ok(())
    }

    /// Runs `work` off the executor, staying responsive to commands. A command that arrives
    /// first interrupts: the work is left to finish on its own (awaited before the disk is
    /// touched again) and a running hash is asked to stop.
    async fn io<T: Send + 'static>(
        &mut self,
        work: impl FnOnce(&Disk) -> Result<T, DiskError> + Send + 'static,
    ) -> Result<Result<T, DiskError>, Interrupt> {
        let disk = Arc::clone(&self.disk);
        let mut handle = tokio::task::spawn_blocking(move || work(&disk));
        loop {
            tokio::select! {
                done = &mut handle => {
                    return Ok(done.unwrap_or(Err(DiskError::Io("the disk work panicked"))));
                }
                command = self.receiver.recv() => {
                    let interrupt = match command {
                        Some(Command::Cancel) | None => Interrupt::Cancel,
                        Some(Command::Discard(id, reply)) => Interrupt::Discard(id, reply),
                        Some(Command::Shutdown(reply)) => Interrupt::Shutdown(reply),
                        // A start or resume while one runs is dropped (its permit with it).
                        Some(Command::Start(..) | Command::Resume(..)) => continue,
                    };
                    self.stop.store(true, Ordering::SeqCst);
                    self.pending = Some(tokio::spawn(async move {
                        let _ = handle.await;
                    }));
                    return Err(interrupt);
                }
            }
        }
    }

    async fn transfer(&mut self, row: &Row) -> Outcome {
        self.settle_pending().await;
        self.stop.store(false, Ordering::SeqCst);
        let Ok(Spec::Hf(spec)) = parse_spec(&row.spec) else {
            return failed("the stored download is not an hf: spec");
        };
        match self.adopt(row).await {
            Ok(Some(outcome)) | Err(outcome) => return outcome,
            Ok(None) => {}
        }
        let (mut file, mut offset) = match self.prepare(row).await {
            Ok(prepared) => prepared,
            Err(outcome) => return outcome,
        };
        if offset < row.size {
            let url = self.client.resolve_url(&spec, &spec.path);
            let mut restarted = false;
            let mut body = loop {
                self.set(
                    row.id,
                    &row.spec,
                    Phase::Running {
                        received: offset,
                        total: row.size,
                    },
                );
                let opened = match self.client.open(url.clone(), offset).await {
                    Ok(body) => body,
                    Err(error) => return failed(error.to_string()),
                };
                match check_answer(&opened, offset, row.size) {
                    Answer::Good => break opened,
                    Answer::Bad(why) => return failed(why),
                    Answer::Restart if !restarted => {
                        restarted = true;
                        drop(opened);
                        match self.restart(row).await {
                            Ok(fresh) => {
                                file = fresh;
                                offset = 0;
                            }
                            Err(outcome) => return outcome,
                        }
                    }
                    Answer::Restart => {
                        return failed("the server does not honour a resumed download");
                    }
                }
            };
            match self.receive(row, &file, &mut body, offset).await {
                Ok(()) => {}
                Err(outcome) => return outcome,
            }
        }
        self.finish(row, file).await
    }

    /// Adopts a final file that already has the name and the right hash (a pinned default or
    /// a file shared by hash); refuses one that differs. `None` when there is no such file.
    async fn adopt(&mut self, row: &Row) -> Result<Option<Outcome>, Outcome> {
        let name = row.file_name.clone();
        let state = match self.io(move |disk| disk.final_state(&name)).await {
            Ok(Ok(state)) => state,
            Ok(Err(error)) => return Err(disk_failed(&error)),
            Err(interrupt) => return Err(interrupt.into()),
        };
        let Some(state) = state else {
            return Ok(None);
        };
        if state.len != row.size {
            return Err(failed(
                "a different file already has this name in the models folder: remove it by hand",
            ));
        }
        self.phase(Phase::Verifying);
        let (name, stop) = (row.file_name.clone(), Arc::clone(&self.stop));
        let hashed = self
            .io(move |disk| {
                let file = disk.open_final(&name)?;
                disk.hash(&file, &name, &|| stop.load(Ordering::SeqCst))
            })
            .await;
        match hashed {
            Ok(Ok(sha)) if sha == row.sha256 => {}
            Ok(Ok(_)) => {
                return Err(failed(
                    "a different file already has this name in the models folder: remove it by hand",
                ));
            }
            Ok(Err(error)) => return Err(disk_failed(&error)),
            Err(interrupt) => return Err(interrupt.into()),
        }
        let sha = row.sha256.clone();
        match self.io(move |disk| disk.remove_partial(&sha)).await {
            Ok(Ok(())) => {}
            Ok(Err(error)) => return Err(disk_failed(&error)),
            Err(interrupt) => return Err(interrupt.into()),
        }
        Ok(Some(match self.rows.complete(row.id).await {
            Ok(()) => Outcome::Done,
            Err(why) => failed(why),
        }))
    }

    /// Opens or creates the partial and checks the free space; returns it with its length.
    async fn prepare(&mut self, row: &Row) -> Result<(Arc<File>, u64), Outcome> {
        let (sha, size, chunk) = (row.sha256.clone(), row.size, self.config.chunk);
        let opened = self
            .io(move |disk| {
                let state = disk.partial_state(&sha)?;
                let (file, len) = match state {
                    Some(partial) if partial.len > size => {
                        disk.remove_partial(&sha)?;
                        (disk.create_partial(&sha)?, 0)
                    }
                    Some(_) => {
                        let (file, len) = disk.open_partial(&sha)?;
                        // A fully received partial goes to verification as it is; any other
                        // is cut back so a power-loss tail is not trusted.
                        let keep = if len == size {
                            len
                        } else {
                            (len / chunk * chunk).saturating_sub(chunk)
                        };
                        if keep != len {
                            Disk::truncate(&file, keep)?;
                        }
                        (file, keep)
                    }
                    None => (disk.create_partial(&sha)?, 0),
                };
                Ok((Arc::new(file), len))
            })
            .await;
        let (file, len) = match opened {
            Ok(Ok(opened)) => opened,
            Ok(Err(error)) => return Err(disk_failed(&error)),
            Err(interrupt) => return Err(interrupt.into()),
        };
        let needed = (row.size - len).saturating_add(self.config.reserve);
        if self.disk.free_space() < needed {
            return Err(failed(format!(
                "not enough free disk space: {} MiB needed in the models folder",
                needed.div_ceil(1 << 20)
            )));
        }
        Ok((file, len))
    }

    /// Deletes the partial and starts a new one from zero.
    async fn restart(&mut self, row: &Row) -> Result<Arc<File>, Outcome> {
        let sha = row.sha256.clone();
        match self
            .io(move |disk| {
                disk.remove_partial(&sha)?;
                Ok(Arc::new(disk.create_partial(&sha)?))
            })
            .await
        {
            Ok(Ok(file)) => Ok(file),
            Ok(Err(error)) => Err(disk_failed(&error)),
            Err(interrupt) => Err(interrupt.into()),
        }
    }

    async fn flush(&mut self, file: &Arc<File>, at: u64, buffer: Vec<u8>) -> Result<(), Outcome> {
        let file = Arc::clone(file);
        match self.io(move |disk| disk.write_at(&file, at, &buffer)).await {
            Ok(Ok(())) => Ok(()),
            Ok(Err(error)) => Err(disk_failed(&error)),
            Err(interrupt) => Err(interrupt.into()),
        }
    }

    /// Reads the body to its end, writing in pieces, with the progress floor.
    async fn receive(
        &mut self,
        row: &Row,
        file: &Arc<File>,
        body: &mut Body,
        start: u64,
    ) -> Result<(), Outcome> {
        let mut written = start;
        let mut buffer: Vec<u8> = Vec::new();
        let mut window_end = Instant::now() + self.config.floor_window;
        let mut window_bytes = 0_u64;
        loop {
            tokio::select! {
                command = self.receiver.recv() => {
                    match command {
                        Some(Command::Cancel) | None => return Err(Outcome::Stopped(Stop::Cancelled)),
                        Some(Command::Discard(id, reply)) => return Err(Outcome::Discard(id, reply)),
                        Some(Command::Shutdown(reply)) => return Err(Outcome::Shutdown(reply)),
                        Some(Command::Start(..) | Command::Resume(..)) => {}
                    }
                }
                () = tokio::time::sleep_until(window_end) => {
                    if window_bytes < self.config.floor_bytes {
                        return Err(Outcome::Stopped(Stop::Stalled));
                    }
                    window_bytes = 0;
                    window_end = Instant::now() + self.config.floor_window;
                }
                chunk = body.chunk() => {
                    let chunk = match chunk {
                        Ok(chunk) => chunk,
                        Err(error) => return Err(failed(error.to_string())),
                    };
                    let Some(chunk) = chunk else { break };
                    let have = written + buffer.len() as u64 + chunk.len() as u64;
                    if have > row.size {
                        let sha = row.sha256.clone();
                        let _ = self.io(move |disk| disk.remove_partial(&sha)).await;
                        return Err(failed("the server sent more bytes than the file's size"));
                    }
                    window_bytes += chunk.len() as u64;
                    buffer.extend_from_slice(&chunk);
                    self.phase(Phase::Running { received: written + buffer.len() as u64, total: row.size });
                    if buffer.len() >= self.config.flush {
                        let piece = std::mem::take(&mut buffer);
                        let length = piece.len() as u64;
                        self.flush(file, written, piece).await?;
                        written += length;
                    }
                }
            }
        }
        if !buffer.is_empty() {
            let length = buffer.len() as u64;
            self.flush(file, written, buffer).await?;
            written += length;
        }
        if written != row.size {
            return Err(failed(
                "the server ended the download early: resume it to fetch the rest",
            ));
        }
        Ok(())
    }

    /// Syncs, verifies, publishes and records the finished partial.
    async fn finish(&mut self, row: &Row, file: Arc<File>) -> Outcome {
        self.phase(Phase::Verifying);
        let (sha, stop) = (row.sha256.clone(), Arc::clone(&self.stop));
        let verified = self
            .io(move |disk| {
                disk.fault(Fault::AfterWrite)?;
                disk.sync_file(&file)?;
                disk.hash(&file, &sha, &|| stop.load(Ordering::SeqCst))
            })
            .await;
        match verified {
            Ok(Ok(sha)) if sha == row.sha256 => {}
            Ok(Ok(_)) => {
                let sha = row.sha256.clone();
                let _ = self.io(move |disk| disk.remove_partial(&sha)).await;
                return failed(
                    "the downloaded file's SHA-256 differs from the one confirmed: it was deleted",
                );
            }
            Ok(Err(error)) => return disk_failed(&error),
            Err(interrupt) => return interrupt.into(),
        }
        let (sha, name) = (row.sha256.clone(), row.file_name.clone());
        let published = self
            .io(move |disk| {
                disk.fault(Fault::AfterVerify)?;
                disk.publish(&sha, &name)
            })
            .await;
        match published {
            Ok(Ok(())) => {}
            Ok(Err(DiskError::Exists)) => {
                return failed(
                    "a different file already has this name in the models folder: remove it by hand",
                );
            }
            Ok(Err(error)) => return disk_failed(&error),
            Err(interrupt) => return interrupt.into(),
        }
        if let Ok(Err(error)) = self.io(|disk| disk.fault(Fault::BeforeCommit)).await {
            return disk_failed(&error);
        }
        match self.rows.complete(row.id).await {
            Ok(()) => Outcome::Done,
            Err(why) => failed(format!(
                "the file is in place but the record failed ({why}); restarting Mimik completes it"
            )),
        }
    }
}

enum Answer {
    Good,
    Bad(String),
    Restart,
}

/// Checks the status and `Content-Range` of an answer against the offset asked for.
fn check_answer(body: &Body, offset: u64, size: u64) -> Answer {
    if offset == 0 {
        if body.status != 200 {
            return Answer::Bad("the server did not send the whole file".to_owned());
        }
        if body.content_length.is_some_and(|length| length != size) {
            return Answer::Bad("the file's size differs from the one confirmed".to_owned());
        }
        return Answer::Good;
    }
    let expected = format!("bytes {offset}-{}/{size}", size - 1);
    if body.status == 206 && body.content_range.as_deref() == Some(expected.as_str()) {
        Answer::Good
    } else {
        Answer::Restart
    }
}

/// Reconciles the disk with the rows at start: a publication that was cut short is completed
/// or cleaned up, stale partials are swept. Runs before any command is accepted.
///
/// # Errors
///
/// A failure to read the rows or the disk, or to record a completed file, by message.
pub(crate) async fn reconcile(disk: &Arc<Disk>, rows: &dyn Rows) -> Result<(), String> {
    let listed = rows.list().await?;
    let (disk, snapshot) = (Arc::clone(disk), listed);
    let verified = tokio::task::spawn_blocking(move || sweep(&disk, &snapshot))
        .await
        .map_err(|_| "reconciliation failed".to_owned())?
        .map_err(|error| error.to_string())?;
    for id in verified {
        rows.complete(id).await?;
    }
    Ok(())
}

/// The blocking part of [`reconcile`]: returns the confirmed rows whose final file is in place
/// and verified (to be recorded as complete).
fn sweep(disk: &Disk, rows: &[Row]) -> Result<Vec<i64>, DiskError> {
    let mut complete = Vec::new();
    for row in rows.iter().filter(|row| row.state == State::Confirmed) {
        let published = disk.final_state(&row.file_name).ok().flatten();
        let mut partial = disk.partial_state(&row.sha256).ok().flatten();
        // A `link` publication cut between the link and the unlink: the final name and the
        // partial name share an inode with two links. Remove the partial name first.
        if let (Some(done), Some(left)) = (published, partial)
            && done.inode == left.inode
            && left.links == 2
        {
            disk.remove_partial(&row.sha256)?;
            partial = None;
        }
        if published.is_some_and(|done| done.len == row.size) {
            let file = disk.open_final(&row.file_name)?;
            let sha = disk.hash(&file, &row.file_name, &|| false)?;
            if sha == row.sha256 {
                disk.remove_partial(&row.sha256)?;
                complete.push(row.id);
                continue;
            }
        }
        if partial.is_some_and(|left| left.len > row.size) {
            disk.remove_partial(&row.sha256)?;
        }
    }
    // Partials no confirmed download owns (a discarded download, a leftover) go.
    for name in disk.partials()? {
        let owned = rows
            .iter()
            .any(|row| row.state == State::Confirmed && row.sha256 == name)
            && !complete
                .iter()
                .any(|id| rows.iter().any(|row| row.id == *id && row.sha256 == name));
        if !owned {
            disk.remove_partial(&name)?;
        }
    }
    Ok(complete)
}

#[cfg(test)]
#[path = "task_tests.rs"]
mod tests;
