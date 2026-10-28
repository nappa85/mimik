//! The download rows in the application's database (migration 023): the implementation of
//! the download module's `Rows`.
use super::download::rows::{New, Row, Rows, State};
use crate::Store;
use async_trait::async_trait;
use rusqlite::{OptionalExtension, params};

#[derive(Clone)]
pub(crate) struct SqlRows(pub Store);

const COLUMNS: &str = "id,spec,file_name,sha256,size,licence_id,licence_path,licence_sha256,confirmed_by,confirmed_at,state,completed_at";

fn row(r: &rusqlite::Row<'_>) -> rusqlite::Result<Row> {
    let state: String = r.get(10)?;
    Ok(Row {
        id: r.get(0)?,
        spec: r.get(1)?,
        file_name: r.get(2)?,
        sha256: r.get(3)?,
        size: u64::try_from(r.get::<_, i64>(4)?).unwrap_or(0),
        licence_id: r.get(5)?,
        licence_path: r.get(6)?,
        licence_sha256: r.get(7)?,
        confirmed_by: r.get(8)?,
        confirmed_at: r.get(9)?,
        state: if state == "complete" {
            State::Complete
        } else {
            State::Confirmed
        },
        completed_at: r.get(11)?,
    })
}

fn failed<T>(result: Result<T, crate::AppError>) -> Result<T, String> {
    result.map_err(|_| "the download record cannot be stored".to_owned())
}

#[async_trait]
impl Rows for SqlRows {
    async fn list(&self) -> Result<Vec<Row>, String> {
        failed(
            self.0
                .run(|db| {
                    let mut statement = db.prepare(&format!(
                        "SELECT {COLUMNS} FROM assistant_downloads ORDER BY id"
                    ))?;
                    Ok(statement
                        .query_map([], row)?
                        .collect::<rusqlite::Result<Vec<_>>>()?)
                })
                .await,
        )
    }

    async fn insert(&self, new: New) -> Result<Row, String> {
        failed(
            self.0
                .run(move |db| {
                    let size = i64::try_from(new.size).unwrap_or(i64::MAX);
                    db.execute(
                        "INSERT INTO assistant_downloads(spec,file_name,sha256,size,licence_id,licence_path,licence_sha256,confirmed_by,confirmed_at,state)
                         VALUES(?1,?2,?3,?4,?5,?6,?7,?8,mimik_auth_now(),'confirmed')",
                        params![
                            new.spec,
                            new.file_name,
                            new.sha256,
                            size,
                            new.licence_id,
                            new.licence_path,
                            new.licence_sha256,
                            new.confirmed_by
                        ],
                    )?;
                    let id = db.last_insert_rowid();
                    Ok(db.query_row(
                        &format!("SELECT {COLUMNS} FROM assistant_downloads WHERE id=?1"),
                        [id],
                        row,
                    )?)
                })
                .await,
        )
    }

    async fn complete(&self, id: i64) -> Result<(), String> {
        failed(
            self.0
                .run(move |db| {
                    db.execute(
                        "UPDATE assistant_downloads SET state='complete',completed_at=mimik_auth_now() WHERE id=?1",
                        [id],
                    )?;
                    Ok(())
                })
                .await,
        )
    }

    async fn delete(&self, id: i64) -> Result<(), String> {
        failed(
            self.0
                .run(move |db| {
                    db.execute("DELETE FROM assistant_downloads WHERE id=?1", [id])?;
                    Ok(())
                })
                .await,
        )
    }

    async fn in_use(&self, file_name: &str) -> Result<bool, String> {
        let name = file_name.to_owned();
        failed(
            self.0
                .run(move |db| {
                    Ok(db
                        .query_row(
                            "SELECT 1 FROM assistant_models m JOIN assistant_model_choice c ON m.id IN (c.active_model,c.previous_model) WHERE m.file_name=?1",
                            [name],
                            |_| Ok(()),
                        )
                        .optional()?
                        .is_some())
                })
                .await,
        )
    }
}

#[cfg(test)]
#[path = "download_rows_tests.rs"]
mod tests;
