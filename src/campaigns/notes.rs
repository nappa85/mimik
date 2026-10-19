//! Change notes of runtime sheets (assisted sheet plan §3a, decision D-T; slice 1.2).
//!
//! A note records how one integer, enum or boolean cell got its value in a revision: what
//! the player typed (`raw`), how it was read (`op`), and the typed values before and after.
//! Notes are help, not evidence: they are written in the save's own `IMMEDIATE` transaction
//! right after `snapshots::write`, and the table is insert-only, but they are not sealed
//! (known POC limitation L1, plan §6). Text and multiline cells, Name and General notes get
//! no notes; history already holds every revision of them.
use crate::{
    AppError,
    definitions::{Control, Definition},
};
use rusqlite::{Connection, params};
use serde_json::{Value, json};
use std::collections::BTreeMap;

/// The most notes one revision may carry (plan §3h); a save above it is refused with 422.
pub(crate) const MAX_NOTES: usize = 256;
/// The most bytes of a canonical typed note value.
pub(crate) const MAX_VALUE_BYTES: usize = 80;

/// How a note's value was read. Slice 1.2 writes `Literal`, `Sum` and `Unresolved`; the
/// others are reserved by the table for later slices (1.3 switch and Undo, 1.4 erase,
/// step 2c confirmation).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Op {
    Literal,
    Sum,
    SignLiteral,
    Switched,
    Unresolved,
    Erased,
    Confirmed,
    Reverted,
}

impl Op {
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::Literal => "literal",
            Self::Sum => "sum",
            Self::SignLiteral => "sign_literal",
            Self::Switched => "switched",
            Self::Unresolved => "unresolved",
            Self::Erased => "erased",
            Self::Confirmed => "confirmed",
            Self::Reverted => "reverted",
        }
    }

    pub(crate) fn parse(text: &str) -> Option<Self> {
        [
            Self::Literal,
            Self::Sum,
            Self::SignLiteral,
            Self::Switched,
            Self::Unresolved,
            Self::Erased,
            Self::Confirmed,
            Self::Reverted,
        ]
        .into_iter()
        .find(|op| op.as_str() == text)
    }
}

/// The typed value of a note (plan §3a), stored as canonical JSON of at most
/// [`MAX_VALUE_BYTES`] bytes. Absence (an optional property not set) is distinct from
/// unknown (`null`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum NoteValue {
    Absent,
    Unknown,
    Int(i64),
    Enum(String),
    Bool(bool),
    /// A whole row's state (present or absent), for row removal from slice 1.5b on.
    Row(bool),
}

impl NoteValue {
    /// The canonical JSON text, refused above [`MAX_VALUE_BYTES`].
    pub(crate) fn encode(&self) -> Result<String, AppError> {
        let value = match self {
            Self::Absent => json!({"absent": true}),
            Self::Unknown => json!({"unknown": true}),
            Self::Int(value) => json!({"int": value.to_string()}),
            Self::Enum(id) => json!({"enum": id}),
            Self::Bool(value) => json!({"bool": value}),
            Self::Row(present) => json!({"row": if *present { "present" } else { "absent" }}),
        };
        let text = value.to_string();
        if text.len() > MAX_VALUE_BYTES {
            return Err(AppError::Invalid("A change note value exceeds 80 bytes."));
        }
        Ok(text)
    }

    /// Reads canonical JSON back; anything that does not re-encode to the same text is
    /// `None` (a non-canonical or foreign value is never interpreted).
    pub(crate) fn decode(text: &str) -> Option<Self> {
        if text.len() > MAX_VALUE_BYTES {
            return None;
        }
        let object = serde_json::from_str::<Value>(text).ok()?;
        let object = object.as_object().filter(|object| object.len() == 1)?;
        let (key, value) = object.iter().next()?;
        let decoded = match (key.as_str(), value) {
            ("absent", Value::Bool(true)) => Self::Absent,
            ("unknown", Value::Bool(true)) => Self::Unknown,
            ("int", Value::String(digits)) => Self::Int(super::values::integer(digits).ok()?),
            ("enum", Value::String(id)) => Self::Enum(id.clone()),
            ("bool", Value::Bool(value)) => Self::Bool(*value),
            ("row", Value::String(state)) if state == "present" => Self::Row(true),
            ("row", Value::String(state)) if state == "absent" => Self::Row(false),
            _ => return None,
        };
        (decoded.encode().ok()? == text).then_some(decoded)
    }

    /// The value of a stored cell of `control`; `None` for a text cell or a value of the
    /// wrong type.
    pub(crate) fn of(control: Control, value: &Value) -> Option<Self> {
        match (control, value) {
            (Control::Integer | Control::Enumeration | Control::Boolean, Value::Null) => {
                Some(Self::Unknown)
            }
            (Control::Integer, Value::Number(number)) => number.as_i64().map(Self::Int),
            (Control::Enumeration, Value::String(id)) => Some(Self::Enum(id.clone())),
            (Control::Boolean, Value::Bool(value)) => Some(Self::Bool(*value)),
            _ => None,
        }
    }

    /// The value as shown in history and next to a cell.
    pub(crate) fn text(&self) -> String {
        match self {
            Self::Absent => "not set".into(),
            Self::Unknown => "unknown".into(),
            Self::Int(value) => value.to_string(),
            Self::Enum(id) => id.clone(),
            Self::Bool(value) => value.to_string(),
            Self::Row(true) => "row present".into(),
            Self::Row(false) => "row absent".into(),
        }
    }
}

/// One change note. `before` is `None` when there was no earlier value: on create and in a
/// new row.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Note {
    pub address: String,
    pub op: Op,
    pub raw: String,
    pub before: Option<NoteValue>,
    pub after: NoteValue,
}

/// A note as read back, with the revision it belongs to.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Stored {
    pub revision: i64,
    pub note: Note,
}

/// The notes a page needs: the current note of each live address (sheet page), the notes of
/// one revision (history page), and the notes a save will write.
#[derive(Clone, Debug, Default)]
pub(crate) struct Notes {
    pub current: BTreeMap<String, Stored>,
    pub revision: Vec<Stored>,
    pub pending: Vec<Note>,
}

/// The typed address of a scalar field (plan §3a, Sol rev-2 M1).
pub(crate) fn field_address(id: &str) -> String {
    format!("field:{id}")
}

/// The typed address of a saved row's cell; the data part is `history_diff::address`.
pub(crate) fn cell_address(list: &str, host: &str, property: &str) -> String {
    format!(
        "cell:{}",
        super::history_diff::address(list, host, Some(property))
    )
}

/// The in-scope (integer, enum, boolean) addresses of a stored runtime snapshot.
pub(crate) fn addresses(definition: &Definition, data: &Value) -> Vec<String> {
    let in_scope = |control| {
        matches!(
            control,
            Control::Integer | Control::Enumeration | Control::Boolean
        )
    };
    let mut addresses = Vec::new();
    for field in definition
        .fields
        .iter()
        .filter(|field| field.formula.is_none())
    {
        if field.control == Control::List {
            for item in data[&field.id].as_array().into_iter().flatten() {
                let Some(host) = item["id"].as_str() else {
                    continue;
                };
                for property in super::values::properties(field) {
                    if in_scope(property.control) {
                        addresses.push(cell_address(&field.id, host, &property.id));
                    }
                }
            }
        } else if in_scope(field.control) {
            addresses.push(field_address(&field.id));
        }
    }
    addresses
}

#[cfg(test)]
pub(super) type InsertHook = Box<dyn FnOnce(&mut Vec<Note>) -> Result<(), AppError> + Send>;
/// Test hook: runs inside the save's transaction after `snapshots::write`, before the notes
/// are inserted; it may change the notes or fail.
#[cfg(test)]
pub(super) static BEFORE_INSERT: std::sync::Mutex<Option<InsertHook>> = std::sync::Mutex::new(None);

/// Inserts the notes of the revision just written. The caller owns the save's `IMMEDIATE`
/// transaction, so any failure rolls the whole save back.
pub(crate) fn insert(
    db: &Connection,
    sheet_id: i64,
    revision: i64,
    notes: &[Note],
) -> Result<(), AppError> {
    if notes.len() > MAX_NOTES {
        return Err(too_many());
    }
    let mut statement = db.prepare_cached("INSERT INTO private_sheet_notes(sheet_id,revision,address,op,raw,before,after) VALUES(?1,?2,?3,?4,?5,?6,?7)")?;
    for note in notes {
        let before = note.before.as_ref().map(NoteValue::encode).transpose()?;
        statement.execute(params![
            sheet_id,
            revision,
            note.address,
            note.op.as_str(),
            note.raw,
            before,
            note.after.encode()?,
        ])?;
    }
    Ok(())
}

pub(crate) fn too_many() -> AppError {
    AppError::Invalid(
        "This save would record more than 256 number, choice or yes/no changes at once (a size limit); save fewer changes at a time.",
    )
}

fn stored(row: &rusqlite::Row<'_>) -> rusqlite::Result<Option<Stored>> {
    let revision: i64 = row.get(0)?;
    let address: String = row.get(1)?;
    let op: String = row.get(2)?;
    let raw: String = row.get(3)?;
    let before: Option<String> = row.get(4)?;
    let after: String = row.get(5)?;
    // A malformed row (only possible by writing the database directly, L1) is skipped.
    let note = (|| {
        Some(Note {
            address,
            op: Op::parse(&op)?,
            raw,
            before: match before {
                Some(text) => Some(NoteValue::decode(&text)?),
                None => None,
            },
            after: NoteValue::decode(&after)?,
        })
    })();
    Ok(note.map(|note| Stored { revision, note }))
}

/// The current (latest) note of each of `addresses`.
pub(crate) fn current(
    db: &Connection,
    sheet_id: i64,
    addresses: &[String],
) -> Result<BTreeMap<String, Stored>, AppError> {
    let mut current = BTreeMap::new();
    if addresses.is_empty() {
        return Ok(current);
    }
    let list = serde_json::to_string(addresses)?;
    let mut query = db.prepare("SELECT n.revision,n.address,n.op,n.raw,n.before,n.after FROM json_each(?2) a JOIN private_sheet_notes n ON n.sheet_id=?1 AND n.address=a.value AND n.revision=(SELECT max(m.revision) FROM private_sheet_notes m WHERE m.sheet_id=?1 AND m.address=a.value)")?;
    for row in query.query_map(params![sheet_id, list], stored)? {
        if let Some(stored) = row? {
            current.insert(stored.note.address.clone(), stored);
        }
    }
    Ok(current)
}

/// The notes of one revision, in address order.
pub(crate) fn of_revision(
    db: &Connection,
    sheet_id: i64,
    revision: i64,
) -> Result<Vec<Stored>, AppError> {
    let mut query = db.prepare("SELECT revision,address,op,raw,before,after FROM private_sheet_notes WHERE sheet_id=?1 AND revision=?2 ORDER BY address LIMIT 257")?;
    let rows = query
        .query_map(params![sheet_id, revision], stored)?
        .collect::<Result<Vec<_>, _>>()?;
    Ok(rows.into_iter().flatten().collect())
}

#[cfg(test)]
#[path = "notes_value_tests.rs"]
mod tests;
