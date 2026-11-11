//! Word boxes at import: the verified PDF goes through a private file in the data directory's
//! own `import-spool` folder (never the system temporary directory, which may be RAM, and never
//! the upload staging area, whose sweep and reconcile delete files they do not know) to the
//! confined worker, in batches of at most eight pages. This runs before the write transaction;
//! the worker never runs inside it. Every row is validated and encoded here, so a worker that
//! misbehaves cannot store anything the reader would refuse.
//!
//! The file is created `O_EXCL|O_NOFOLLOW` with mode 0600 in a 0700 folder, written once and
//! hashed through the same descriptor. The worker then opens it again by path (its launch API
//! takes a path and pins the file's identity between its own open and the sandbox bind), so
//! only another process of the same user could swap the file in between.
use std::{
    collections::BTreeMap,
    io::{Read, Seek, SeekFrom, Write},
    path::{Path, PathBuf},
    time::{Duration, SystemTime},
};

use rustix::fs::{Mode, OFlags};
use sha2::{Digest, Sha256};

use super::{
    import::{Fetched, WantedPage, WordSource},
    page_words::{self, PageWord},
    worker::{self, Cancel, Reason},
};

/// The folder of the data directory holding import spool files.
pub(crate) const SPOOL_DIR: &str = "import-spool";
const SPOOL_PREFIX: &str = "import-";
const SPOOL_SUFFIX: &str = ".part";
/// A spool file this old is left over from an import that died; younger ones may be running.
const STALE: Duration = Duration::from_secs(3600);
/// Pages per worker run (the render batch size).
const BATCH: usize = 8;
/// The bundle's page size and the worker's may differ by this many points (as in the import's
/// own PDF check); a page that differs more is left without words.
const SIZE_TOLERANCE: f64 = 2.0;

/// A source that cannot run, with the reason the report shows.
pub(crate) struct Unavailable(pub &'static str);
impl WordSource for Unavailable {
    fn fetch(&self, _: &[u8], _: &str, _: &[WantedPage]) -> Fetched {
        Fetched {
            unavailable: Some(self.0.to_owned()),
            ..Fetched::default()
        }
    }
}

pub(crate) struct WorkerWords {
    dir: rustix::fd::OwnedFd,
    path: PathBuf,
}
impl WorkerWords {
    /// Opens (creating it 0700) the spool folder of `data` and removes stale spool files.
    ///
    /// # Errors
    /// Refuses a symbolic link or a folder that cannot be created or opened.
    pub(crate) fn open(data: &Path) -> std::io::Result<Self> {
        let flags = OFlags::RDONLY | OFlags::DIRECTORY | OFlags::NOFOLLOW | OFlags::CLOEXEC;
        let parent = rustix::fs::open(data, flags, Mode::empty())?;
        match rustix::fs::mkdirat(&parent, SPOOL_DIR, Mode::RWXU) {
            Ok(()) | Err(rustix::io::Errno::EXIST) => {}
            Err(error) => return Err(error.into()),
        }
        let dir = rustix::fs::openat(&parent, SPOOL_DIR, flags, Mode::empty())?;
        let stat = rustix::fs::fstat(&dir)?;
        // A folder the owner made group- or world-accessible is not private.
        if stat.st_mode & 0o077 != 0 {
            rustix::fs::fchmod(&dir, Mode::RWXU)?;
        }
        let this = Self {
            dir,
            path: data.join(SPOOL_DIR),
        };
        this.remove_stale();
        Ok(this)
    }

    fn remove_stale(&self) {
        let Ok(mut entries) = rustix::fs::Dir::read_from(&self.dir) else {
            return;
        };
        while let Some(Ok(entry)) = entries.next() {
            let name = entry.file_name().to_bytes();
            let ours = name.starts_with(SPOOL_PREFIX.as_bytes())
                && name.ends_with(SPOOL_SUFFIX.as_bytes());
            if !ours {
                continue;
            }
            let Ok(stat) = rustix::fs::statat(
                &self.dir,
                entry.file_name(),
                rustix::fs::AtFlags::SYMLINK_NOFOLLOW,
            ) else {
                continue;
            };
            let age = SystemTime::now()
                .duration_since(
                    SystemTime::UNIX_EPOCH
                        + Duration::from_secs(u64::try_from(stat.st_mtime).unwrap_or(0)),
                )
                .unwrap_or_default();
            if age >= STALE {
                let _ = rustix::fs::unlinkat(
                    &self.dir,
                    entry.file_name(),
                    rustix::fs::AtFlags::empty(),
                );
            }
        }
    }

    fn create(&self) -> std::io::Result<(String, std::fs::File)> {
        let mut random = [0; 16];
        getrandom::fill(&mut random).map_err(std::io::Error::other)?;
        let name = format!("{SPOOL_PREFIX}{}{SPOOL_SUFFIX}", hex::encode(random));
        let fd = rustix::fs::openat(
            &self.dir,
            &name,
            OFlags::RDWR | OFlags::CREATE | OFlags::EXCL | OFlags::NOFOLLOW | OFlags::CLOEXEC,
            Mode::RUSR | Mode::WUSR,
        )?;
        Ok((name, std::fs::File::from(fd)))
    }
}

/// Removes the spool file however the run ends.
struct Spool<'a> {
    dir: &'a rustix::fd::OwnedFd,
    name: String,
}
impl Drop for Spool<'_> {
    fn drop(&mut self) {
        let _ = rustix::fs::unlinkat(self.dir, &self.name, rustix::fs::AtFlags::empty());
    }
}

fn sha256_of(file: &mut std::fs::File) -> std::io::Result<String> {
    file.seek(SeekFrom::Start(0))?;
    let mut digest = Sha256::new();
    let mut buffer = vec![0; 1 << 16];
    loop {
        let read = file.read(&mut buffer)?;
        if read == 0 {
            return Ok(hex::encode(digest.finalize()));
        }
        digest.update(&buffer[..read]);
    }
}

impl WordSource for WorkerWords {
    fn fetch(&self, pdf: &[u8], pdf_sha256: &str, pages: &[WantedPage]) -> Fetched {
        let unavailable = |reason: &str| Fetched {
            unavailable: Some(reason.to_owned()),
            ..Fetched::default()
        };
        let Ok((name, mut file)) = self.create() else {
            return unavailable("the import spool file could not be created");
        };
        let _spool = Spool {
            dir: &self.dir,
            name: name.clone(),
        };
        if file.write_all(pdf).and_then(|()| file.sync_all()).is_err() {
            return unavailable("the import spool file could not be written");
        }
        // The worker reads the file, not the verified bytes: hash what the descriptor holds.
        if sha256_of(&mut file).is_ok_and(|hash| hash == pdf_sha256) {
            run(&self.path.join(&name), pages)
        } else {
            unavailable("the import spool file did not match the verified PDF")
        }
    }
}

/// The worker's words as stored words; one whose box collapsed in the narrowing to `f32` is
/// dropped (it has no area to place), without failing its page.
fn convert(words: &[worker::Word]) -> Vec<PageWord> {
    words
        .iter()
        .map(|word| PageWord {
            text: word.text.clone(),
            // Boxes are validated inside the page (at most 4,000 points) before this point.
            #[allow(clippy::cast_possible_truncation)]
            bounds: word.bounds.map(|v| v as f32),
        })
        .filter(|word| word.bounds[0] < word.bounds[2] && word.bounds[1] < word.bounds[3])
        .collect()
}

fn run(path: &Path, wanted: &[WantedPage]) -> Fetched {
    let mut out = Fetched::default();
    let inspection = match worker::inspect(path) {
        Ok(inspection) => inspection,
        Err(Reason::Unavailable(reason)) => {
            out.unavailable = Some(reason);
            return out;
        }
        Err(_) => {
            out.unavailable = Some("the worker could not read the PDF".into());
            return out;
        }
    };
    // Pages the worker sees at another size than the bundle are skipped, not guessed at.
    let mut usable: Vec<(WantedPage, worker::Page)> = Vec::new();
    for want in wanted {
        let page = usize::try_from(want.pdf_page)
            .ok()
            .and_then(|n| n.checked_sub(1))
            .and_then(|n| inspection.pages.get(n));
        match page {
            Some(page)
                if (page.width - want.width).abs() <= SIZE_TOLERANCE
                    && (page.height - want.height).abs() <= SIZE_TOLERANCE =>
            {
                usable.push((*want, page.clone()));
            }
            _ => out.size_mismatch.push(want.pdf_page),
        }
    }
    for batch in usable.chunks(BATCH) {
        if !batch_words(path, batch, &mut out) {
            return out;
        }
    }
    out
}

/// Run one batch; on a document error retry its pages one by one, so one hostile page costs only
/// itself. Returns false when the worker is unavailable (the rest is not attempted).
fn batch_words(path: &Path, batch: &[(WantedPage, worker::Page)], out: &mut Fetched) -> bool {
    let mut collected = BTreeMap::new();
    let result = worker::words(
        path,
        batch.iter().map(|(_, page)| page.clone()).collect(),
        &Cancel::default(),
        |page| {
            collected.insert(page.n, convert(&page.words));
            Ok(())
        },
    );
    match result {
        Ok(()) => keep(batch, collected, out),
        Err(Reason::Unavailable(reason)) => {
            out.unavailable = Some(reason);
            return false;
        }
        Err(_) if batch.len() > 1 => {
            for single in batch.chunks(1) {
                if !batch_words(path, single, out) {
                    return false;
                }
            }
        }
        Err(_) => {}
    }
    true
}

fn keep(
    batch: &[(WantedPage, worker::Page)],
    mut collected: BTreeMap<u32, Vec<PageWord>>,
    out: &mut Fetched,
) {
    for (want, page) in batch {
        if let Some(words) = collected.remove(&page.n)
            && let Ok(row) = page_words::encode(&words, page.width, page.height)
        {
            out.rows.insert(want.pdf_page, row);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pages() -> Vec<WantedPage> {
        vec![WantedPage {
            pdf_page: 1,
            width: 200.0,
            height: 300.0,
        }]
    }

    fn spool_entries(dir: &Path) -> usize {
        std::fs::read_dir(dir.join(SPOOL_DIR)).unwrap().count()
    }

    #[test]
    fn the_pdf_goes_through_the_import_spool_and_is_removed() {
        let dir = tempfile::tempdir().unwrap();
        let source = WorkerWords::open(dir.path()).unwrap();
        let pdf = b"%PDF-1.7\nnot a document\n%%EOF\n";
        let hash = hex::encode(Sha256::digest(pdf));
        // Whatever the worker makes of it (here: unavailable or unreadable), nothing is stored
        // and the spool file is gone.
        let fetched = source.fetch(pdf, &hash, &pages());
        assert_eq!(fetched.rows.len(), 0);
        assert_eq!(spool_entries(dir.path()), 0);
        assert!(
            !dir.path().join("staging").exists(),
            "never the upload staging area"
        );
    }

    #[test]
    fn a_file_that_does_not_match_the_verified_hash_is_never_given_to_the_worker() {
        let dir = tempfile::tempdir().unwrap();
        let source = WorkerWords::open(dir.path()).unwrap();
        let fetched = source.fetch(b"%PDF-1.7\n", &"0".repeat(64), &pages());
        assert_eq!(fetched.rows.len(), 0);
        assert_eq!(
            fetched.unavailable.as_deref(),
            Some("the import spool file did not match the verified PDF")
        );
        assert_eq!(spool_entries(dir.path()), 0);
    }

    #[test]
    fn the_spool_folder_and_file_are_private() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        // A folder that exists with loose permissions is tightened.
        std::fs::create_dir(dir.path().join(SPOOL_DIR)).unwrap();
        std::fs::set_permissions(
            dir.path().join(SPOOL_DIR),
            std::fs::Permissions::from_mode(0o755),
        )
        .unwrap();
        let source = WorkerWords::open(dir.path()).unwrap();
        let mode = |p: PathBuf| std::fs::metadata(p).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode(dir.path().join(SPOOL_DIR)), 0o700);
        let (name, _file) = source.create().unwrap();
        assert_eq!(mode(dir.path().join(SPOOL_DIR).join(&name)), 0o600);
        assert!(name.starts_with("import-"));
        assert_eq!(
            Path::new(&name).extension().and_then(|e| e.to_str()),
            Some("part")
        );
    }

    #[test]
    fn open_removes_only_stale_import_spools() {
        let dir = tempfile::tempdir().unwrap();
        let spool = dir.path().join(SPOOL_DIR);
        std::fs::create_dir(&spool).unwrap();
        let old = spool.join("import-old.part");
        let young = spool.join("import-young.part");
        let other = spool.join("keep.txt");
        for file in [&old, &young, &other] {
            std::fs::write(file, b"x").unwrap();
        }
        let long_ago = SystemTime::now() - Duration::from_hours(2);
        std::fs::File::options()
            .write(true)
            .open(&old)
            .unwrap()
            .set_modified(long_ago)
            .unwrap();
        WorkerWords::open(dir.path()).unwrap();
        assert!(!old.exists() && young.exists() && other.exists());
    }

    #[test]
    fn the_uploads_reconcile_leaves_the_import_spool_alone() {
        use crate::sources::staging::Files;
        let dir = tempfile::tempdir().unwrap();
        let source = WorkerWords::open(dir.path()).unwrap();
        let files = Files::open(dir.path()).unwrap();
        let (name, mut file) = source.create().unwrap();
        file.write_all(b"%PDF").unwrap();
        // The upload manager's sweep keeps nothing of its own here and deletes every unknown
        // entry of its staging folder: the import spool is not in that folder.
        files.reconcile_active(|_, _| false, None).unwrap();
        assert!(dir.path().join(SPOOL_DIR).join(&name).exists());
    }

    #[test]
    fn a_word_whose_box_collapses_when_narrowed_is_dropped_not_the_page() {
        let words = vec![
            worker::Word {
                text: "kept".into(),
                bounds: [1.0, 1.0, 5.0, 5.0],
            },
            // Wider than an f64 hair, narrower than an f32 step at 500 points.
            worker::Word {
                text: "gone".into(),
                bounds: [500.0, 1.0, 500.000_000_001, 5.0],
            },
        ];
        let kept: Vec<String> = convert(&words).into_iter().map(|w| w.text).collect();
        assert_eq!(kept, ["kept"]);
    }
}
