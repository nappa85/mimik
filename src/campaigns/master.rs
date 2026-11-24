//! The Master workspace projection (CAMP-14, ROAD-8): every sheet of a campaign for a current
//! Master, in two sections (player sheets and NPC sheets) with this Master's own acknowledged
//! revision. Only this projection reads `private_sheets.is_npc`; no player-visible query does.
use super::{Campaign, Listing, campaign, role};
use crate::{AppError, Store, auth::RequestAuth};
use rusqlite::params;

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
    /// This Master's acknowledged revision; `None` when they never opened the sheet.
    pub seen: Option<i64>,
}

impl MasterSheet {
    /// The textual badge: never colour alone.
    pub(crate) fn badge(&self) -> &'static str {
        match self.seen {
            None => "Not opened",
            Some(seen) if seen < self.revision => "Changed",
            Some(_) => "Opened",
        }
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
        self.run_campaign(auth.clone(), move |db| {
            let tx = db.transaction()?;
            auth.recheck(&tx)?;
            let campaign = campaign(&tx, id)?;
            if role(&tx, id, auth.account_id())?.as_deref() != Some("master") {
                return Err(AppError::NotFound);
            }
            let section = |npc: bool, after: i64| -> Result<Vec<MasterSheet>, AppError> {
                let mut q = tx.prepare("SELECT s.id,CASE WHEN length(CAST(s.snapshot AS BLOB))<=1048576 THEN CASE WHEN length(json_extract(s.snapshot,'$.name'))<=100 THEN json_extract(s.snapshot,'$.name') END END,a.display_name,s.revision,CASE WHEN length(s.state)<=6 THEN s.state END,m.seen_revision FROM private_sheets s JOIN accounts a ON a.id=s.custodian_id LEFT JOIN master_sheet_seen m ON m.master_id=?1 AND m.sheet_id=s.id WHERE s.campaign_id=?2 AND s.is_npc=?3 AND s.id>?4 ORDER BY s.id LIMIT 51")?;
                let rows = q.query_map(params![auth.account_id(), id, npc, after], |r| {
                    Ok(MasterSheet {
                        id: r.get(0)?,
                        name: r.get::<_, Option<String>>(1)?.unwrap_or_else(|| "(name unavailable)".into()),
                        custodian: r.get(2)?,
                        revision: r.get(3)?,
                        state: r.get::<_, Option<String>>(4)?.unwrap_or_else(|| "unknown".into()),
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
        })
        .await
    }
}
