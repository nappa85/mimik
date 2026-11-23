//! Revisioned catalogue drafts (plan §3 step 1), mirroring `definitions::store`: one
//! nonqueued admission job per process (the shared definition permit), the bounded local
//! reader, and source consistency checks inside the write transaction.
use super::{
    CatalogueDraftSummary, CatalogueError, MAX_BYTES,
    admission::{self, Admitted},
    invalid,
};
use crate::{Store, registry::RulesetKey};
use rusqlite::{Connection, OptionalExtension, TransactionBehavior, params};
use std::{collections::BTreeSet, path::Path};
use tokio::sync::OwnedSemaphorePermit;

struct Job {
    data: Admitted,
    permit: OwnedSemaphorePermit,
}

pub(super) fn admit() -> Result<OwnedSemaphorePermit, CatalogueError> {
    crate::definitions::admit().map_err(|_| CatalogueError::Busy)
}

async fn prepare(
    input: impl FnOnce() -> Result<Vec<u8>, CatalogueError> + Send + 'static,
    expected_revision: i64,
) -> Result<Job, CatalogueError> {
    let permit = admit()?;
    if expected_revision < 0 {
        return Err(invalid(
            "/revision",
            "nonnegative-expected-revision-required",
        ));
    }
    tokio::task::spawn_blocking(move || {
        let bytes = input()?;
        Ok(Job {
            data: admission::validate(&bytes)?,
            permit,
        })
    })
    .await
    .map_err(|_| CatalogueError::Worker)?
}

/// Source consistency at draft save (plan D2): the registered pair, each listed source of
/// that exact pair with the stated PDF hash and finalized, and each referenced unit on its
/// page in its source. Rejected units stay allowed in drafts.
pub(super) fn references(db: &Connection, data: &Admitted) -> Result<(), CatalogueError> {
    let catalogue = &data.catalogue;
    let key = RulesetKey {
        game_id: catalogue.game_id.clone(),
        edition_id: catalogue.edition_id.clone(),
    };
    let registered: bool = db.query_row(
        "SELECT EXISTS(SELECT 1 FROM registry_editions WHERE game_id=?1 AND edition_id=?2)",
        params![key.game_id, key.edition_id],
        |row| row.get(0),
    )?;
    if !registered {
        return Err(invalid("/", "registered-ruleset-required"));
    }
    for (index, source) in catalogue.sources.iter().enumerate() {
        let path = format!("/sources/{index}");
        let finalized: Option<bool> = db
            .query_row(
                "SELECT EXISTS(SELECT 1 FROM source_finalizations f WHERE f.source_id=s.id) FROM sources s JOIN source_rulesets r ON r.source_id=s.id WHERE s.id=?1 AND s.pdf_hash=?2 AND r.game_id=?3 AND r.edition_id=?4",
                params![source.source_id, source.pdf_sha256, key.game_id, key.edition_id],
                |row| row.get(0),
            )
            .optional()?;
        match finalized {
            None => return Err(invalid(&path, "source-ruleset-or-pdf-hash-mismatch")),
            Some(false) => return Err(invalid(&path, "source-not-finalized")),
            Some(true) => {}
        }
    }
    let mut query = db.prepare_cached("SELECT count(*) FROM source_units u JOIN source_pages p ON p.id=u.page_id WHERE u.id=?1 AND p.id=?2 AND p.source_id=?3")?;
    let mut checked = BTreeSet::new();
    for (index, entry) in catalogue.entries.iter().enumerate() {
        for unit in entry.description.units.iter().flatten() {
            if !checked.insert(unit) {
                continue;
            }
            let matched: i64 = query
                .query_row(params![unit.unit_id, unit.page_id, unit.source_id], |row| {
                    row.get(0)
                })?;
            if matched != 1 {
                return Err(invalid(
                    &format!("/entries/{index}/description"),
                    "unit-page-source-mismatch",
                ));
            }
        }
    }
    Ok(())
}

fn summary(id: i64, revision: i64, data: &Admitted) -> CatalogueDraftSummary {
    let catalogue = &data.catalogue;
    CatalogueDraftSummary {
        id,
        game_id: catalogue.game_id.clone(),
        edition_id: catalogue.edition_id.clone(),
        catalogue_id: catalogue.catalogue_id.clone(),
        revision,
        content_hash: data.hash.clone(),
        status: "draft",
        counts: data.counts,
    }
}

fn save(
    db: &mut Connection,
    data: &Admitted,
    expected: i64,
    after_read: impl FnOnce(),
) -> Result<CatalogueDraftSummary, CatalogueError> {
    // IMMEDIATE: take the write lock before the reads, so a concurrent writer waits (busy
    // timeout) instead of failing to upgrade a read lock, and a same-identity loser sees the
    // committed revision and answers Conflict.
    let tx = db.transaction_with_behavior(TransactionBehavior::Immediate)?;
    references(&tx, data)?;
    let catalogue = &data.catalogue;
    let existing: Option<(i64, i64)> = tx
        .query_row(
            "SELECT id,revision FROM catalogue_drafts WHERE game_id=?1 AND edition_id=?2 AND catalogue_id=?3",
            params![catalogue.game_id, catalogue.edition_id, catalogue.catalogue_id],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .optional()?;
    after_read();
    let (id, revision) = match existing {
        None if expected == 0 => {
            tx.execute(
                "INSERT INTO catalogue_drafts (game_id,edition_id,catalogue_id,revision,content,content_hash) VALUES (?1,?2,?3,1,?4,?5)",
                params![catalogue.game_id, catalogue.edition_id, catalogue.catalogue_id, data.content, data.hash],
            )?;
            (tx.last_insert_rowid(), 1)
        }
        Some((id, current)) if expected > 0 && expected == current => {
            let next = current
                .checked_add(1)
                .ok_or_else(|| invalid("/revision", "revision-overflow"))?;
            let changed = tx.execute(
                "UPDATE catalogue_drafts SET content=?1,content_hash=?2,revision=?3 WHERE id=?4 AND revision=?5",
                params![data.content, data.hash, next, id, expected],
            )?;
            if changed != 1 {
                return Err(CatalogueError::Conflict);
            }
            (id, next)
        }
        _ => return Err(CatalogueError::Conflict),
    };
    tx.commit()?;
    Ok(summary(id, revision, data))
}

impl Store {
    async fn save_admitted_catalogue(
        &self,
        job: Job,
        expected_revision: i64,
    ) -> Result<CatalogueDraftSummary, CatalogueError> {
        // The DB worker owns the permit even if its async caller is cancelled.
        self.run(move |db| {
            let _permit = job.permit;
            Ok(save(db, &job.data, expected_revision, || {}))
        })
        .await?
    }

    /// Strictly admit in-memory bytes and save a revision-checked catalogue draft.
    ///
    /// # Errors
    /// Rejects saturation, malformed/oversized drafts, bad source references and stale writes.
    pub async fn save_catalogue_draft(
        &self,
        bytes: Vec<u8>,
        expected_revision: i64,
    ) -> Result<CatalogueDraftSummary, CatalogueError> {
        let job = prepare(move || Ok(bytes), expected_revision).await?;
        self.save_admitted_catalogue(job, expected_revision).await
    }

    /// Acquire admission before opening and reading the bounded regular input file.
    ///
    /// # Errors
    /// Rejects symlinks/special/oversized files, saturation, invalid drafts or stale writes.
    pub async fn import_catalogue_draft(
        &self,
        path: &Path,
        expected_revision: i64,
    ) -> Result<CatalogueDraftSummary, CatalogueError> {
        let path = path.to_owned();
        let job = prepare(
            move || Ok(crate::local_file::read_path(&path, MAX_BYTES)?),
            expected_revision,
        )
        .await?;
        self.save_admitted_catalogue(job, expected_revision).await
    }

    /// Read and re-verify a stored draft by row ID; metadata only.
    ///
    /// # Errors
    /// Rejects saturation, unknown IDs, corrupt content/hash/identity or storage failures.
    pub async fn catalogue_draft(&self, id: i64) -> Result<CatalogueDraftSummary, CatalogueError> {
        let (draft, _permit) = load_draft(self, id).await?;
        Ok(summary(draft.id, draft.revision, &draft.data))
    }
}

/// A stored draft, re-admitted and checked against its stored hash and identity.
pub(super) struct Draft {
    pub id: i64,
    pub revision: i64,
    pub data: Admitted,
}

/// Take the admission permit, read draft `id` and re-admit it outside the database mutex.
pub(super) async fn load_draft(
    store: &Store,
    id: i64,
) -> Result<(Draft, OwnedSemaphorePermit), CatalogueError> {
    if id <= 0 {
        return Err(invalid("/id", "positive-row-id-required"));
    }
    let permit = admit()?;
    let (content, stored_hash, revision, game, edition, catalogue_id, permit) = store
        .run(move |db| {
            let row: Option<(Option<String>, String, i64, String, String, String)> = db
                .query_row(
                    "SELECT CASE WHEN length(CAST(content AS BLOB))<=4194304 THEN content END,content_hash,revision,game_id,edition_id,catalogue_id FROM catalogue_drafts WHERE id=?1",
                    [id],
                    |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?, row.get(4)?, row.get(5)?)),
                )
                .optional()?;
            let (content, hash, revision, game, edition, catalogue_id) =
                row.ok_or(crate::AppError::NotFound)?;
            let content = content.ok_or(crate::AppError::Invalid(
                "Stored catalogue draft exceeds its size bound.",
            ))?;
            Ok((content, hash, revision, game, edition, catalogue_id, permit))
        })
        .await?;
    tokio::task::spawn_blocking(move || {
        let data = admission::validate(content.as_bytes())?;
        if data.content != content
            || data.hash != stored_hash
            || data.catalogue.game_id != game
            || data.catalogue.edition_id != edition
            || data.catalogue.catalogue_id != catalogue_id
        {
            return Err(invalid("/", "stored-draft-identity-or-hash-mismatch"));
        }
        Ok((Draft { id, revision, data }, permit))
    })
    .await
    .map_err(|_| CatalogueError::Worker)?
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{
        sync::{Arc, Condvar, Mutex},
        time::Duration,
    };

    /// Two parties meet after their read phase, or give up after a timeout (a writer that
    /// holds the write lock keeps the other from reaching its read phase at all).
    #[derive(Default)]
    struct Rendezvous {
        arrived: Mutex<usize>,
        turn: Condvar,
    }

    impl Rendezvous {
        fn arrive(&self) {
            let mut arrived = self.arrived.lock().unwrap();
            *arrived += 1;
            self.turn.notify_all();
            let _ = self
                .turn
                .wait_timeout_while(arrived, Duration::from_secs(2), |count| *count < 2)
                .unwrap();
        }
    }

    fn draft(catalogue_id: &str, title: &str) -> Admitted {
        let value = serde_json::json!({
            "contract_version": 1, "game_id": "synthetic-game", "edition_id": "first",
            "catalogue_id": catalogue_id, "title": title, "sources": [],
            "entry_types": [{"id": "charm", "label": "Charm", "properties": []}],
            "entries": [{"key": "house-charm", "type": "charm", "name": "House Charm",
                         "properties": {}, "description": {"owner_text": "Synthetic."}}]
        });
        admission::validate(&serde_json::to_vec(&value).unwrap()).unwrap()
    }

    async fn database() -> (tempfile::TempDir, std::path::PathBuf) {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("race.sqlite3");
        let store = Store::open(&path).unwrap();
        store
            .register_ruleset(crate::registry::RegisterRuleset {
                game_id: "synthetic-game".into(),
                game_label: "Synthetic game".into(),
                edition_id: "first".into(),
                edition_label: "First edition".into(),
            })
            .await
            .unwrap();
        (dir, path)
    }

    /// Run two saves on separate connections whose read phases would overlap.
    fn race(
        path: &std::path::Path,
        jobs: [(Admitted, i64); 2],
    ) -> Vec<Result<CatalogueDraftSummary, CatalogueError>> {
        let meet = Arc::new(Rendezvous::default());
        let threads: Vec<_> = jobs
            .into_iter()
            .map(|(data, expected)| {
                let meet = meet.clone();
                let path = path.to_owned();
                std::thread::spawn(move || {
                    let mut db = Connection::open(path).unwrap();
                    db.busy_timeout(Duration::from_secs(10)).unwrap();
                    db.execute_batch("PRAGMA foreign_keys=ON").unwrap();
                    save(&mut db, &data, expected, || meet.arrive())
                })
            })
            .collect();
        threads.into_iter().map(|t| t.join().unwrap()).collect()
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn concurrent_saves_of_different_catalogues_both_succeed() {
        let (_dir, path) = database().await;
        let results = race(
            &path,
            [(draft("spells", "One"), 0), (draft("items", "Two"), 0)],
        );
        for result in &results {
            assert!(result.is_ok(), "{result:?}");
        }
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn concurrent_saves_of_one_catalogue_give_one_winner_and_a_conflict() {
        let (_dir, path) = database().await;
        for expected in [0, 1] {
            let results = race(
                &path,
                [
                    (draft("spells", "One"), expected),
                    (draft("spells", "Two"), expected),
                ],
            );
            assert_eq!(
                results.iter().filter(|r| r.is_ok()).count(),
                1,
                "{results:?}"
            );
            assert!(
                results
                    .iter()
                    .any(|r| matches!(r, Err(CatalogueError::Conflict))),
                "{results:?}"
            );
        }
    }
}
