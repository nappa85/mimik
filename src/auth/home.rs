//! The home page (`GET /`) and its "mark as seen" action (docs/auth-and-sessions.md, Home page).
//!
//! The Owner's home reads only Owner tables and a player's home only campaign tables (two
//! separate readers, each behind its own admission); both show counts, titles, links and times
//! and never sheet contents or another player's private data. Every section is its own indexed
//! query that fetches at most `SHOWN + 1` rows.
use super::{AuthResult, AuthState, materials, nav};
use crate::{
    AppError, Store,
    auth::RequestAuth,
    presentation::{DisplayTime, datetime},
    sources::materials::{read_text, text},
};
use askama::Template;
use axum::{
    Router,
    extract::{Form, State},
    http::StatusCode,
    response::{Html, IntoResponse, Redirect},
    routing::{get, post},
};
use rusqlite::{Connection, OptionalExtension, TransactionBehavior, params};
use serde::Deserialize;
use tower_sessions::Session;

/// Rows a section shows; its query fetches one more to know whether the list goes on.
const SHOWN: usize = 10;
const FETCH: i64 = 11;
/// Newest suggestions the open-suggestions block looks at.
const SUGGESTION_WINDOW: i64 = 1000;

pub(super) fn routes() -> Router<AuthState> {
    Router::new()
        .route("/", get(landing))
        .route("/home/seen", post(mark))
}

pub(crate) struct Link {
    pub label: String,
    pub href: String,
}

/// One line of a section: a label (linked when `href` is not empty), a note, an optional time
/// and further links.
pub(crate) struct Row {
    pub label: String,
    pub note: String,
    pub href: String,
    /// The item's time; for "new" items it decides the mark.
    pub at: Option<i64>,
    pub links: Vec<Link>,
}

impl Row {
    fn new(label: String, note: String, href: String, at: Option<i64>) -> Self {
        Self {
            label,
            note,
            href,
            at,
            links: Vec::new(),
        }
    }
    pub(crate) fn time(&self) -> Option<DisplayTime> {
        self.at.map(datetime)
    }
}

pub(crate) struct Block {
    pub heading: &'static str,
    pub empty: &'static str,
    /// The page holding the full list.
    pub all: &'static str,
    pub all_label: &'static str,
    pub rows: Vec<Row>,
    /// The list has more rows than shown.
    pub more: bool,
}

impl Block {
    fn new(
        heading: &'static str,
        empty: &'static str,
        all: &'static str,
        all_label: &'static str,
        mut rows: Vec<Row>,
    ) -> Self {
        let more = rows.len() > SHOWN;
        rows.truncate(SHOWN);
        Self {
            heading,
            empty,
            all,
            all_label,
            rows,
            more,
        }
    }
    /// The query looked at a bounded window only, so more may exist than it found.
    fn windowed(mut self, windowed: bool) -> Self {
        self.more |= windowed;
        self
    }
}

pub(crate) struct Home {
    pub pending: Vec<Block>,
    pub news: Vec<Block>,
    /// The account's mark; `None` until the first "Mark as seen".
    pub mark: Option<i64>,
    /// The newest time among the "new" rows shown: what "Mark as seen" moves the mark to.
    pub newest: Option<i64>,
    /// The second this page was read (the request-auth clock): the mark never reaches it, so an
    /// item that appears later in the same second is not hidden.
    pub shown_at: i64,
}

impl Home {
    fn new(mark: Option<i64>, now: i64, pending: Vec<Block>, news: Vec<Block>) -> Self {
        // Clamped to now: a time from another clock (SQLite's `CURRENT_TIMESTAMP`) ahead of the
        // auth clock must not make the mark refuse.
        let newest = news
            .iter()
            .flat_map(|block| &block.rows)
            .filter_map(|row| row.at)
            .max()
            .map(|newest| newest.min(now));
        Self {
            pending,
            news,
            mark,
            newest,
            shown_at: now,
        }
    }
    pub(crate) fn mark_time(&self) -> Option<DisplayTime> {
        self.mark.map(datetime)
    }
}

fn mark_of(db: &Connection, account: i64) -> Result<Option<i64>, AppError> {
    Ok(db
        .query_row(
            "SELECT seen_at FROM account_home_marks WHERE account_id=?1",
            [account],
            |row| row.get(0),
        )
        .optional()?)
}

// ---- Owner ----

fn owner_read(db: &Connection, account: i64, now: i64) -> Result<Home, AppError> {
    let mark = mark_of(db, account)?;
    let since = mark.unwrap_or(0);
    let pending = vec![
        owner_materials(db)?,
        owner_uploads(db)?,
        owner_suggestions(db)?,
        owner_invitations(db, now)?,
        owner_downloads(db)?,
    ];
    let news = vec![
        owner_finished_uploads(db, since)?,
        owner_redeemed(db, since)?,
        owner_reports(db, since)?,
    ];
    Ok(Home::new(mark, now, pending, news))
}

fn owner_materials(db: &Connection) -> Result<Block, AppError> {
    let sql = format!(
        "SELECT s.id,{},
           (SELECT count(*) FROM source_pages p WHERE p.source_id=s.id AND p.complete=0),
           (SELECT count(*) FROM source_pages p WHERE p.source_id=s.id),
           (SELECT p.id FROM source_pages p WHERE p.source_id=s.id AND p.complete=0 ORDER BY p.pdf_page LIMIT 1)
         FROM sources s
         WHERE EXISTS(SELECT 1 FROM source_pages p WHERE p.source_id=s.id AND p.complete=0)
         ORDER BY s.id LIMIT {FETCH}",
        text("s.title", 200)
    );
    let rows = db
        .prepare(&sql)?
        .query_map([], |row| {
            let open: i64 = row.get(2)?;
            let total: i64 = row.get(3)?;
            let page: i64 = row.get(4)?;
            Ok(Row::new(
                read_text(row, 1)?,
                format!("{open} of {total} imported pages not complete"),
                format!("/admin/materials/pages/{page}"),
                None,
            ))
        })?
        .collect::<Result<Vec<_>, _>>()?;
    Ok(Block::new(
        "Materials to review",
        "Every imported page is complete.",
        "/admin/materials",
        "All sources",
        rows,
    ))
}

fn owner_uploads(db: &Connection) -> Result<Block, AppError> {
    let rows = db
        .prepare(&format!(
            "SELECT u.updated_at,u.state,coalesce(j.state,''),coalesce(j.reason,''),u.system_id,u.edition
             FROM source_uploads u LEFT JOIN source_jobs j ON j.upload_id=u.id
             WHERE u.state IN ('rendering','failed') OR j.state IN ('queued','running','failed')
             ORDER BY u.updated_at DESC LIMIT {FETCH}"
        ))?
        .query_map([], |row| {
            let (upload, job, reason): (String, String, String) =
                (row.get(1)?, row.get(2)?, row.get(3)?);
            let failed = upload == "failed" || job == "failed";
            let note = match (failed, reason.is_empty()) {
                (true, true) => "failed".to_owned(),
                (true, false) => format!("failed ({reason})"),
                (false, _) => "running".to_owned(),
            };
            Ok(Row::new(
                format!("{} · {}", row.get::<_, String>(4)?, row.get::<_, String>(5)?),
                note,
                "/admin/materials/uploads".into(),
                Some(row.get(0)?),
            ))
        })?
        .collect::<Result<Vec<_>, _>>()?;
    Ok(Block::new(
        "Uploads running or failed",
        "No upload is running or failed.",
        "/admin/materials/uploads",
        "Uploads",
        rows,
    ))
}

fn owner_suggestions(db: &Connection) -> Result<Block, AppError> {
    // Open means neither dismissed nor applied, which no index on `source_suggestions` can
    // express; the walk is limited to the newest `SUGGESTION_WINDOW` suggestions (each probed
    // by the dismissal and application keys) and the block says when older ones may exist.
    let sql = format!(
        "SELECT s.id,s.kind,{} FROM (SELECT id,kind,source_id FROM source_suggestions ORDER BY id DESC LIMIT {SUGGESTION_WINDOW}) s
         JOIN sources src ON src.id=s.source_id
         WHERE NOT EXISTS(SELECT 1 FROM source_suggestion_dismissals d WHERE d.suggestion_id=s.id)
           AND NOT EXISTS(SELECT 1 FROM source_suggestion_applications a WHERE a.suggestion_id=s.id)
         ORDER BY s.id DESC LIMIT {FETCH}",
        text("src.title", 200)
    );
    let rows = db
        .prepare(&sql)?
        .query_map([], |row| {
            let id: i64 = row.get(0)?;
            let kind: String = row.get(1)?;
            Ok(Row::new(
                read_text(row, 2)?,
                format!("open {kind} suggestion"),
                format!("/admin/materials/suggestions/{id}"),
                None,
            ))
        })?
        .collect::<Result<Vec<_>, _>>()?;
    let older: bool = db.query_row(
        "SELECT EXISTS(SELECT 1 FROM source_suggestions WHERE id<(SELECT min(id) FROM (SELECT id FROM source_suggestions ORDER BY id DESC LIMIT ?1)))",
        [SUGGESTION_WINDOW],
        |row| row.get(0),
    )?;
    Ok(Block::new(
        "Open suggestions",
        "No suggestion is waiting.",
        "/admin/materials",
        "All sources",
        rows,
    )
    .windowed(older))
}

fn owner_invitations(db: &Connection, now: i64) -> Result<Block, AppError> {
    let rows = db
        .prepare(&format!(
            "SELECT a.display_name,c.expires_at FROM account_credentials c JOIN accounts a ON a.id=c.account_id
             WHERE c.state='active' AND c.purpose='enroll' AND c.expires_at>?1
             ORDER BY c.expires_at,c.digest LIMIT {FETCH}"
        ))?
        .query_map([now], |row| {
            Ok(Row::new(
                row.get(0)?,
                "invitation not redeemed, expires".into(),
                String::new(),
                Some(row.get(1)?),
            ))
        })?
        .collect::<Result<Vec<_>, _>>()?;
    Ok(Block::new(
        "Pending invitations",
        "No invitation is waiting.",
        "/admin",
        "Accounts",
        rows,
    ))
}

fn owner_downloads(db: &Connection) -> Result<Block, AppError> {
    let rows = db
        .prepare(&format!(
            "SELECT file_name FROM assistant_downloads WHERE state='confirmed' ORDER BY id LIMIT {FETCH}"
        ))?
        .query_map([], |row| {
            Ok(Row::new(
                row.get(0)?,
                "model download confirmed, not finished".into(),
                String::new(),
                None,
            ))
        })?
        .collect::<Result<Vec<_>, _>>()?;
    Ok(Block::new(
        "Assistant model downloads",
        "No download is waiting.",
        "/admin/assistant",
        "Assistant",
        rows,
    ))
}

fn owner_finished_uploads(db: &Connection, since: i64) -> Result<Block, AppError> {
    let rows = db
        .prepare(&format!(
            "SELECT j.finished_at,j.state,coalesce(j.reason,''),u.system_id,u.edition
             FROM source_jobs j JOIN source_uploads u ON u.id=j.upload_id
             WHERE j.state IN ('succeeded','failed') AND j.finished_at>?1
             ORDER BY j.finished_at DESC,j.upload_id LIMIT {FETCH}"
        ))?
        .query_map([since], |row| {
            let state: String = row.get(1)?;
            let reason: String = row.get(2)?;
            let note = if state == "failed" && !reason.is_empty() {
                format!("upload failed ({reason})")
            } else if state == "failed" {
                "upload failed".to_owned()
            } else {
                "upload finished".to_owned()
            };
            Ok(Row::new(
                format!(
                    "{} · {}",
                    row.get::<_, String>(3)?,
                    row.get::<_, String>(4)?
                ),
                note,
                "/admin/materials/uploads".into(),
                Some(row.get(0)?),
            ))
        })?
        .collect::<Result<Vec<_>, _>>()?;
    Ok(Block::new(
        "Uploads",
        "No upload finished.",
        "/admin/materials/uploads",
        "Uploads",
        rows,
    ))
}

fn owner_redeemed(db: &Connection, since: i64) -> Result<Block, AppError> {
    let rows = db
        .prepare(&format!(
            "SELECT a.display_name,e.occurred_at FROM account_events e JOIN accounts a ON a.id=e.account_id
             WHERE e.action='enroll' AND e.occurred_at>?1
             ORDER BY e.occurred_at DESC,e.id DESC LIMIT {FETCH}"
        ))?
        .query_map([since], |row| {
            Ok(Row::new(
                row.get(0)?,
                "redeemed an invitation".into(),
                String::new(),
                Some(row.get(1)?),
            ))
        })?
        .collect::<Result<Vec<_>, _>>()?;
    Ok(Block::new(
        "Invitations redeemed",
        "No invitation was redeemed.",
        "/admin",
        "Accounts",
        rows,
    ))
}

fn owner_reports(db: &Connection, since: i64) -> Result<Block, AppError> {
    let sql = format!(
        "SELECT r.source_id,{},r.correction+r.reject+r.merge+r.missing_text+r.note,unixepoch(r.imported_at)
         FROM source_suggestion_reports r JOIN sources s ON s.id=r.source_id
         WHERE r.imported_at>datetime(?1,'unixepoch')
         ORDER BY r.imported_at DESC,r.id DESC LIMIT {FETCH}",
        text("s.title", 200)
    );
    let rows = db
        .prepare(&sql)?
        .query_map([since], |row| {
            let source: i64 = row.get(0)?;
            let count: i64 = row.get(2)?;
            Ok(Row::new(
                read_text(row, 1)?,
                format!("{count} suggestions imported"),
                format!("/admin/materials/sources/{source}"),
                Some(row.get(3)?),
            ))
        })?
        .collect::<Result<Vec<_>, _>>()?;
    Ok(Block::new(
        "Suggestions added",
        "No suggestions were added.",
        "/admin/materials",
        "All sources",
        rows,
    ))
}

// ---- Players and Masters ----

fn player_read(db: &Connection, account: i64, now: i64) -> Result<Home, AppError> {
    let mark = mark_of(db, account)?;
    let since = mark.unwrap_or(0);
    let run = player_campaigns(db, account, "master")?;
    let play = player_campaigns(db, account, "player")?;
    let shown: Vec<i64> = run.iter().chain(&play).map(|(id, ..)| *id).collect();
    let mut sheets = own_sheets(db, account, &shown)?;
    let mut campaign_block = |heading, empty, list: Vec<(i64, String, String)>| {
        let rows = list
            .into_iter()
            .map(|(id, title, ruleset)| {
                let mut row = Row::new(title, ruleset, format!("/campaigns/{id}"), None);
                row.links = sheets.remove(&id).unwrap_or_default();
                row
            })
            .collect();
        Block::new(heading, empty, "/campaigns", "All campaigns", rows)
    };
    let is_master = !run.is_empty();
    let mut pending = vec![
        campaign_block("Campaigns I run", "You do not run a campaign.", run),
        campaign_block(
            "Campaigns I play in",
            "You do not play in a campaign yet. Ask a Master to accept you from the campaign list.",
            play,
        ),
        my_requests(db, account)?,
    ];
    if is_master {
        pending.push(requests_to_answer(db, account)?);
    }
    let news = vec![player_events(db, account, since)?];
    Ok(Home::new(mark, now, pending, news))
}

fn player_campaigns(
    db: &Connection,
    account: i64,
    role: &str,
) -> Result<Vec<(i64, String, String)>, AppError> {
    Ok(db
        .prepare(&format!(
            "SELECT c.id,c.title,c.game_id||' / '||c.edition_id FROM campaign_members m JOIN campaigns c ON c.id=m.campaign_id
             WHERE m.account_id=?1 AND m.state='active' AND m.role=?2 ORDER BY c.id LIMIT {FETCH}"
        ))?
        .query_map(params![account, role], |row| {
            Ok((row.get(0)?, row.get(1)?, row.get(2)?))
        })?
        .collect::<Result<Vec<_>, _>>()?)
}

/// Sheets shown per campaign; a further link opens the campaign page.
const SHEETS_SHOWN: usize = 3;

/// The account's own active sheets in the listed campaigns, as links by number: no sheet data,
/// not even its name, is read (the snapshot column is not selected).
fn own_sheets(
    db: &Connection,
    account: i64,
    campaigns: &[i64],
) -> Result<std::collections::HashMap<i64, Vec<Link>>, AppError> {
    let ids = serde_json::to_string(campaigns)?;
    let mut map = std::collections::HashMap::<i64, Vec<Link>>::new();
    let mut query = db.prepare(
        "SELECT id,campaign_id FROM private_sheets
         WHERE custodian_id=?1 AND state='active' AND campaign_id IN (SELECT value FROM json_each(?2))
         ORDER BY campaign_id,id LIMIT 200",
    )?;
    let rows = query.query_map(params![account, ids], |row| {
        Ok((row.get::<_, i64>(0)?, row.get::<_, i64>(1)?))
    })?;
    let mut more = std::collections::HashSet::new();
    for row in rows {
        let (id, campaign) = row?;
        let links = map.entry(campaign).or_default();
        if links.len() < SHEETS_SHOWN {
            links.push(Link {
                label: format!("Sheet #{id}"),
                href: format!("/private-sheets/{id}"),
            });
        } else {
            more.insert(campaign);
        }
    }
    for campaign in more {
        map.entry(campaign).or_default().push(Link {
            label: "more sheets".into(),
            href: format!("/campaigns/{campaign}"),
        });
    }
    Ok(map)
}

fn my_requests(db: &Connection, account: i64) -> Result<Block, AppError> {
    let rows = db
        .prepare(&format!(
            "SELECT c.id,c.title FROM campaign_requests r JOIN campaigns c ON c.id=r.campaign_id
             WHERE r.account_id=?1 AND r.state='pending' ORDER BY r.campaign_id LIMIT {FETCH}"
        ))?
        .query_map([account], |row| {
            let id: i64 = row.get(0)?;
            Ok(Row::new(
                row.get(1)?,
                "waiting for a Master to answer".into(),
                format!("/campaigns/{id}"),
                None,
            ))
        })?
        .collect::<Result<Vec<_>, _>>()?;
    Ok(Block::new(
        "My join requests",
        "No join request of yours is waiting.",
        "/campaigns",
        "All campaigns",
        rows,
    ))
}

fn requests_to_answer(db: &Connection, account: i64) -> Result<Block, AppError> {
    let rows = db
        .prepare(&format!(
            "SELECT r.campaign_id,c.title,a.display_name
             FROM campaign_members m
             JOIN campaign_requests r ON r.campaign_id=m.campaign_id AND r.state='pending'
             JOIN campaigns c ON c.id=r.campaign_id JOIN accounts a ON a.id=r.account_id
             WHERE m.account_id=?1 AND m.state='active' AND m.role='master'
               AND NOT EXISTS(SELECT 1 FROM account_owner o WHERE o.account_id=a.id)
             ORDER BY r.campaign_id,r.account_id LIMIT {FETCH}"
        ))?
        .query_map([account], |row| {
            let id: i64 = row.get(0)?;
            let title: String = row.get(1)?;
            let who: String = row.get(2)?;
            Ok(Row::new(
                title,
                format!("{who} asks to join"),
                format!("/campaigns/{id}"),
                None,
            ))
        })?
        .collect::<Result<Vec<_>, _>>()?;
    Ok(Block::new(
        "Join requests to answer",
        "No join request is waiting for you.",
        "/campaigns",
        "All campaigns",
        rows,
    ))
}

struct Event {
    id: i64,
    at: i64,
    campaign: i64,
    title: String,
    action: String,
    actor: String,
    target: i64,
    target_name: String,
}

fn events(
    db: &Connection,
    sql: &str,
    arguments: impl rusqlite::Params,
) -> Result<Vec<Event>, AppError> {
    Ok(db
        .prepare(sql)?
        .query_map(arguments, |row| {
            Ok(Event {
                id: row.get(0)?,
                at: row.get(1)?,
                campaign: row.get(2)?,
                title: row.get(3)?,
                action: row.get(4)?,
                actor: row.get(5)?,
                target: row.get(6)?,
                target_name: row.get(7)?,
            })
        })?
        .collect::<Result<Vec<_>, _>>()?)
}

/// Events about the account itself, and for a Master the events of the campaigns they run now;
/// the account's own actions are not news to it.
fn player_events(db: &Connection, account: i64, since: i64) -> Result<Block, AppError> {
    let columns = "SELECT e.id,e.occurred_at,e.campaign_id,c.title,e.action,actor.display_name,e.target_id,target.display_name
        FROM campaign_events e JOIN campaigns c ON c.id=e.campaign_id
        JOIN accounts actor ON actor.id=e.actor_id JOIN accounts target ON target.id=e.target_id";
    let own = events(
        db,
        &format!(
            "{columns} WHERE e.target_id=?1 AND e.occurred_at>?2 AND e.actor_id<>?1
             ORDER BY e.occurred_at DESC,e.id DESC LIMIT {FETCH}"
        ),
        params![account, since],
    )?;
    // The campaigns they run now (at most 128 campaigns exist), each read through
    // `campaign_events (campaign_id, occurred_at)` with its own limit.
    let mastered: Vec<i64> = db
        .prepare("SELECT campaign_id FROM campaign_members WHERE account_id=?1 AND state='active' AND role='master' ORDER BY campaign_id LIMIT 129")?
        .query_map([account], |row| row.get(0))?
        .collect::<Result<_, _>>()?;
    let mut all = own;
    for campaign in mastered {
        let run = events(
            db,
            &format!(
                "{columns} WHERE e.campaign_id=?3 AND e.occurred_at>?2 AND e.actor_id<>?1
                 ORDER BY e.occurred_at DESC,e.id DESC LIMIT {FETCH}"
            ),
            params![account, since, campaign],
        )?;
        for event in run {
            if all.iter().all(|seen| seen.id != event.id) {
                all.push(event);
            }
        }
    }
    all.sort_by_key(|event| std::cmp::Reverse((event.at, event.id)));
    let rows = all
        .into_iter()
        .take(SHOWN + 1)
        .map(|event| {
            let note = describe(&event, account);
            Row::new(
                event.title,
                note,
                format!("/campaigns/{}", event.campaign),
                Some(event.at),
            )
        })
        .collect();
    Ok(Block::new(
        "Campaign changes",
        "Nothing changed in your campaigns.",
        "/campaigns",
        "All campaigns",
        rows,
    ))
}

fn describe(event: &Event, account: i64) -> String {
    let whom = if event.target == account {
        "you"
    } else {
        event.target_name.as_str()
    };
    let who = &event.actor;
    match event.action.as_str() {
        "create" => format!("{who} created the campaign"),
        "request" => format!("{who} asked to join"),
        "accept" => format!("{who} accepted {whom}"),
        "decline" => format!("{who} declined {whom}"),
        "promote" => format!("{who} made {whom} a Master"),
        "demote" => format!("{who} made {whom} a player"),
        "leave" => format!("{who} left"),
        "remove" => format!("{who} removed {whom}"),
        _ => format!("{who} changed the campaign"),
    }
}

// ---- Store entry points ----

impl Store {
    /// The Owner's home. Reads Owner tables only; no campaign query runs.
    pub(crate) async fn owner_home(&self, auth: RequestAuth) -> Result<Home, AppError> {
        self.run_bounded_auth(move |db| {
            let tx = db.transaction()?;
            let now = auth.recheck_owner(&tx)?;
            let home = owner_read(&tx, auth.account_id(), now)?;
            auth.recheck_owner(&tx)?;
            tx.commit()?;
            Ok(home)
        })
        .await
    }

    /// A player's (or Master's) home. Reads the account's campaign data only.
    pub(crate) async fn player_home(&self, auth: RequestAuth) -> Result<Home, AppError> {
        self.run_campaign(auth.clone(), move |db| {
            let tx = db.transaction()?;
            let now = auth.recheck(&tx)?;
            let home = player_read(&tx, auth.account_id(), now)?;
            auth.recheck(&tx)?;
            tx.commit()?;
            Ok(home)
        })
        .await
    }

    /// Moves the account's mark towards `seen`, the newest "new" time its page showed, rendered at
    /// `shown_at`. Both are at most now, `seen` is at most the newest new item the home shows
    /// now, and the mark never reaches the second the page was rendered (an item arriving later
    /// in that second stays new). A value at or below the mark changes nothing.
    pub(crate) async fn mark_home_seen(
        &self,
        auth: RequestAuth,
        csrf: crate::auth::CsrfAdmission,
        owner: bool,
        seen: i64,
        shown_at: i64,
    ) -> Result<(), AppError> {
        let admitted = auth.clone();
        let work = move |db: &mut Connection| {
            let tx = db.transaction_with_behavior(TransactionBehavior::Immediate)?;
            let recheck = |tx: &Connection| {
                if owner {
                    csrf.recheck_owner_write(tx, &auth)
                } else {
                    auth.recheck(tx)
                }
            };
            let now = recheck(&tx)?;
            if seen < 1 || seen > now || shown_at < 1 || shown_at > now {
                return Err(AppError::Invalid("The time to mark is not valid."));
            }
            let target = seen.min(shown_at - 1);
            let account = auth.account_id();
            if target < 1 || mark_of(&tx, account)?.is_some_and(|mark| target <= mark) {
                tx.commit()?;
                return Ok(());
            }
            let home = if owner {
                owner_read(&tx, account, now)?
            } else {
                player_read(&tx, account, now)?
            };
            if home.newest.is_none_or(|newest| seen > newest) {
                return Err(AppError::Invalid("Nothing that new is shown on the home."));
            }
            tx.execute(
                "INSERT INTO account_home_marks(account_id,seen_at) VALUES(?1,?2)
                 ON CONFLICT(account_id) DO UPDATE SET seen_at=max(seen_at,excluded.seen_at)",
                params![account, target],
            )?;
            recheck(&tx)?;
            tx.commit()?;
            Ok(())
        };
        if owner {
            self.run_bounded_auth(work).await
        } else {
            self.run_campaign(admitted, work).await
        }
    }
}

// ---- Routes ----

#[derive(Template)]
#[template(path = "home.html")]
struct HomeTemplate {
    nav: nav::Nav,
    home: Home,
    csrf: String,
}

async fn landing(State(state): State<AuthState>, session: Session) -> AuthResult {
    let Some(account) = super::principal(&state, &session).await? else {
        return Ok(Redirect::to("/auth/login").into_response());
    };
    let auth = super::admission(&account)?;
    let store = state.store();
    let home = if account.owner {
        store.owner_home(auth).await
    } else {
        store.player_home(auth).await
    }
    .map_err(|error| materials::error(&error))?;
    let csrf = super::token(&session)
        .await?
        .ok_or(StatusCode::UNAUTHORIZED)?;
    Ok(Html(
        HomeTemplate {
            nav: nav::Nav::here(nav::Section::Home),
            home,
            csrf,
        }
        .render()
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?,
    )
    .into_response())
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct MarkForm {
    csrf: String,
    seen: i64,
    shown_at: i64,
}

async fn mark(
    State(state): State<AuthState>,
    session: Session,
    Form(form): Form<MarkForm>,
) -> AuthResult {
    let csrf = super::csrf_admission(&session, &form.csrf).await?;
    let account = super::principal(&state, &session)
        .await?
        .ok_or(StatusCode::UNAUTHORIZED)?;
    let auth = super::admission(&account)?;
    state
        .store()
        .mark_home_seen(auth, csrf, account.owner, form.seen, form.shown_at)
        .await
        .map_err(|error| materials::error(&error))?;
    Ok(Redirect::to("/").into_response())
}
