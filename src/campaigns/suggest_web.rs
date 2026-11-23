//! The suggest route (assisted sheet step 4, plan item 7): `GET
//! /private-sheets/{id}/suggest/{f}/{p}?q=…[&typo=1]`, JSON only, for the sheet module's
//! combobox on a catalogue-backed name cell (the new-row name uses the same route with its
//! list and property).
//!
//! Order: raw query, `q` and the address are checked first (400, before the session); then
//! the route's own pool of 4 (`SUGGEST`, 429 at saturation); request auth; one read
//! transaction that reads only the sheet's identity and pin columns (never the raw sheet:
//! this runs per keystroke): the read right, the pin's definition row, the member's
//! catalogue and its latest sealed version, and the name cache (a miss reads the version's
//! names in that same transaction). The member facts per (field, property) come from a small
//! cache keyed by the validated definition's content hash; a hit needs no runtime permit, a
//! miss takes it once (try only, 503 when busy, R6) to validate the pin in `spawn_blocking`.
//! The owned pool permit moves into every blocking closure the route starts, so an aborted
//! request holds it until its work ends.
//!
//! Every refusal after the shape checks is one uniform 403 `{"status": "unavailable"}`: a
//! forged address, a property without a member, an unlinked catalogue, another game or
//! edition, an unsupported or corrupt pin, or no sealed version look the same.
use super::{SheetPage, SuggestAnswer, sheet_json};
use crate::{
    AppError, Store,
    auth::{self, AuthState, RequestAuth},
    campaigns::{
        snapshots::{self, Pin},
        values,
    },
    catalogues::{
        read,
        suggest::{self, NAMES, NameIndex},
    },
    definitions::private_pin,
};
use axum::{
    extract::{Path, State},
    http::{HeaderMap, StatusCode, Uri, header},
    response::{IntoResponse, Response},
};
use hashlink::LruCache;
use rusqlite::{OptionalExtension, params};
use serde_json::{Value, json};
use std::sync::{Arc, LazyLock, Mutex, PoisonError};
use tokio::sync::{OwnedSemaphorePermit, Semaphore};
use tower_sessions::Session;

/// The route's own pool: suggestions never take the `JOBS` permit on a pin-facts hit.
pub(in crate::campaigns) static SUGGEST: LazyLock<Arc<Semaphore>> =
    LazyLock::new(|| Arc::new(Semaphore::new(4)));

/// Raw query bound.
const MAX_QUERY_BYTES: usize = 1024;
/// `q` bounds, in scalars and bytes (before folding), and after folding (3 for a typo pass).
const MIN_SCALARS: usize = 2;
const MAX_SCALARS: usize = 64;
const MAX_Q_BYTES: usize = 256;
const MIN_TYPO_SCALARS: usize = 3;
/// Pin-fact sets kept (each is a few hundred bytes).
const FACTS_KEPT: usize = 64;

/// The catalogue member of one property: the catalogue's id and the member's entry type.
#[derive(Clone)]
pub(super) struct Member {
    pub catalogue: String,
    pub entry_type: Option<String>,
}

/// What a validated definition says about suggestions: its game and edition and, per field
/// and property, the property's catalogue member when its catalogue is linked.
pub(super) struct Facts {
    members: Vec<Vec<Option<Member>>>,
}

impl Facts {
    fn of(pin: &private_pin::ImmutablePin) -> Self {
        let definition = pin.definition();
        let members = definition
            .fields
            .iter()
            .map(|field| {
                if field.control != crate::definitions::Control::List || field.formula.is_some() {
                    return Vec::new();
                }
                values::properties(field)
                    .iter()
                    .map(|property| {
                        values::linked_member(definition, property).map(|member| Member {
                            catalogue: member.id.clone(),
                            entry_type: member.entry_type.clone(),
                        })
                    })
                    .collect()
            })
            .collect();
        Self { members }
    }

    fn member(&self, f: usize, p: usize) -> Option<&Member> {
        self.members.get(f)?.get(p)?.as_ref()
    }
}

/// The pin-facts cache, keyed by the definition's content hash.
static FACTS: LazyLock<Mutex<LruCache<String, Arc<Facts>>>> =
    LazyLock::new(|| Mutex::new(LruCache::new(FACTS_KEPT)));

fn cached_facts(hash: &str) -> Option<Arc<Facts>> {
    FACTS
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .get(hash)
        .cloned()
}

/// Forgets every pin-fact set (tests only).
#[cfg(test)]
pub(in crate::campaigns) fn forget_facts() {
    FACTS.lock().unwrap_or_else(PoisonError::into_inner).clear();
}

/// Runs inside the matching closure, before it matches (tests only).
#[cfg(test)]
pub(in crate::campaigns) static IN_MATCH: Mutex<Option<Arc<dyn Fn() + Send + Sync>>> =
    Mutex::new(None);

/// Why a request got no suggestions.
enum Failure {
    /// The uniform refusal.
    Unavailable,
    /// The runtime or database permit is busy.
    Busy,
    /// Any other status (an ended session, a server error).
    Status(StatusCode),
}

impl From<AppError> for Failure {
    fn from(error: AppError) -> Self {
        match error {
            AppError::NotFound => Self::Unavailable,
            AppError::AuthStorageBusy => Self::Busy,
            _ => {
                // Nothing about the request (names, query) is logged.
                eprintln!("Mimik: suggestion lookup failed");
                Self::Status(StatusCode::INTERNAL_SERVER_ERROR)
            }
        }
    }
}

fn answer(status: StatusCode, body: &Value) -> Response {
    let mut response = (
        status,
        [(header::CONTENT_TYPE, "application/json")],
        body.to_string(),
    )
        .into_response();
    if status == StatusCode::TOO_MANY_REQUESTS || status == StatusCode::SERVICE_UNAVAILABLE {
        response
            .headers_mut()
            .insert(header::RETRY_AFTER, header::HeaderValue::from_static("1"));
    }
    response.extensions_mut().insert(SheetPage);
    response.extensions_mut().insert(SuggestAnswer);
    response
}

fn failed(failure: &Failure) -> Response {
    match failure {
        Failure::Unavailable => answer(StatusCode::FORBIDDEN, &json!({"status": "unavailable"})),
        Failure::Busy => answer(StatusCode::SERVICE_UNAVAILABLE, &json!({"status": "busy"})),
        Failure::Status(status) => (*status).into_response(),
    }
}

/// The query as the route admits it.
struct Asked {
    q: String,
    typo: bool,
}

/// `q` and `typo` from the raw query: at most 1 KiB, no unknown or repeated parameter, `q`
/// trimmed to 2-64 scalars and 256 bytes, `typo` only as `1`.
fn asked(uri: &Uri) -> Option<Asked> {
    let raw = uri.query().unwrap_or_default();
    if raw.len() > MAX_QUERY_BYTES {
        return None;
    }
    let (mut q, mut typo) = (None, None);
    for (key, value) in form_urlencoded::parse(raw.as_bytes()) {
        let slot = match key.as_ref() {
            "q" => &mut q,
            "typo" => &mut typo,
            _ => return None,
        };
        if slot.replace(value.into_owned()).is_some() {
            return None;
        }
    }
    let q = q?.trim().to_owned();
    let scalars = q.chars().count();
    if !(MIN_SCALARS..=MAX_SCALARS).contains(&scalars) || q.len() > MAX_Q_BYTES {
        return None;
    }
    match typo.as_deref() {
        None => Some(Asked { q, typo: false }),
        Some("1") => Some(Asked { q, typo: true }),
        Some(_) => None,
    }
}

/// The pair and sealed version a lookup read the names of.
#[derive(Clone)]
struct Sealed {
    game: String,
    edition: String,
    version: i64,
}

/// What the first read transaction found.
enum Step {
    Ready(Arc<NameIndex>, Member, Sealed),
    /// The pin's definition is not in the facts cache: its raw row, to validate.
    Miss(private_pin::RawPin),
}

/// The first read transaction (see the module's order).
fn first(
    db: &mut rusqlite::Connection,
    auth: &RequestAuth,
    id: i64,
    (f, p): (usize, usize),
) -> Result<Step, AppError> {
    let tx = db.transaction()?;
    auth.recheck(&tx)?;
    snapshots::authorize_read(&tx, auth, id)?;
    // Overlong identity or pin text reads as NULL (the byte guards): the same uniform refusal.
    let (game, edition, kind, pin): (
        Option<String>,
        Option<String>,
        Option<String>,
        Option<String>,
    ) = tx
        .query_row(
            "SELECT CASE WHEN length(CAST(game_id AS BLOB))<=64 THEN game_id END,CASE WHEN length(CAST(edition_id AS BLOB))<=64 THEN edition_id END,CASE WHEN length(kind)<=7 THEN kind END,CASE WHEN length(CAST(pin AS BLOB))<=4096 THEN pin END FROM private_sheets WHERE id=?1",
            [id],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
        )
        .optional()?
        .ok_or(AppError::NotFound)?;
    let (Some(game), Some(edition), Some(kind)) = (game, edition, kind) else {
        return Err(AppError::NotFound);
    };
    let pin: Pin =
        serde_json::from_str(&pin.ok_or(AppError::NotFound)?).map_err(|_| AppError::NotFound)?;
    let Pin::Runtime {
        version_id,
        version,
        content_hash,
        schema: 1,
    } = pin
    else {
        return Err(AppError::NotFound);
    };
    if kind != "runtime" {
        return Err(AppError::NotFound);
    }
    // The definition row still is the one the sheet pinned, of the sheet's own pair.
    let pinned: bool = tx.query_row(
        "SELECT EXISTS(SELECT 1 FROM definition_versions WHERE id=?1 AND version=?2 AND game_id=?3 AND edition_id=?4 AND content_hash=?5)",
        params![version_id, version, game, edition, content_hash],
        |row| row.get(0),
    )?;
    if !pinned {
        return Err(AppError::NotFound);
    }
    let Some(facts) = cached_facts(&content_hash) else {
        let raw = private_pin::load(&tx, version_id)?;
        auth.recheck(&tx)?;
        tx.commit()?;
        return Ok(Step::Miss(raw));
    };
    let member = facts.member(f, p).ok_or(AppError::NotFound)?.clone();
    let linked =
        read::latest_sealed(&tx, &game, &edition, &member.catalogue)?.ok_or(AppError::NotFound)?;
    let key = (linked.version_id, linked.content_hash.clone());
    let index = if let Some(index) = NAMES.get(&key) {
        index
    } else {
        let index = Arc::new(suggest::load(&tx, linked.version_id)?);
        NAMES.insert(key, index.clone());
        index
    };
    auth.recheck(&tx)?;
    tx.commit()?;
    let sealed = Sealed {
        game,
        edition,
        version: linked.version_id,
    };
    Ok(Step::Ready(index, member, sealed))
}

/// The index and member the request matches against, validating the pin on a facts miss.
async fn lookup(
    store: &Store,
    auth: &RequestAuth,
    id: i64,
    at: (usize, usize),
    mut permit: OwnedSemaphorePermit,
) -> Result<(Arc<NameIndex>, (Member, Sealed), OwnedSemaphorePermit), Failure> {
    for attempt in 0..2 {
        let request = auth.clone();
        let (step, kept) = store
            .run_bounded_auth(move |db| Ok((first(db, &request, id, at)?, permit)))
            .await?;
        permit = kept;
        match step {
            Step::Ready(index, member, sealed) => return Ok((index, (member, sealed), permit)),
            // A second miss (the facts were evicted at once) is busy, not a refusal.
            Step::Miss(_) if attempt == 1 => return Err(Failure::Busy),
            Step::Miss(raw) => {
                let runtime = crate::definitions::admit().map_err(|_| Failure::Busy)?;
                let hash = raw.hash.clone();
                let (validated, kept) = tokio::task::spawn_blocking(move || {
                    let _runtime = runtime;
                    let facts = private_pin::validate_for_write(raw)
                        .map(|pin| Arc::new(Facts::of(&pin)))
                        .map_err(|_| AppError::NotFound);
                    (facts, permit)
                })
                .await
                .map_err(|_| Failure::Status(StatusCode::INTERNAL_SERVER_ERROR))?;
                permit = kept;
                let facts = validated?;
                FACTS
                    .lock()
                    .unwrap_or_else(PoisonError::into_inner)
                    .insert(hash, facts);
            }
        }
    }
    Err(Failure::Busy)
}

/// One suggestion as the JSON answer carries it.
struct Hit {
    name: String,
    key: String,
    kind: String,
    duplicate: bool,
}

/// Matches `q` against `index` outside the database mutex.
fn matched(
    index: &NameIndex,
    member: &Member,
    asked: &Asked,
    norm: &str,
) -> (Vec<Hit>, bool, bool) {
    let query = suggest::Query {
        norm,
        scalars: norm.chars().count(),
        typo: asked.typo,
    };
    let started = std::time::Instant::now();
    let found = suggest::suggest(index, member.entry_type.as_deref(), &query, &mut || {
        started.elapsed()
    });
    let hits = found
        .items
        .iter()
        .map(|found| Hit {
            name: found.entry.name.clone(),
            key: found.entry.key.clone(),
            kind: index.types[found.entry.kind].1.clone(),
            duplicate: found.duplicate,
        })
        .collect();
    (hits, found.more, found.cut)
}

/// The hints of the duplicates among `hits`: the first characters of each entry's context,
/// read in a short transaction of its own; an entry whose version is no longer the latest
/// sealed one has none.
async fn hints(
    store: &Store,
    auth: &RequestAuth,
    id: i64,
    (member, sealed): (Member, Sealed),
    hits: Vec<Hit>,
    permit: OwnedSemaphorePermit,
) -> Result<(Vec<(Hit, String)>, OwnedSemaphorePermit), AppError> {
    let request = auth.clone();
    store
        .run_bounded_auth(move |db| {
            let tx = db.transaction()?;
            request.recheck(&tx)?;
            snapshots::authorize_read(&tx, &request, id)?;
            let linked =
                read::latest_sealed(&tx, &sealed.game, &sealed.edition, &member.catalogue)?
                    .filter(|linked| linked.version_id == sealed.version);
            let budget =
                crate::sources::search::Budget::start(&tx, crate::sources::search::QUERY_BUDGET);
            let mut shown = Vec::with_capacity(hits.len());
            for hit in hits {
                let hint = match (&linked, hit.duplicate) {
                    (Some(linked), true) => {
                        read::entry_context(&tx, linked, &hit.key, read::SUGGESTION_HINT_CHARACTERS)
                            .ok()
                            .flatten()
                            .map(|context| context.text.replace(['\n', '\t'], " "))
                            .unwrap_or_default()
                    }
                    _ => String::new(),
                };
                shown.push((hit, hint));
            }
            drop(budget);
            request.recheck(&tx)?;
            tx.commit()?;
            Ok((shown, permit))
        })
        .await
}

pub(super) async fn suggest(
    State(state): State<AuthState>,
    session: Session,
    Path((id, f, p)): Path<(i64, String, String)>,
    uri: Uri,
    headers: HeaderMap,
) -> Response {
    // Shape first, before the session (all 400).
    let (Some(f), Some(p), true) = (
        values::index(&f),
        values::index(&p),
        sheet_json::wanted(&headers),
    ) else {
        return StatusCode::BAD_REQUEST.into_response();
    };
    let Some(asked) = asked(&uri) else {
        return StatusCode::BAD_REQUEST.into_response();
    };
    let norm = crate::text_fold::name_norm(&asked.q);
    let scalars = norm.chars().count();
    let least = if asked.typo {
        MIN_TYPO_SCALARS
    } else {
        MIN_SCALARS
    };
    if !(least..=MAX_SCALARS).contains(&scalars) {
        return StatusCode::BAD_REQUEST.into_response();
    }
    let Ok(permit) = SUGGEST.clone().try_acquire_owned() else {
        return answer(StatusCode::TOO_MANY_REQUESTS, &json!({"status": "busy"}));
    };
    let auth = match auth::request_auth(&state, &session).await {
        Ok(auth) => auth,
        Err(StatusCode::FORBIDDEN) => return failed(&Failure::Unavailable),
        Err(status) => return status.into_response(),
    };
    match run(&state.store(), auth, id, (f, p), (asked, norm), permit).await {
        Ok(body) => answer(StatusCode::OK, &body),
        Err(failure) => failed(&failure),
    }
}

async fn run(
    store: &Store,
    auth: RequestAuth,
    id: i64,
    at: (usize, usize),
    (asked, norm): (Asked, String),
    permit: OwnedSemaphorePermit,
) -> Result<Value, Failure> {
    let (index, (member, sealed), permit) = lookup(store, &auth, id, at, permit).await?;
    let (matches, permit) = tokio::task::spawn_blocking({
        let (index, member) = (index.clone(), member.clone());
        move || {
            #[cfg(test)]
            {
                let hook = IN_MATCH.lock().unwrap().take();
                if let Some(hook) = hook {
                    hook();
                }
            }
            let found = matched(&index, &member, &asked, &norm);
            (found, permit)
        }
    })
    .await
    .map_err(|_| Failure::Status(StatusCode::INTERNAL_SERVER_ERROR))?;
    let (hits, more, cut) = matches;
    let hits = if hits.iter().any(|hit| hit.duplicate) {
        let (shown, _permit) = hints(store, &auth, id, (member, sealed), hits, permit).await?;
        shown
    } else {
        drop(permit);
        hits.into_iter().map(|hit| (hit, String::new())).collect()
    };
    let suggestions: Vec<Value> = hits
        .into_iter()
        .map(|(hit, hint)| {
            json!({"name": hit.name, "key": hit.key, "type": hit.kind,
                   "duplicate": hit.duplicate, "hint": hint})
        })
        .collect();
    Ok(json!({"status": "ok", "suggestions": suggestions, "more": more, "cut": cut}))
}
