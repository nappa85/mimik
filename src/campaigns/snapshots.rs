use super::{name_notes, role};
use crate::{AppError, auth::RequestAuth};
use rusqlite::{Connection, OptionalExtension, params};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

pub(crate) const MAX_BYTES: usize = 1024 * 1024;

#[derive(Clone, Deserialize, Serialize, PartialEq, Eq)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub(crate) enum Pin {
    Native {
        adapter: String,
        schema: u32,
        calculation_version: u32,
    },
    Runtime {
        version_id: i64,
        version: i64,
        content_hash: String,
        schema: u32,
    },
}

#[derive(Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct PrivateSnapshot {
    pub encoding: u32,
    pub sheet_id: i64,
    pub campaign_id: i64,
    pub game_id: String,
    pub edition_id: String,
    pub custodian_id: i64,
    pub kind: String,
    pub pin: Pin,
    pub revision: i64,
    pub state: String,
    pub name: String,
    pub notes: String,
    pub data: serde_json::Value,
}

pub(crate) fn hash(bytes: &[u8]) -> String {
    let mut h = Sha256::new();
    h.update(b"mimik-private-sheet-v1\0");
    h.update(bytes);
    hex::encode(h.finalize())
}

fn serialize(snapshot: &PrivateSnapshot) -> Result<String, AppError> {
    let bytes = serde_json::to_string(snapshot)?;
    if bytes.len() > MAX_BYTES {
        return Err(AppError::Invalid("Private snapshot exceeds 1 MiB."));
    }
    Ok(bytes)
}

/// The writer's encoding: within 1 MiB **and** reopenable by the bounded sheet decoder
/// (node, key and element caps), so a written snapshot can always be loaded and frozen.
fn encode(snapshot: &PrivateSnapshot) -> Result<String, AppError> {
    let bytes = serialize(snapshot)?;
    crate::definitions::decode::parse_sheet(bytes.as_bytes())
        .map_err(|_| AppError::Invalid("Private snapshot exceeds the sheet decoder limits."))?;
    Ok(bytes)
}

/// Host-managed list item IDs (plan D3). Every item ID absent from the previous revision's
/// list (every ID on create) must be `r<revision>-<k>`, with `revision` the one being
/// written and `k` numbering that list's newly allocated rows 1, 2, … in row order. Retained
/// IDs keep their slot; a removed ID can never return because `revision` only grows.
fn host_item_ids(
    definition: &crate::definitions::Definition,
    previous: Option<&serde_json::Value>,
    data: &serde_json::Value,
    revision: i64,
) -> Result<(), AppError> {
    for field in definition
        .fields
        .iter()
        .filter(|field| field.control == crate::definitions::Control::List)
    {
        let retained: std::collections::HashSet<&str> = previous
            .and_then(|previous| previous[&field.id].as_array())
            .into_iter()
            .flatten()
            .filter_map(|item| item["id"].as_str())
            .collect();
        let mut allocated = 0_usize;
        for item in data[&field.id].as_array().into_iter().flatten() {
            let id = item["id"].as_str().unwrap_or_default();
            if retained.contains(id) {
                continue;
            }
            allocated += 1;
            if id != format!("r{revision}-{allocated}") {
                return Err(AppError::Invalid(
                    "New list rows must carry host-assigned IDs.",
                ));
            }
        }
    }
    Ok(())
}

pub(crate) fn supported(snapshot: &PrivateSnapshot) -> bool {
    match &snapshot.pin {
        Pin::Native {
            adapter,
            schema,
            calculation_version,
        } => {
            snapshot.kind == "native"
                && adapter == "mork-borg-it-2025"
                && *schema == 1
                && *calculation_version == 1
        }
        Pin::Runtime { schema, .. } => snapshot.kind == "runtime" && *schema == 1,
    }
}

fn native(snapshot: &PrivateSnapshot) -> Result<(), AppError> {
    if snapshot.kind != "native" {
        return Ok(());
    }
    if native_data_v1(snapshot) && snapshot.data["revision"].as_i64() != Some(snapshot.revision) {
        return Err(AppError::Invalid(
            "Native schema1 mirrored revision mismatch.",
        ));
    }
    match &snapshot.pin {
        Pin::Native {
            adapter,
            schema,
            calculation_version,
        } if adapter == "mork-borg-it-2025" && *schema == 1 && *calculation_version == 1 => {
            if snapshot.game_id != "mork-borg" || snapshot.edition_id != "it-2025" {
                return Err(AppError::Invalid("Native pin identity mismatch."));
            }
            let sheet: crate::mork_borg::Sheet = serde_json::from_value(snapshot.data.clone())?;
            sheet.validate()?;
            if sheet.revision != snapshot.revision || serde_json::to_value(sheet)? != snapshot.data
            {
                return Err(AppError::Invalid("Native canonical data mismatch."));
            }
        }
        _ => {}
    }
    Ok(())
}

fn native_data_v1(snapshot: &PrivateSnapshot) -> bool {
    matches!(&snapshot.pin,Pin::Native{adapter,schema:1,..} if adapter=="mork-borg-it-2025")
        && snapshot.kind == "native"
        && snapshot.game_id == "mork-borg"
        && snapshot.edition_id == "it-2025"
}

pub(crate) fn decode(content: &str, expected: &str) -> Result<PrivateSnapshot, AppError> {
    let value = crate::definitions::decode::parse_sheet(content.as_bytes())
        .map_err(|_| AppError::Invalid("Corrupt bounded private snapshot."))?;
    let snapshot: PrivateSnapshot = serde_json::from_value(value)?;
    if snapshot.encoding != 1
        || !matches!(
            (snapshot.kind.as_str(), &snapshot.pin),
            ("native", Pin::Native { .. }) | ("runtime", Pin::Runtime { .. })
        )
        || snapshot.sheet_id <= 0
        || snapshot.campaign_id <= 0
        || snapshot.custodian_id <= 0
        || snapshot.revision <= 0
        || !matches!(snapshot.state.as_str(), "active" | "frozen")
        || (snapshot.state == "active" && snapshot.revision == i64::MAX)
        || name_notes(&snapshot.name, &snapshot.notes)? != snapshot.name
        || serialize(&snapshot)? != content
        || hash(content.as_bytes()) != expected
    {
        return Err(AppError::Invalid("Private snapshot integrity error."));
    }
    native(&snapshot)?;
    Ok(snapshot)
}

pub(crate) struct RawSnapshot {
    pub identity: PrivateSnapshot,
    content: String,
    hash: String,
    pin: String,
}

impl RawSnapshot {
    pub(crate) fn decode(self) -> Result<PrivateSnapshot, AppError> {
        let snapshot = decode(&self.content, &self.hash)?;
        if !self.identity_matches(&snapshot)? {
            return Err(AppError::Invalid("Live/history identity mismatch."));
        }
        Ok(snapshot)
    }

    fn identity_matches(&self, snapshot: &PrivateSnapshot) -> Result<bool, AppError> {
        let identity = &self.identity;
        Ok(!(snapshot.sheet_id != identity.sheet_id
            || snapshot.campaign_id != identity.campaign_id
            || snapshot.custodian_id != identity.custodian_id
            || snapshot.game_id != identity.game_id
            || snapshot.edition_id != identity.edition_id
            || snapshot.kind != identity.kind
            || snapshot.revision != identity.revision
            || snapshot.state != identity.state
            || serde_json::to_string(&snapshot.pin)? != self.pin))
    }

    pub(crate) fn corresponds(
        &self,
        snapshot: &PrivateSnapshot,
        content: &str,
        hash: &str,
    ) -> Result<bool, AppError> {
        Ok(self.content == content && self.hash == hash && self.identity_matches(snapshot)?)
    }
}

/// Project one bounded full payload; metadata and correspondence come from the same SQL snapshot.
pub(crate) fn raw_load(
    db: &Connection,
    id: i64,
    history: Option<i64>,
) -> Result<RawSnapshot, AppError> {
    let (content,hash,pin,campaign,custodian,game,edition,kind,revision,state):(Option<String>,String,String,i64,i64,String,String,String,i64,String)=db.query_row("SELECT CASE WHEN length(CAST(h.snapshot AS BLOB))<=1048576 THEN h.snapshot END,CASE WHEN length(CAST(h.hash AS BLOB))=64 THEN h.hash END,CASE WHEN length(CAST(s.pin AS BLOB))<=4096 THEN s.pin END,s.campaign_id,s.custodian_id,CASE WHEN length(CAST(s.game_id AS BLOB))<=64 THEN s.game_id END,CASE WHEN length(CAST(s.edition_id AS BLOB))<=64 THEN s.edition_id END,CASE WHEN length(s.kind)<=7 THEN s.kind END,h.revision,CASE h.operation WHEN 'freeze' THEN 'frozen' ELSE 'active' END FROM private_sheets s JOIN private_sheet_history h ON h.sheet_id=s.id AND h.revision=coalesce(?2,s.revision) WHERE s.id=?1 AND EXISTS(SELECT 1 FROM private_sheet_events_valid current WHERE current.sheet_id=s.id AND current.revision=s.revision AND current.hash=s.hash AND current.snapshot=s.snapshot AND json_extract(current.snapshot,'$.state')=s.state)",params![id,history],|r|Ok((r.get(0)?,r.get(1)?,r.get(2)?,r.get(3)?,r.get(4)?,r.get(5)?,r.get(6)?,r.get(7)?,r.get(8)?,r.get(9)?))).optional()?.ok_or(AppError::NotFound)?;
    let content = content.ok_or(AppError::Invalid("Stored private snapshot exceeds limit."))?;
    if !event_matches(db, id, revision, &hash, &content)? {
        return Err(AppError::Invalid("History event integrity error."));
    }
    let typed_pin: Pin = serde_json::from_str(&pin)?;
    let identity = PrivateSnapshot {
        encoding: 1,
        sheet_id: id,
        campaign_id: campaign,
        game_id: game,
        edition_id: edition,
        custodian_id: custodian,
        kind,
        pin: typed_pin,
        revision,
        state,
        name: String::new(),
        notes: String::new(),
        data: serde_json::Value::Null,
    };
    Ok(RawSnapshot {
        identity,
        content,
        hash,
        pin,
    })
}

pub(crate) fn load(db: &Connection, id: i64) -> Result<PrivateSnapshot, AppError> {
    raw_load(db, id, None)?.decode()
}

pub(crate) fn event_matches(
    db: &Connection,
    id: i64,
    revision: i64,
    hash: &str,
    content: &str,
) -> Result<bool, AppError> {
    Ok(db.query_row("SELECT EXISTS(SELECT 1 FROM private_sheet_events_valid WHERE sheet_id=?1 AND revision=?2 AND hash=?3 AND snapshot=?4)",params![id,revision,hash,content],|r|r.get(0))?)
}

pub(crate) fn authorize_read(db: &Connection, auth: &RequestAuth, id: i64) -> Result<(), AppError> {
    let identity: Option<(i64, i64)> = db
        .query_row(
            "SELECT campaign_id,custodian_id FROM private_sheets WHERE id=?1",
            [id],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .optional()?;
    let (campaign, custodian) = identity.ok_or(AppError::NotFound)?;
    if custodian != auth.account_id()
        && role(db, campaign, auth.account_id())?.as_deref() != Some("master")
    {
        return Err(AppError::NotFound);
    }
    Ok(())
}

pub(crate) fn authorize_edit(
    db: &Connection,
    auth: &RequestAuth,
    snapshot: &PrivateSnapshot,
) -> Result<(), AppError> {
    let role = role(db, snapshot.campaign_id, auth.account_id())?;
    let supervision:bool=db.query_row("SELECT EXISTS(SELECT 1 FROM campaign_members m JOIN accounts a ON a.id=m.account_id WHERE m.campaign_id=?1 AND m.role='master' AND m.state='active' AND a.state='enabled' AND NOT EXISTS(SELECT 1 FROM account_owner o WHERE o.account_id=a.id))",params![snapshot.campaign_id],|r|r.get(0))?;
    if snapshot.state != "active"
        || !supported(snapshot)
        || !supervision
        || !(role.as_deref() == Some("master")
            || (snapshot.custodian_id == auth.account_id() && role.is_some()))
    {
        return Err(AppError::NotFound);
    }
    Ok(())
}

/// The sole private live/history writer. Caller owns an immediate transaction.
pub(crate) fn write(
    db: &Connection,
    auth: &RequestAuth,
    mut resulting: PrivateSnapshot,
    operation: &str,
    admitted: Option<&crate::definitions::private_pin::ImmutablePin>,
) -> Result<PrivateSnapshot, AppError> {
    if operation != "freeze" && !supported(&resulting) {
        return Err(AppError::Invalid(
            "Unsupported interpretation pin is read-only.",
        ));
    }
    if resulting.kind == "runtime" && supported(&resulting) {
        let admitted = admitted.ok_or(AppError::Invalid(
            "Runtime writer requires an admitted immutable pin.",
        ))?;
        super::sheets::match_pin(&resulting, admitted)?;
        admitted.unchanged(db)?;
        admitted.validate_values(&resulting.data)?;
    }
    let previous = if operation == "create" {
        0
    } else {
        resulting.revision
    };
    if !matches!(operation, "create" | "edit" | "freeze") {
        return Err(AppError::Invalid("Invalid private operation."));
    }
    let mut previous_data = None;
    if previous > 0 {
        let old = load(db, resulting.sheet_id)?;
        if old.revision != previous {
            return Err(AppError::Conflict);
        }
        if old.state != "active"
            || old.campaign_id != resulting.campaign_id
            || old.custodian_id != resulting.custodian_id
            || old.pin != resulting.pin
            || old.game_id != resulting.game_id
            || old.edition_id != resulting.edition_id
            || old.kind != resulting.kind
        {
            return Err(AppError::Invalid("Immutable private identity changed."));
        }
        if operation == "freeze"
            && (old.name != resulting.name
                || old.notes != resulting.notes
                || old.data != resulting.data)
        {
            return Err(AppError::Invalid("Freeze must preserve values."));
        }
        previous_data = Some(old.data);
    }
    resulting.revision = previous.checked_add(1).ok_or(AppError::Conflict)?;
    // After the revision-conflict check, so a stale write answers 409 whatever its IDs.
    // Freezing already requires identical data.
    if operation != "freeze"
        && let Some(admitted) = admitted.filter(|_| resulting.kind == "runtime")
    {
        host_item_ids(
            admitted.definition(),
            previous_data.as_ref(),
            &resulting.data,
            resulting.revision,
        )?;
    }
    resulting.state = if operation == "freeze" {
        "frozen"
    } else {
        "active"
    }
    .into();
    if resulting.state == "active" && resulting.revision == i64::MAX {
        return Err(AppError::Conflict);
    }
    resulting.name = name_notes(&resulting.name, &resulting.notes)?;
    if native_data_v1(&resulting) {
        resulting.data["revision"] = resulting.revision.into();
    }
    native(&resulting)?;
    if resulting.state == "active" {
        let mut reserve = resulting.clone();
        reserve.revision = i64::MAX;
        reserve.state = "frozen".into();
        if native_data_v1(&reserve) {
            reserve.data["revision"] = i64::MAX.into();
        }
        encode(&reserve)?;
    }
    let content = encode(&resulting)?;
    let hash = hash(content.as_bytes());
    let pin = serde_json::to_string(&resulting.pin)?;
    let now = auth.recheck(db)?;
    if operation=="create"{
        db.execute("INSERT INTO private_sheets(id,campaign_id,custodian_id,game_id,edition_id,kind,pin,revision,state,snapshot,hash) VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11)",params![resulting.sheet_id,resulting.campaign_id,resulting.custodian_id,resulting.game_id,resulting.edition_id,resulting.kind,pin,resulting.revision,resulting.state,content,hash])?;
    }else if db.execute("UPDATE private_sheets SET revision=?3,state=?4,snapshot=?5,hash=?6 WHERE id=?1 AND revision=?2",params![resulting.sheet_id,previous,resulting.revision,resulting.state,content,hash])?!=1{return Err(AppError::Conflict);}
    if db.execute("INSERT INTO private_sheet_history(sheet_id,revision,hash,snapshot,operation,previous_revision,actor_id,actor_epoch,session_digest,occurred_at) VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9,?10)",params![resulting.sheet_id,resulting.revision,hash,content,operation,previous,auth.account_id(),auth.epoch(),auth.digest(),now])?!=1{return Err(AppError::Conflict);}
    auth.recheck(db)?;
    Ok(resulting)
}
