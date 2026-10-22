use crate::{AppError, Character, CharacterInput};
use rusqlite::{Connection, OptionalExtension, TransactionBehavior, params};
use std::{
    path::Path,
    sync::{Arc, Mutex},
    time::Duration,
};

/// Current database schema version; migration `N` sets `PRAGMA user_version=N`.
pub const SCHEMA_VERSION: i64 = 21;

/// Contiguous migrations in application order: index `N-1` holds migration `N`.
const MIGRATIONS: [&str; 21] = [
    include_str!("../migrations/001_characters.sql"),
    include_str!("../migrations/002_mork_borg.sql"),
    include_str!("../migrations/003_sources.sql"),
    include_str!("../migrations/004_source_coverage.sql"),
    include_str!("../migrations/005_registry.sql"),
    include_str!("../migrations/006_definition_drafts.sql"),
    include_str!("../migrations/007_source_finalization.sql"),
    include_str!("../migrations/008_definition_publication.sql"),
    include_str!("../migrations/009_review_integrity.sql"),
    include_str!("../migrations/010_accounts.sql"),
    include_str!("../migrations/011_auth_sessions.sql"),
    include_str!("../migrations/012_campaigns.sql"),
    include_str!("../migrations/013_owner_administration.sql"),
    include_str!("../migrations/014_account_administration.sql"),
    include_str!("../migrations/015_material_administration.sql"),
    include_str!("../migrations/016_registry_administration.sql"),
    include_str!("../migrations/017_source_search.sql"),
    include_str!("../migrations/018_formula_profile.sql"),
    include_str!("../migrations/019_catalogues.sql"),
    include_str!("../migrations/020_sheet_notes.sql"),
    include_str!("../migrations/021_row_notes.sql"),
];

#[derive(Clone)]
pub struct Store(Arc<Mutex<Connection>>, Arc<std::sync::OnceLock<NoteHook>>);

/// Called after every committed sheet save that wrote change notes: the sheet, the new
/// revision and the noted addresses (the assistant's post-commit hook, plan §1.4).
pub(crate) type NoteHook = Arc<dyn Fn(i64, i64, &[String]) + Send + Sync>;

pub(crate) static AUTH_WORKERS: std::sync::LazyLock<Arc<tokio::sync::Semaphore>> =
    std::sync::LazyLock::new(|| Arc::new(tokio::sync::Semaphore::new(16)));

pub(crate) fn register_auth_clock(
    connection: &Connection,
    clock: Arc<dyn Fn() -> Result<i64, AppError> + Send + Sync>,
) -> Result<(), AppError> {
    connection.create_scalar_function(
        "mimik_auth_now",
        0,
        rusqlite::functions::FunctionFlags::SQLITE_UTF8,
        move |_| {
            let now = clock()
                .map_err(|_| rusqlite::Error::UserFunctionError("Auth clock failed.".into()))?;
            if !(1..=253_402_300_799).contains(&now) {
                return Err(rusqlite::Error::UserFunctionError(
                    "Auth clock out of bounds.".into(),
                ));
            }
            Ok(now)
        },
    )?;
    Ok(())
}

impl Store {
    /// Open a local database and apply supported migrations.
    ///
    /// # Errors
    /// Fails on filesystem/SQLite errors or a database from a newer application schema.
    pub fn open(path: &Path) -> Result<Self, AppError> {
        Self::initialize(Connection::open(path)?)
    }

    fn initialize(connection: Connection) -> Result<Self, AppError> {
        connection.busy_timeout(Duration::from_secs(5))?;
        connection.execute_batch("PRAGMA foreign_keys = ON;")?;
        Self::migrate(connection)
    }

    fn migrate(mut connection: Connection) -> Result<Self, AppError> {
        crate::auth::register_domain_guard(&connection)?;
        register_auth_clock(
            &connection,
            Arc::new(|| {
                let seconds = std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .map_err(|_| AppError::Invalid("Auth clock failed."))?
                    .as_secs();
                i64::try_from(seconds).map_err(|_| AppError::Invalid("Auth clock overflow."))
            }),
        )?;
        let tx = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let version: i64 = tx.query_row("PRAGMA user_version", [], |r| r.get(0))?;
        if version > SCHEMA_VERSION {
            return Err(AppError::Invalid(
                "Database schema is newer than this application.",
            ));
        }
        for (number, migration) in (1_i64..).zip(MIGRATIONS) {
            if version < number {
                tx.execute_batch(migration)?;
            }
        }
        crate::sources::search::ensure(&tx)?;
        tx.commit()?;
        Ok(Self(
            Arc::new(Mutex::new(connection)),
            Arc::new(std::sync::OnceLock::new()),
        ))
    }

    /// Installs the post-commit note hook; a second one is refused.
    pub(crate) fn set_note_hook(&self, hook: NoteHook) -> Result<(), AppError> {
        self.1
            .set(hook)
            .map_err(|_| AppError::Invalid("A note hook is already installed."))
    }

    /// Runs the post-commit note hook, when one is installed.
    pub(crate) fn notes_committed(&self, sheet: i64, revision: i64, addresses: &[String]) {
        if let Some(hook) = self.1.get()
            && !addresses.is_empty()
        {
            hook(sheet, revision, addresses);
        }
    }

    pub(crate) async fn run<T: Send + 'static>(
        &self,
        operation: impl FnOnce(&mut Connection) -> Result<T, AppError> + Send + 'static,
    ) -> Result<T, AppError> {
        let connection = self.0.clone();
        tokio::task::spawn_blocking(move || {
            let mut guard = connection.lock().map_err(|_| AppError::Worker)?;
            operation(&mut guard)
        })
        .await
        .map_err(|_| AppError::Worker)?
    }

    pub(crate) async fn run_bounded_auth<T: Send + 'static>(
        &self,
        operation: impl FnOnce(&mut Connection) -> Result<T, AppError> + Send + 'static,
    ) -> Result<T, AppError> {
        let permit = AUTH_WORKERS
            .clone()
            .try_acquire_owned()
            .map_err(|_| AppError::AuthStorageBusy)?;
        self.run(move |db| {
            let _permit = permit;
            operation(db)
        })
        .await
    }

    pub(crate) async fn run_campaign<T: Send + 'static>(
        &self,
        auth: crate::auth::RequestAuth,
        operation: impl FnOnce(&mut Connection) -> Result<T, AppError> + Send + 'static,
    ) -> Result<T, AppError> {
        self.run_bounded_auth(move |db| crate::auth::with_domain_request(auth, || operation(db)))
            .await
    }

    /// List character records, newest first.
    ///
    /// # Errors
    /// Returns storage/worker failures.
    pub async fn list(&self) -> Result<Vec<Character>, AppError> {
        self.run(|db| {
            let mut query = db.prepare("SELECT c.id,c.name,c.system_id,c.edition,c.notes,c.revision,r.game_id,r.edition_id FROM characters c LEFT JOIN character_rulesets r ON r.character_id=c.id ORDER BY c.id DESC")?;
            let rows = query.query_map([], read_character)?;
            Ok(rows.collect::<Result<Vec<_>, _>>()?)
        }).await
    }

    /// Load a character by its local database identifier.
    ///
    /// # Errors
    /// Fails if the character does not exist or storage is unavailable.
    pub async fn get(&self, id: i64) -> Result<Character, AppError> {
        self.run(move |db| load(db, id)).await
    }

    /// Create a validated character identity record.
    ///
    /// # Errors
    /// Fails on invalid input or storage/worker failures.
    pub async fn create(&self, input: CharacterInput) -> Result<Character, AppError> {
        let input = input.normalize();
        input.validate()?;
        self.run(move |db| {
            let tx = db.transaction()?;
            let character = insert_character(&tx, &input)?;
            tx.commit()?;
            Ok(character)
        })
        .await
    }

    /// Update an identity using optimistic revision checks.
    ///
    /// # Errors
    /// Fails on invalid input, an unknown character, stale revision or storage errors.
    pub async fn update(&self, id: i64, input: CharacterInput) -> Result<Character, AppError> {
        let input = input.normalize();
        input.validate()?;
        self.run(move |db| {
            let tx = db.transaction()?;
            let current = load(&tx, id)?;
            if current.input.revision != input.revision { return Err(AppError::Conflict); }
            if current.input.system_id != input.system_id || current.input.edition != input.edition {
                return Err(AppError::Invalid("Game and edition cannot be changed on an existing record."));
            }
            let changed = tx.execute("UPDATE characters SET name = ?1, notes = ?2, revision = revision + 1 WHERE id = ?3 AND revision = ?4",
                params![input.name, input.notes, id, input.revision])?;
            if changed != 1 { return Err(AppError::Conflict); }
            let result = load(&tx, id)?;
            tx.commit()?;
            Ok(result)
        }).await
    }
}

pub(crate) fn insert_character(
    db: &Connection,
    input: &CharacterInput,
) -> Result<Character, AppError> {
    db.execute(
        "INSERT INTO characters (name,system_id,edition,notes) VALUES (?1,?2,?3,?4)",
        params![input.name, input.system_id, input.edition, input.notes],
    )?;
    let id = db.last_insert_rowid();
    db.execute("INSERT INTO character_rulesets SELECT ?1,game_id,edition_id FROM legacy_ruleset_aliases WHERE system_id=?2 AND edition=?3", params![id,input.system_id,input.edition])?;
    load(db, id)
}

fn read_character(row: &rusqlite::Row<'_>) -> rusqlite::Result<Character> {
    Ok(Character {
        id: row.get(0)?,
        input: CharacterInput {
            name: row.get(1)?,
            system_id: row.get(2)?,
            edition: row.get(3)?,
            notes: row.get(4)?,
            revision: row.get(5)?,
        },
        ruleset: row
            .get::<_, Option<String>>(6)?
            .map(|game_id| {
                row.get(7).map(|edition_id| crate::registry::RulesetKey {
                    game_id,
                    edition_id,
                })
            })
            .transpose()?,
    })
}

pub(crate) fn load(db: &Connection, id: i64) -> Result<Character, AppError> {
    db.query_row(
        "SELECT c.id,c.name,c.system_id,c.edition,c.notes,c.revision,r.game_id,r.edition_id FROM characters c LEFT JOIN character_rulesets r ON r.character_id=c.id WHERE c.id=?1",
        [id],
        read_character,
    )
    .optional()?
    .ok_or(AppError::NotFound)
}

#[cfg(test)]
mod migration_tests {
    use super::*;
    use std::sync::mpsc::{self, Receiver, Sender};

    struct BusySync {
        blocked: Sender<()>,
        resume: Receiver<()>,
    }

    static BUSY_SYNC: Mutex<Option<BusySync>> = Mutex::new(None);

    fn wait_for_migration_commit(attempt: i32) -> bool {
        assert_eq!(
            attempt, 0,
            "migration should only wait for the held writer once"
        );
        let guard = BUSY_SYNC.lock().unwrap();
        let sync = guard.as_ref().unwrap();
        sync.blocked.send(()).unwrap();
        sync.resume.recv_timeout(Duration::from_secs(10)).unwrap();
        true
    }

    #[test]
    fn concurrent_migration_reads_version_after_writer_commits() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("concurrent.sqlite3");
        let mut writer = Connection::open(&path).unwrap();
        let tx = writer
            .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)
            .unwrap();
        let (blocked_tx, blocked_rx) = mpsc::channel();
        let (resume_tx, resume_rx) = mpsc::channel();
        *BUSY_SYNC.lock().unwrap() = Some(BusySync {
            blocked: blocked_tx,
            resume: resume_rx,
        });
        let worker = std::thread::spawn(move || {
            let connection = Connection::open(path).unwrap();
            connection.execute_batch("PRAGMA foreign_keys=ON").unwrap();
            connection
                .busy_handler(Some(wait_for_migration_commit))
                .unwrap();
            Store::migrate(connection)
        });
        // SQLite itself signals lock contention; no scheduler timing or sleeps.
        blocked_rx.recv_timeout(Duration::from_secs(10)).unwrap();
        for migration in &MIGRATIONS[..10] {
            tx.execute_batch(migration).unwrap();
        }
        tx.commit().unwrap();
        resume_tx.send(()).unwrap();
        let result = worker.join().unwrap();
        *BUSY_SYNC.lock().unwrap() = None;
        assert!(
            result.is_ok(),
            "concurrent opener reused a stale schema version: {:?}",
            result.err()
        );
        assert_eq!(
            writer
                .query_row("PRAGMA user_version", [], |row| row.get::<_, i64>(0))
                .unwrap(),
            SCHEMA_VERSION
        );
    }

    #[test]
    fn migration_files_are_contiguous_and_match_the_applied_chain() {
        let directory = Path::new(env!("CARGO_MANIFEST_DIR")).join("migrations");
        let mut files = std::fs::read_dir(&directory)
            .unwrap()
            .map(|entry| entry.unwrap().file_name().into_string().unwrap())
            .collect::<Vec<_>>();
        files.sort();
        assert_eq!(i64::try_from(MIGRATIONS.len()).unwrap(), SCHEMA_VERSION);
        assert_eq!(files.len(), MIGRATIONS.len(), "{files:?}");
        for ((number, file), applied) in (1_i64..).zip(&files).zip(MIGRATIONS) {
            let (prefix, rest) = file.split_once('_').unwrap();
            assert_eq!(
                prefix,
                format!("{number:03}"),
                "migration numbers must be contiguous: {files:?}"
            );
            let name = Path::new(rest);
            assert!(
                name.extension().is_some_and(|extension| extension == "sql")
                    && name.file_stem().is_some_and(|stem| !stem.is_empty()),
                "{file}"
            );
            let contents = std::fs::read_to_string(directory.join(file)).unwrap();
            assert_eq!(contents, applied, "{file} is out of chain order");
            // `--` comment lines are not statements; a version-only migration may start with one.
            let statements: String = contents
                .lines()
                .filter(|line| !line.trim_start().starts_with("--"))
                .collect::<Vec<_>>()
                .join("\n");
            let last = statements.trim_end().rsplit(';').nth(1).unwrap().trim();
            assert_eq!(
                last.replace(' ', ""),
                format!("PRAGMAuser_version={number}"),
                "{file} must end by setting its own schema version"
            );
        }
    }
}
