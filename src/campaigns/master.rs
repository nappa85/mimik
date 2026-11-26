//! The Master workspace projection (CAMP-14, ROAD-8): every sheet of a campaign for a current
//! Master, in two sections (player sheets and NPC sheets) with this Master's own acknowledged
//! revision. Two Master-only readers read `private_sheets.is_npc`: this projection and the
//! home block of the latest edited sheets; no player-visible query does. The HTML page and the
//! JSON snapshot share the one query below. The guarded sheet name read lives here too, shared
//! by the workspace and the home block.
use super::{Campaign, Listing, campaign, role};
use crate::{AppError, Store, auth::RequestAuth};
use rusqlite::{Connection, params};
use std::collections::HashMap;

/// The sheet name as the Master views read it from `private_sheets s`: only from a snapshot of
/// at most 1 MiB and a name of at most 100 characters, otherwise `NULL`.
const NAME_SQL: &str = "CASE WHEN length(CAST(s.snapshot AS BLOB))<=1048576 THEN CASE WHEN length(json_extract(s.snapshot,'$.name'))<=100 THEN json_extract(s.snapshot,'$.name') END END";
const NAME_UNAVAILABLE: &str = "(name unavailable)";

/// The names of the sheets `ids`, read with the guards above; a sheet that is missing or fails a
/// guard reads "(name unavailable)". Callers pass only the rows they show.
pub(crate) fn sheet_names(db: &Connection, ids: &[i64]) -> Result<HashMap<i64, String>, AppError> {
    let ids = serde_json::to_string(ids)?;
    let mut query = db.prepare(&format!(
        "SELECT j.value,(SELECT {NAME_SQL} FROM private_sheets s WHERE s.id=j.value) FROM json_each(?1) j"
    ))?;
    let rows = query.query_map([ids], |row| {
        Ok((
            row.get::<_, i64>(0)?,
            row.get::<_, Option<String>>(1)?
                .unwrap_or_else(|| NAME_UNAVAILABLE.into()),
        ))
    })?;
    Ok(rows.collect::<Result<_, _>>()?)
}

/// The textual badge for a Master's acknowledged revision against the live one: never colour
/// alone.
pub(crate) fn badge_of(seen: Option<i64>, revision: i64) -> &'static str {
    match seen {
        None => "Not opened",
        Some(seen) if seen < revision => "Changed",
        Some(_) => "Opened",
    }
}

/// The two keyset cursors of the workspace page, nonnegative like [`super::domain::DetailPage`].
#[derive(Clone, Copy, Default, serde::Deserialize)]
#[serde(default, deny_unknown_fields)]
pub(crate) struct MasterPage {
    #[serde(rename = "players_after")]
    pub players: i64,
    #[serde(rename = "npcs_after")]
    pub npcs: i64,
}

impl MasterPage {
    pub(crate) fn valid(self) -> bool {
        self.players >= 0 && self.npcs >= 0
    }
}

/// One sheet row of the workspace: only display labels and revision numbers, no sheet values.
/// The section a row appears in is its classification.
pub(crate) struct MasterSheet {
    pub id: i64,
    pub name: String,
    pub custodian: String,
    pub revision: i64,
    pub state: String,
    /// The NPC classification: the section the row was read for.
    pub npc: bool,
    /// This Master's acknowledged revision; `None` when they never opened the sheet.
    pub seen: Option<i64>,
}

impl MasterSheet {
    /// The textual badge: never colour alone.
    pub(crate) fn badge(&self) -> &'static str {
        badge_of(self.seen, self.revision)
    }
}

pub(crate) struct MasterWorkspace {
    pub campaign: Campaign,
    pub players: Listing<MasterSheet>,
    pub npcs: Listing<MasterSheet>,
}

impl Store {
    /// The workspace of campaign `id` for the requesting current Master; anyone else (the
    /// Owner, a player, a former Master, another campaign's Master) is refused alike, without
    /// any sheet metadata. Authorised again at the transaction's final boundary.
    pub(crate) async fn master_workspace(
        &self,
        auth: RequestAuth,
        id: i64,
        page: MasterPage,
    ) -> Result<MasterWorkspace, AppError> {
        if !page.valid() {
            return Err(AppError::Invalid("Cursors must be nonnegative."));
        }
        self.run_campaign(auth.clone(), move |db| read_workspace(db, &auth, id, page))
            .await
    }
}

/// The one projection: both selected pages in a single read transaction that rechecks the
/// session binding and the current Master role at its start and at its final boundary.
pub(super) fn read_workspace(
    db: &mut rusqlite::Connection,
    auth: &RequestAuth,
    id: i64,
    page: MasterPage,
) -> Result<MasterWorkspace, AppError> {
    let tx = db.transaction()?;
    auth.recheck(&tx)?;
    let campaign = campaign(&tx, id)?;
    if role(&tx, id, auth.account_id())?.as_deref() != Some("master") {
        return Err(AppError::NotFound);
    }
    let section = |npc: bool, after: i64| -> Result<Vec<MasterSheet>, AppError> {
        let mut q = tx.prepare(&format!("SELECT s.id,{NAME_SQL},a.display_name,s.revision,CASE WHEN length(s.state)<=6 THEN s.state END,m.seen_revision FROM private_sheets s JOIN accounts a ON a.id=s.custodian_id LEFT JOIN master_sheet_seen m ON m.master_id=?1 AND m.sheet_id=s.id WHERE s.campaign_id=?2 AND s.is_npc=?3 AND s.id>?4 ORDER BY s.id LIMIT 51"))?;
        let rows = q.query_map(params![auth.account_id(), id, npc, after], |r| {
            Ok(MasterSheet {
                id: r.get(0)?,
                name: r
                    .get::<_, Option<String>>(1)?
                    .unwrap_or_else(|| NAME_UNAVAILABLE.into()),
                custodian: r.get(2)?,
                revision: r.get(3)?,
                state: r
                    .get::<_, Option<String>>(4)?
                    .unwrap_or_else(|| "unknown".into()),
                npc,
                seen: r.get(5)?,
            })
        })?;
        Ok(rows.collect::<Result<Vec<_>, _>>()?)
    };
    let players = section(false, page.players)?;
    let npcs = section(true, page.npcs)?;
    // The final boundary: still a current Master after the reads.
    auth.recheck(&tx)?;
    if role(&tx, id, auth.account_id())?.as_deref() != Some("master") {
        return Err(AppError::NotFound);
    }
    tx.commit()?;
    Ok(MasterWorkspace {
        campaign,
        players: Listing::new(players, |row| row.id),
        npcs: Listing::new(npcs, |row| row.id),
    })
}

/// Largest JSON row, and largest snapshot body, of the Master snapshot.
const MAX_ROW_BYTES: usize = 2048;
const MAX_BODY_BYTES: usize = 256 * 1024;
/// Visible rows of one snapshot: two sections of one page (50) each.
const MAX_ROWS: usize = 100;

#[derive(serde::Serialize)]
struct JsonRow<'a> {
    id: String,
    name: &'a str,
    custodian: &'a str,
    revision: String,
    state: &'a str,
    npc: bool,
    seen: Option<String>,
}

#[derive(serde::Serialize)]
struct JsonSection<'a> {
    rows: Vec<JsonRow<'a>>,
    next: Option<String>,
}

#[derive(serde::Serialize)]
struct JsonSnapshot<'a> {
    players: JsonSection<'a>,
    npcs: JsonSection<'a>,
}

impl MasterWorkspace {
    /// The snapshot JSON of the selected pages, IDs and revisions as decimal strings (JavaScript
    /// loses i64 precision). `None` when a row exceeds 2 KiB, the body 256 KiB or the rows 100:
    /// the caller answers a visible error and never truncates.
    pub(crate) fn snapshot_json(&self) -> Option<String> {
        self.snapshot_json_within(MAX_BODY_BYTES)
    }

    /// [`Self::snapshot_json`] with an explicit body limit (the 100 rows of at most 2 KiB reach
    /// 200 KiB, so the 256 KiB production limit is defence in depth; tests lower it).
    pub(crate) fn snapshot_json_within(&self, max_body: usize) -> Option<String> {
        fn section(listing: &Listing<MasterSheet>) -> Option<JsonSection<'_>> {
            let rows = listing
                .rows
                .iter()
                .map(|row| JsonRow {
                    id: row.id.to_string(),
                    name: &row.name,
                    custodian: &row.custodian,
                    revision: row.revision.to_string(),
                    state: &row.state,
                    npc: row.npc,
                    seen: row.seen.map(|seen| seen.to_string()),
                })
                .collect::<Vec<_>>();
            for row in &rows {
                if serde_json::to_vec(row).ok()?.len() > MAX_ROW_BYTES {
                    return None;
                }
            }
            Some(JsonSection {
                rows,
                next: listing.next.map(|next| next.to_string()),
            })
        }
        let (players, npcs) = (section(&self.players)?, section(&self.npcs)?);
        if players.rows.len() + npcs.rows.len() > MAX_ROWS {
            return None;
        }
        let body = serde_json::to_string(&JsonSnapshot { players, npcs }).ok()?;
        (body.len() <= max_body).then_some(body)
    }
}
