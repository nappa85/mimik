//! Revisioned catalogue drafts (plan §3 step 1), mirroring `definitions::store`: one
//! nonqueued admission job per process (the shared definition permit), the bounded local
//! reader, and source consistency checks inside the write transaction.
use super::{
    CatalogueDraftSummary, CatalogueError, MAX_BYTES,
    admission::{self, Admitted},
    invalid,
};
use crate::{Store, registry::RulesetKey};
use rusqlite::{Connection, OptionalExtension, params};
use std::{collections::BTreeSet, path::Path};
use tokio::sync::OwnedSemaphorePermit;

struct Job {
    data: Admitted,
    permit: OwnedSemaphorePermit,
}

fn admit() -> Result<OwnedSemaphorePermit, CatalogueError> {
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
fn references(db: &Connection, data: &Admitted) -> Result<(), CatalogueError> {
    let catalogue = &data.catalogue;
    let key = RulesetKey {
        game_id: catalogue.game_id.clone(),
        edition_id: catalogue.edition_id.clone(),
    };
    let native: Option<bool> = db
        .query_row(
            "SELECT native FROM registry_editions WHERE game_id=?1 AND edition_id=?2",
            params![key.game_id, key.edition_id],
            |row| row.get(0),
        )
        .optional()?;
    if native.is_none() || native == Some(true) || crate::games::adapter_for_ruleset(&key).is_some()
    {
        return Err(invalid("/", "registered-nonnative-ruleset-required"));
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
) -> Result<CatalogueDraftSummary, CatalogueError> {
    let tx = db.transaction()?;
    references(&tx, data)?;
    let catalogue = &data.catalogue;
    let existing: Option<(i64, i64)> = tx
        .query_row(
            "SELECT id,revision FROM catalogue_drafts WHERE game_id=?1 AND edition_id=?2 AND catalogue_id=?3",
            params![catalogue.game_id, catalogue.edition_id, catalogue.catalogue_id],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .optional()?;
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
            Ok(save(db, &job.data, expected_revision))
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
        if id <= 0 {
            return Err(invalid("/id", "positive-row-id-required"));
        }
        let permit = admit()?;
        let (content, stored_hash, revision, game, edition, catalogue_id, permit) = self
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
            let _permit = permit;
            let data = admission::validate(content.as_bytes())?;
            if data.content != content
                || data.hash != stored_hash
                || data.catalogue.game_id != game
                || data.catalogue.edition_id != edition
                || data.catalogue.catalogue_id != catalogue_id
            {
                return Err(invalid("/", "stored-draft-identity-or-hash-mismatch"));
            }
            Ok(summary(id, revision, &data))
        })
        .await
        .map_err(|_| CatalogueError::Worker)?
    }
}
