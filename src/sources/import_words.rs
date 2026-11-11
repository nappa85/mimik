//! Word boxes at import: the verified PDF goes through a private file in the data directory's
//! staging area (never the system temporary directory, which may be RAM) to the confined
//! worker, in batches of at most eight pages. This runs before the write transaction; the
//! worker never runs inside it. Every row is validated and encoded here, so a worker that
//! misbehaves cannot store anything the reader would refuse.
use std::{collections::BTreeMap, io::Read, io::Write};

use sha2::{Digest, Sha256};

use super::{
    import::{Fetched, WantedPage, WordSource},
    page_words::{self, PageWord},
    staging::Files,
    worker::{self, Cancel, Reason},
};

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
    files: Files,
}
impl WorkerWords {
    pub(crate) fn new(files: Files) -> Self {
        Self { files }
    }
}

/// Removes the spool file however the run ends.
struct Spool<'a> {
    files: &'a Files,
    name: String,
}
impl Drop for Spool<'_> {
    fn drop(&mut self) {
        let _ = self.files.remove_spool(&self.name);
    }
}

fn sha256_of(path: &std::path::Path) -> std::io::Result<String> {
    let mut file = std::fs::File::open(path)?;
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
        let Ok((name, mut file)) = self.files.spool() else {
            return unavailable("the staging file could not be created");
        };
        let _spool = Spool {
            files: &self.files,
            name: name.clone(),
        };
        let Ok(path) = self.files.spool_path(&name) else {
            return unavailable("the staging file could not be created");
        };
        if file.write_all(pdf).and_then(|()| file.sync_all()).is_err() {
            return unavailable("the staging file could not be written");
        }
        drop(file);
        // The worker reads the file, not the verified bytes: check what is on disk.
        if sha256_of(&path).is_ok_and(|hash| hash == pdf_sha256) {
            run(&path, pages)
        } else {
            unavailable("the staging file did not match the verified PDF")
        }
    }
}

fn convert(words: &[worker::Word]) -> Vec<PageWord> {
    words
        .iter()
        .map(|word| PageWord {
            text: word.text.clone(),
            // Boxes are validated inside the page (at most 4,000 points) before this point.
            #[allow(clippy::cast_possible_truncation)]
            bounds: word.bounds.map(|v| v as f32),
        })
        .collect()
}

fn run(path: &std::path::Path, wanted: &[WantedPage]) -> Fetched {
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
    let usable: Vec<(WantedPage, worker::Page)> = wanted
        .iter()
        .filter_map(|want| {
            let page = inspection
                .pages
                .get(usize::try_from(want.pdf_page).ok()?.checked_sub(1)?)?;
            ((page.width - want.width).abs() <= SIZE_TOLERANCE
                && (page.height - want.height).abs() <= SIZE_TOLERANCE)
                .then(|| (*want, page.clone()))
        })
        .collect();
    for batch in usable.chunks(BATCH) {
        if !batch_words(path, batch, &mut out) {
            return out;
        }
    }
    out
}

/// Run one batch; on a document error retry its pages one by one, so one hostile page costs only
/// itself. Returns false when the worker is unavailable (the rest is not attempted).
fn batch_words(
    path: &std::path::Path,
    batch: &[(WantedPage, worker::Page)],
    out: &mut Fetched,
) -> bool {
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

    fn staging_entries(dir: &std::path::Path) -> usize {
        std::fs::read_dir(dir.join("staging")).unwrap().count()
    }

    #[test]
    fn the_pdf_goes_through_the_data_directorys_staging_and_is_removed() {
        let dir = tempfile::tempdir().unwrap();
        let source = WorkerWords::new(Files::open(dir.path()).unwrap());
        let pdf = b"%PDF-1.7\nnot a document\n%%EOF\n";
        let hash = hex::encode(Sha256::digest(pdf));
        // Whatever the worker makes of it (here: unavailable or unreadable), nothing is stored
        // and the spool file is gone.
        let fetched = source.fetch(pdf, &hash, &pages());
        assert!(fetched.rows.is_empty());
        assert_eq!(staging_entries(dir.path()), 0);
    }

    #[test]
    fn a_file_that_does_not_match_the_verified_hash_is_never_given_to_the_worker() {
        let dir = tempfile::tempdir().unwrap();
        let source = WorkerWords::new(Files::open(dir.path()).unwrap());
        let fetched = source.fetch(b"%PDF-1.7\n", &"0".repeat(64), &pages());
        assert!(fetched.rows.is_empty());
        assert_eq!(
            fetched.unavailable.as_deref(),
            Some("the staging file did not match the verified PDF")
        );
        assert_eq!(staging_entries(dir.path()), 0);
    }

    #[test]
    fn the_spool_file_is_private() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let files = Files::open(dir.path()).unwrap();
        let (name, _file) = files.spool().unwrap();
        let path = files.spool_path(&name).unwrap();
        assert!(path.starts_with(dir.path().join("staging")));
        assert_eq!(
            std::fs::metadata(path).unwrap().permissions().mode() & 0o777,
            0o600
        );
        files.remove_spool(&name).unwrap();
    }
}
