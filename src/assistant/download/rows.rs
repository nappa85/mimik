//! The download rows as the download module sees them: plain data and a trait. The database
//! implementation lives outside this module (`assistant/download_rows.rs`).
use async_trait::async_trait;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum State {
    /// Confirmed by the owner: waiting, running or stopped (the partial's length is progress).
    Confirmed,
    /// A verified file is in the models folder.
    Complete,
}

/// One confirmed download.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Row {
    pub id: i64,
    /// The spec as the owner wrote it (`hf:<org>/<repo>/<path>@<revision>`).
    pub spec: String,
    /// The file's name in the models folder.
    pub file_name: String,
    pub sha256: String,
    pub size: u64,
    pub licence_id: Option<String>,
    pub licence_path: Option<String>,
    pub licence_sha256: Option<String>,
    pub confirmed_by: i64,
    pub confirmed_at: i64,
    pub state: State,
    pub completed_at: Option<i64>,
}

/// A download to record (the review's snapshot, as confirmed).
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct New {
    pub spec: String,
    pub file_name: String,
    pub sha256: String,
    pub size: u64,
    pub licence_id: Option<String>,
    pub licence_path: Option<String>,
    pub licence_sha256: Option<String>,
    pub confirmed_by: i64,
}

#[async_trait]
pub(crate) trait Rows: Send + Sync {
    /// Every row, oldest first.
    async fn list(&self) -> Result<Vec<Row>, String>;
    /// A new `confirmed` row; a spec that already has one is refused.
    async fn insert(&self, new: New) -> Result<Row, String>;
    /// Marks the row `complete`.
    async fn complete(&self, id: i64) -> Result<(), String>;
    /// Makes a `complete` row `confirmed` again (its file is gone and is fetched again; the
    /// licence snapshot stays).
    async fn reopen(&self, id: i64) -> Result<(), String>;
    /// Removes the row.
    async fn delete(&self, id: i64) -> Result<(), String>;
    /// Whether the stored model choice (the active or previous model) uses the file `name`.
    async fn in_use(&self, file_name: &str) -> Result<bool, String>;
}

#[cfg(test)]
pub(crate) mod memory {
    //! An in-memory [`Rows`] for the task tests, with failures on demand.
    use super::*;
    use std::sync::{
        Mutex,
        atomic::{AtomicBool, AtomicI64, Ordering},
    };

    #[derive(Default)]
    pub(crate) struct Memory {
        pub rows: Mutex<Vec<Row>>,
        next: AtomicI64,
        /// While set, `complete` fails (a database commit failure after the publication).
        pub fail_complete: AtomicBool,
        /// While set, `list` and `delete` fail.
        pub fail_list: AtomicBool,
        pub fail_delete: AtomicBool,
        pub in_use: Mutex<Vec<String>>,
    }

    impl Memory {
        /// Sets a row's state directly (a test of a row left behind by a crash).
        pub(crate) fn set_state(&self, id: i64, state: State) {
            for row in self.rows.lock().unwrap().iter_mut() {
                if row.id == id {
                    row.state = state;
                    row.completed_at = (state == State::Complete).then_some(2);
                }
            }
        }
    }

    #[async_trait]
    impl Rows for Memory {
        async fn list(&self) -> Result<Vec<Row>, String> {
            if self.fail_list.load(Ordering::SeqCst) {
                return Err("the database refused the read".into());
            }
            Ok(self.rows.lock().unwrap().clone())
        }

        async fn insert(&self, new: New) -> Result<Row, String> {
            let mut rows = self.rows.lock().unwrap();
            if rows.iter().any(|row| row.spec == new.spec) {
                return Err("already confirmed".into());
            }
            let row = Row {
                id: self.next.fetch_add(1, Ordering::SeqCst) + 1,
                spec: new.spec,
                file_name: new.file_name,
                sha256: new.sha256,
                size: new.size,
                licence_id: new.licence_id,
                licence_path: new.licence_path,
                licence_sha256: new.licence_sha256,
                confirmed_by: new.confirmed_by,
                confirmed_at: 1,
                state: State::Confirmed,
                completed_at: None,
            };
            rows.push(row.clone());
            Ok(row)
        }

        async fn complete(&self, id: i64) -> Result<(), String> {
            if self.fail_complete.load(Ordering::SeqCst) {
                return Err("the database refused the write".into());
            }
            for row in self.rows.lock().unwrap().iter_mut() {
                if row.id == id {
                    row.state = State::Complete;
                    row.completed_at = Some(2);
                }
            }
            Ok(())
        }

        async fn reopen(&self, id: i64) -> Result<(), String> {
            self.set_state(id, State::Confirmed);
            Ok(())
        }

        async fn delete(&self, id: i64) -> Result<(), String> {
            if self.fail_delete.load(Ordering::SeqCst) {
                return Err("the database refused the delete".into());
            }
            self.rows.lock().unwrap().retain(|row| row.id != id);
            Ok(())
        }

        async fn in_use(&self, file_name: &str) -> Result<bool, String> {
            Ok(self
                .in_use
                .lock()
                .unwrap()
                .iter()
                .any(|name| name == file_name))
        }
    }
}
