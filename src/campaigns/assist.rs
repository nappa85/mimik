//! Switch and Undo on runtime sheets (assisted sheet plan §3a D-Q, §3f D-N; slice 1.3).
//!
//! Both act on the **current note** of one typed address and are bound to what the page
//! showed: the switch only while the cell's `sign_literal` note is still current and the
//! cell still holds the value it wrote; Undo only for the current (last) change of the cell.
//! The writer rechecks that binding inside the save's `IMMEDIATE` transaction
//! ([`super::notes::Notes::bound`]). Arithmetic is checked `i64`.
use super::notes::{Note, NoteValue, Op, Stored};
use crate::{
    AppError,
    definitions::{Control, Definition, Field, ItemProperty},
};
use serde_json::Value;

/// Where an in-scope (integer, enum, boolean) address lives in a stored runtime snapshot.
pub(crate) struct Located<'a> {
    /// The cell's label as the page names it ("Current HP", "Quantity, row 2 of Equipment").
    pub label: String,
    cell: Cell<'a>,
}

enum Cell<'a> {
    Field(&'a Field),
    Row {
        list: &'a Field,
        index: usize,
        property: &'a ItemProperty,
    },
}

/// The longest typed address (the note table's bound).
const MAX_ADDRESS_BYTES: usize = 256;

fn in_scope(control: Control) -> bool {
    matches!(
        control,
        Control::Integer | Control::Enumeration | Control::Boolean
    )
}

/// Locates a `field:<id>` or `cell:<list>/<host>/<property>` address among the editable
/// in-scope cells of `data`: a non-derived integer, enum or boolean field, or such a
/// property of a live row. Anything else (text, derived, unknown IDs or rows, metadata) is
/// `None`.
pub(crate) fn locate<'a>(
    definition: &'a Definition,
    data: &Value,
    address: &str,
) -> Option<Located<'a>> {
    let editable = |id: &str| {
        definition
            .fields
            .iter()
            .find(|field| field.id == id && field.formula.is_none())
    };
    if let Some(id) = address.strip_prefix("field:") {
        let field = editable(id).filter(|field| in_scope(field.control))?;
        return Some(Located {
            label: field.label.clone(),
            cell: Cell::Field(field),
        });
    }
    let path = address.strip_prefix("cell:")?;
    let mut parts = path.splitn(3, '/');
    let (list_id, host, property_id) = (parts.next()?, parts.next()?, parts.next()?);
    let list = editable(list_id).filter(|field| field.control == Control::List)?;
    let property = super::values::properties(list)
        .iter()
        .find(|property| property.id == property_id && in_scope(property.control))?;
    // The address must be exactly the canonical one (no `/` smuggled into a part).
    if super::notes::cell_address(list_id, host, property_id) != address {
        return None;
    }
    let index = data[&list.id]
        .as_array()?
        .iter()
        .position(|item| item["id"].as_str() == Some(host))?;
    Some(Located {
        label: format!("{}, row {} of {}", property.label, index + 1, list.label),
        cell: Cell::Row {
            list,
            index,
            property,
        },
    })
}

/// The typed value at a located address: `Absent` for an optional property that is not set.
pub(crate) fn value_at(located: &Located<'_>, data: &Value) -> Option<NoteValue> {
    match located.cell {
        Cell::Field(field) => NoteValue::of(field.control, data.get(&field.id)?),
        Cell::Row {
            list,
            index,
            property,
        } => match data[&list.id][index].get(&property.id) {
            None if property.optional => Some(NoteValue::Absent),
            None => None,
            Some(value) => NoteValue::of(property.control, value),
        },
    }
}

/// Writes `value` at a located address: a number, unknown (`null`), an enum ID or a
/// boolean, or `Absent` (an optional property is removed). Schema validation of the result
/// is the writer's (`snapshots::write`).
pub(crate) fn set(
    located: &Located<'_>,
    data: &mut Value,
    value: &NoteValue,
) -> Result<(), AppError> {
    let refused = || AppError::Invalid("This value cannot be written to this cell.");
    let stored = match value {
        NoteValue::Unknown => Some(Value::Null),
        NoteValue::Int(number) => Some(Value::from(*number)),
        NoteValue::Enum(id) => Some(Value::String(id.clone())),
        NoteValue::Bool(flag) => Some(Value::Bool(*flag)),
        NoteValue::Absent => None,
        NoteValue::Row(_) => return Err(refused()),
    };
    match located.cell {
        Cell::Field(field) => {
            data[&field.id] = stored.ok_or_else(refused)?;
        }
        Cell::Row {
            list,
            index,
            property,
        } => {
            let item = data[&list.id][index].as_object_mut().ok_or_else(refused)?;
            match stored {
                Some(stored) => {
                    item.insert(property.id.clone(), stored);
                }
                None if property.optional => {
                    item.remove(&property.id);
                }
                None => return Err(refused()),
            }
        }
    }
    Ok(())
}

/// The form value naming a stored note: `<revision>:<address>`.
pub(crate) fn key(stored: &Stored) -> String {
    format!("{}:{}", stored.revision, stored.note.address)
}

/// Parses a note key: a canonical positive revision, then a `field:` or `cell:` address of
/// at most 256 bytes with a non-empty rest.
pub(crate) fn parse_key(raw: &str) -> Option<(i64, &str)> {
    let (revision, address) = raw.split_once(':')?;
    let revision = super::values::integer(revision).ok().filter(|r| *r > 0)?;
    let typed = ["field:", "cell:"].iter().any(|prefix| {
        address
            .strip_prefix(prefix)
            .is_some_and(|rest| !rest.is_empty())
    });
    (typed && address.len() <= MAX_ADDRESS_BYTES).then_some((revision, address))
}

/// The value the D-Q switch writes, b + n, for a current `sign_literal` note over the known
/// number b whose cell still holds n (the note's `after`, which its raw text must spell);
/// `None` when there is nothing to offer: another op, no known number before, a newer
/// value, an overflow (checked `i64`), or b = 0 (the relative reading is the literal).
pub(crate) fn switch_value(stored: &Stored, current: Option<&NoteValue>) -> Option<i64> {
    let note = &stored.note;
    let (Op::SignLiteral, Some(NoteValue::Int(before)), NoteValue::Int(after)) =
        (note.op, note.before.as_ref(), &note.after)
    else {
        return None;
    };
    let reading = super::entry::read(&note.raw).ok()?;
    if current != Some(&note.after) || !reading.signed || reading.value != *after {
        return None;
    }
    let value = before.checked_add(*after)?;
    (value != *after).then_some(value)
}

/// Whether the current note of a cell can be undone (§3f, D-N, D-X1): a change with a
/// recorded value before that differs from its value after, with the cell still holding
/// that value after. Scribbles, new cells and row notes (slice 1.5b) are not undoable.
pub(crate) fn undoable(stored: &Stored, current: Option<&NoteValue>) -> bool {
    let note = &stored.note;
    matches!(
        note.op,
        Op::Literal
            | Op::Sum
            | Op::SignLiteral
            | Op::Switched
            | Op::Confirmed
            | Op::Reverted
            | Op::Erased
    ) && note
        .before
        .as_ref()
        .is_some_and(|before| before != &note.after && !matches!(before, NoteValue::Row(_)))
        && !matches!(note.after, NoteValue::Row(_))
        && current == Some(&note.after)
}

/// The note a switch writes: what was typed, from the literal to b + n.
pub(crate) fn switch_note(stored: &Stored, value: i64) -> Note {
    Note {
        address: stored.note.address.clone(),
        op: Op::Switched,
        raw: stored.note.raw.clone(),
        before: Some(stored.note.after.clone()),
        after: NoteValue::Int(value),
        target: None,
    }
}

/// The note an Undo writes: back to the undone note's value before, naming its revision.
/// Call only for an [`undoable`] note.
pub(crate) fn undo_note(stored: &Stored) -> Note {
    Note {
        address: stored.note.address.clone(),
        op: Op::Reverted,
        raw: String::new(),
        before: Some(stored.note.after.clone()),
        after: stored
            .note
            .before
            .clone()
            .unwrap_or_else(|| stored.note.after.clone()),
        target: Some(stored.revision),
    }
}

#[cfg(test)]
#[path = "assist_unit_tests.rs"]
mod tests;
