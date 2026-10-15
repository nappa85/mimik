use super::{
    DefinitionDraft, DefinitionError, DraftSummary, MAX_BYTES, Mapping, ValidationReport,
    admission::{self, Admitted},
    admit,
};
use crate::{AppError, Store, registry::RulesetKey};
use rusqlite::{Connection, OptionalExtension, params};
use std::path::Path;
use tokio::sync::OwnedSemaphorePermit;

struct Job {
    data: Admitted,
    permit: OwnedSemaphorePermit,
}

async fn prepare(
    input: impl FnOnce() -> Result<Vec<u8>, DefinitionError> + Send + 'static,
    expected_revision: i64,
) -> Result<Job, DefinitionError> {
    let permit = admit()?;
    if expected_revision < 0 {
        return Err(ValidationReport::one(
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
    .map_err(|_| DefinitionError::Worker)?
}

pub(super) fn references(db: &Connection, data: &Admitted) -> Result<(), DefinitionError> {
    let definition = &data.definition;
    let key = RulesetKey {
        game_id: definition.game_id.clone(),
        edition_id: definition.edition_id.clone(),
    };
    let native: Option<bool> = db
        .query_row(
            "SELECT native FROM registry_editions WHERE game_id=?1 AND edition_id=?2",
            params![key.game_id, key.edition_id],
            |row| row.get(0),
        )
        .optional()
        .map_err(AppError::from)?;
    if native.is_none() || native == Some(true) || crate::games::adapter_for_ruleset(&key).is_some()
    {
        return Err(ValidationReport::one(
            "/",
            "registered-nonnative-ruleset-required",
        ));
    }
    for source in &definition.sources {
        let matched: i64 = db.query_row("SELECT count(*) FROM sources s JOIN source_rulesets r ON r.source_id=s.id WHERE s.id=?1 AND s.pdf_hash=?2 AND r.game_id=?3 AND r.edition_id=?4", params![source.source_id,source.pdf_sha256,key.game_id,key.edition_id], |row| row.get(0)).map_err(AppError::from)?;
        if matched != 1 {
            return Err(ValidationReport::one(
                "/sources",
                "source-ruleset-or-pdf-hash-mismatch",
            ));
        }
    }
    for field in &definition.fields {
        if let Mapping::Mapped { regions } = &field.mapping {
            for region in regions {
                let matched: i64 = db.query_row("SELECT count(*) FROM source_units u JOIN source_pages p ON p.id=u.page_id WHERE u.id=?1 AND p.id=?2 AND p.source_id=?3", params![region.unit_id,region.page_id,region.source_id], |row| row.get(0)).map_err(AppError::from)?;
                if matched != 1 {
                    return Err(ValidationReport::one(
                        "/fields",
                        "mapped-unit-page-source-mismatch",
                    ));
                }
            }
        }
    }
    Ok(())
}

fn save(
    db: &mut Connection,
    data: &Admitted,
    expected: i64,
) -> Result<DefinitionDraft, DefinitionError> {
    let tx = db.transaction().map_err(AppError::from)?;
    references(&tx, data)?;
    let definition = &data.definition;
    let existing: Option<(i64,i64)> = tx.query_row("SELECT id,revision FROM definition_drafts WHERE game_id=?1 AND edition_id=?2 AND definition_id=?3", params![definition.game_id,definition.edition_id,definition.definition_id], |row| Ok((row.get(0)?,row.get(1)?))).optional().map_err(AppError::from)?;
    let (id, revision) = match existing {
        None if expected == 0 => {
            tx.execute("INSERT INTO definition_drafts (game_id,edition_id,definition_id,revision,content,content_hash) VALUES (?1,?2,?3,1,?4,?5)", params![definition.game_id,definition.edition_id,definition.definition_id,data.content,data.hash]).map_err(AppError::from)?;
            (tx.last_insert_rowid(), 1)
        }
        Some((id, current)) if expected > 0 && expected == current => {
            let next = current
                .checked_add(1)
                .ok_or_else(|| ValidationReport::one("/revision", "revision-overflow"))?;
            let changed = tx.execute("UPDATE definition_drafts SET content=?1,content_hash=?2,revision=?3 WHERE id=?4 AND revision=?5", params![data.content,data.hash,next,id,expected]).map_err(AppError::from)?;
            if changed != 1 {
                return Err(DefinitionError::Conflict);
            }
            (id, next)
        }
        _ => return Err(DefinitionError::Conflict),
    };
    tx.commit().map_err(AppError::from)?;
    Ok(DefinitionDraft {
        id,
        revision,
        content_hash: data.hash.clone(),
        status: "draft",
        definition: data.definition.clone(),
    })
}

impl Store {
    async fn save_admitted(
        &self,
        job: Job,
        expected_revision: i64,
    ) -> Result<DefinitionDraft, DefinitionError> {
        // The DB worker owns the permit even if its async caller is cancelled.
        self.run(move |db| {
            let _permit = job.permit;
            Ok(save(db, &job.data, expected_revision))
        })
        .await?
    }

    /// Strictly admit existing in-memory bytes and save a revision-checked draft.
    ///
    /// # Errors
    /// Rejects saturation, malformed/oversized proposals, bad references and stale writes.
    pub async fn save_definition_draft(
        &self,
        bytes: Vec<u8>,
        expected_revision: i64,
    ) -> Result<DefinitionDraft, DefinitionError> {
        let job = prepare(move || Ok(bytes), expected_revision).await?;
        self.save_admitted(job, expected_revision).await
    }

    /// Acquire admission before opening/reading the bounded regular input file.
    ///
    /// # Errors
    /// Rejects symlinks/special/oversized files, saturation, invalid proposals or stale writes.
    pub async fn import_definition_draft(
        &self,
        path: &Path,
        expected_revision: i64,
    ) -> Result<DefinitionDraft, DefinitionError> {
        let path = path.to_owned();
        let job = prepare(
            move || Ok(crate::local_file::read_path(&path, MAX_BYTES)?),
            expected_revision,
        )
        .await?;
        self.save_admitted(job, expected_revision).await
    }

    /// Read and verify a stored draft by its database row ID, without approving it.
    ///
    /// # Errors
    /// Rejects saturation, unknown IDs, corrupt content/hash or storage/worker errors.
    pub async fn definition_draft(&self, id: i64) -> Result<DefinitionDraft, DefinitionError> {
        if id <= 0 {
            return Err(ValidationReport::one("/id", "positive-row-id-required"));
        }
        let permit = admit()?;
        let (content, stored_hash, revision, key, definition_id, permit) = self.run(move |db| {
            let (content, hash, revision, game, edition, definition_id): (Option<String>,String,i64,String,String,String) = db.query_row("SELECT CASE WHEN length(CAST(content AS BLOB))<=262144 THEN content END,content_hash,revision,game_id,edition_id,definition_id FROM definition_drafts WHERE id=?1", [id], |row| Ok((row.get(0)?,row.get(1)?,row.get(2)?,row.get(3)?,row.get(4)?,row.get(5)?))).optional()?.ok_or(AppError::NotFound)?;
            Ok((content.ok_or(AppError::Invalid("Stored draft exceeds its size bound."))?,hash,revision,RulesetKey { game_id: game,edition_id: edition },definition_id,permit))
        }).await.map_err(|error| if matches!(error, AppError::NotFound) { DefinitionError::NotFound } else { error.into() })?;
        tokio::task::spawn_blocking(move || {
            let _permit = permit;
            let data = admission::validate(content.as_bytes())?;
            if data.content != content
                || data.hash != stored_hash
                || data.definition.game_id != key.game_id
                || data.definition.edition_id != key.edition_id
                || data.definition.definition_id != definition_id
            {
                return Err(ValidationReport::one(
                    "/",
                    "stored-draft-identity-or-hash-mismatch",
                ));
            }
            Ok(DefinitionDraft {
                id,
                revision,
                content_hash: stored_hash,
                status: "draft",
                definition: data.definition,
            })
        })
        .await
        .map_err(|_| DefinitionError::Worker)?
    }

    /// Read at most 100 summaries in ascending row-ID order, exclusively after `after_id`.
    ///
    /// # Errors
    /// Rejects invalid IDs/cursor or storage failures.
    pub async fn definition_drafts(
        &self,
        game: String,
        edition: String,
        after_id: i64,
    ) -> Result<Vec<DraftSummary>, DefinitionError> {
        if !crate::registry::valid_id(&game) || !crate::registry::valid_id(&edition) || after_id < 0
        {
            return Err(ValidationReport::one("/", "invalid-ruleset-or-cursor"));
        }
        Ok(self.run(move |db| {
            let mut query = db.prepare("SELECT id,definition_id,revision,content_hash FROM definition_drafts WHERE game_id=?1 AND edition_id=?2 AND id>?3 ORDER BY id LIMIT 100")?;
            let rows = query.query_map(params![game,edition,after_id], |row| Ok(DraftSummary { id: row.get(0)?, definition_id: row.get(1)?, revision: row.get(2)?, content_hash: row.get(3)?, status: "draft" }))?;
            Ok(rows.collect::<Result<Vec<_>,_>>()?)
        }).await?)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{future::Future, time::Duration};

    async fn wait_for_release() {
        tokio::time::timeout(Duration::from_secs(5), async {
            loop {
                if let Ok(permit) = admit() {
                    drop(permit);
                    break;
                }
                tokio::task::yield_now().await;
            }
        })
        .await
        .unwrap();
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn cancelled_admission_and_db_workers_keep_capacity_until_blocking_work_finishes() {
        let _guard = super::super::TEST_RESOURCE_LOCK.lock().await;
        let (started, entered) = tokio::sync::oneshot::channel();
        let (release, unblock) = std::sync::mpsc::channel();
        let task = tokio::spawn(prepare(
            move || {
                started.send(()).unwrap();
                unblock.recv().unwrap();
                Ok(include_bytes!("../../schemas/definition-example.json").to_vec())
            },
            0,
        ));
        entered.await.unwrap();
        task.abort();
        assert!(matches!(task.await, Err(error) if error.is_cancelled()));
        assert!(matches!(admit(), Err(DefinitionError::Busy)));
        release.send(()).unwrap();
        wait_for_release().await;

        let dir = tempfile::tempdir().unwrap();
        let store = Store::open(&dir.path().join("cancel.sqlite3")).unwrap();
        store
            .register_ruleset(crate::registry::RegisterRuleset {
                game_id: "synthetic-game".into(),
                game_label: "Synthetic game".into(),
                edition_id: "first".into(),
                edition_label: "First edition".into(),
            })
            .await
            .unwrap();
        let (started, entered) = tokio::sync::oneshot::channel();
        let (release, unblock) = std::sync::mpsc::channel();
        let locked = store.clone();
        let lock = tokio::spawn(async move {
            locked
                .run(move |_| {
                    started.send(()).unwrap();
                    unblock.recv().unwrap();
                    Ok(())
                })
                .await
        });
        entered.await.unwrap();
        let job = Job {
            data: admission::validate(include_bytes!("../../schemas/definition-example.json"))
                .unwrap(),
            permit: admit().unwrap(),
        };
        let (started, enqueued) = tokio::sync::oneshot::channel();
        let pending = store.clone();
        let task = tokio::spawn(async move {
            let mut operation = Box::pin(pending.save_admitted(job, 0));
            let mut started = Some(started);
            std::future::poll_fn(|context| {
                let result = operation.as_mut().poll(context);
                if let Some(started) = started.take() {
                    started.send(()).unwrap();
                }
                result
            })
            .await
        });
        enqueued.await.unwrap();
        task.abort();
        assert!(matches!(task.await, Err(error) if error.is_cancelled()));
        assert!(matches!(admit(), Err(DefinitionError::Busy)));
        release.send(()).unwrap();
        lock.await.unwrap().unwrap();
        wait_for_release().await;
        // Cancellation after enqueue may commit atomically; it cannot free the
        // resource while detached DB work is still active.
        assert_eq!(
            store
                .definition_drafts("synthetic-game".into(), "first".into(), 0)
                .await
                .unwrap()
                .len(),
            1
        );
    }
}
