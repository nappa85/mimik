//! Switch and Undo on runtime sheets (assisted sheet plan §3a D-Q, §3f D-N; slice 1.3).
//!
//! Both act on the **current note** of one typed address and are bound to what the page
//! showed: the switch only while the cell's `sign_literal` note is still current and the
//! cell still holds the value it wrote; Undo only for the current (last) change of the cell.
//! The writer rechecks that binding inside the save's `IMMEDIATE` transaction
//! ([`super::notes::Notes::bound`]). Arithmetic is checked `i64`.
//!
//! Slice 1.5b adds the whole-row address `row:<list>/<host>`: removing a row writes its note
//! there, and Undo re-inserts the row from the revision before the removal (D-V, §3f).
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
    /// A whole row by its host ID, present or not (slice 1.5b).
    Line {
        list: &'a Field,
        host: String,
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

/// What an address names in a pinned definition, before the live data is consulted.
enum Target<'a, 's> {
    Field(&'a Field),
    Row {
        list: &'a Field,
        host: &'s str,
        property: &'a ItemProperty,
    },
    Line {
        list: &'a Field,
        host: &'s str,
    },
}

/// Parses a `field:<id>`, `cell:<list>/<host>/<property>` or `row:<list>/<host>` address
/// against the pin: a non-derived integer, enum or boolean field, such a property of a
/// list, or a row of a list, with a non-empty host ID and the exact canonical spelling.
fn target<'a, 's>(definition: &'a Definition, address: &'s str) -> Option<Target<'a, 's>> {
    let editable = |id: &str| {
        definition
            .fields
            .iter()
            .find(|field| field.id == id && field.formula.is_none())
    };
    if let Some(id) = address.strip_prefix("field:") {
        return editable(id)
            .filter(|field| in_scope(field.control))
            .map(Target::Field);
    }
    if let Some(path) = address.strip_prefix("row:") {
        let (list_id, host) = path.split_once('/')?;
        let list = editable(list_id).filter(|field| field.control == Control::List)?;
        return (crate::registry::valid_id(host)
            && super::notes::row_address(list_id, host) == address)
            .then_some(Target::Line { list, host });
    }
    let path = address.strip_prefix("cell:")?;
    let mut parts = path.splitn(3, '/');
    let (list_id, host, property_id) = (parts.next()?, parts.next()?, parts.next()?);
    let list = editable(list_id).filter(|field| field.control == Control::List)?;
    let property = super::values::properties(list)
        .iter()
        .find(|property| property.id == property_id && in_scope(property.control))?;
    // The address must be exactly the canonical one (no `/` smuggled into a part).
    if host.is_empty() || super::notes::cell_address(list_id, host, property_id) != address {
        return None;
    }
    Some(Target::Row {
        list,
        host,
        property,
    })
}

/// Whether `address` names an in-scope cell of the pinned definition, whether or not its
/// row is still on the sheet (a removed row's address is stale, not malformed).
pub(crate) fn in_pin(definition: &Definition, address: &str) -> bool {
    target(definition, address).is_some()
}

/// Locates an address among the editable in-scope cells of `data`: [`in_pin`], and for a
/// row cell a live row with that host ID. Anything else (text, derived, unknown IDs or
/// rows, metadata) is `None`.
pub(crate) fn locate<'a>(
    definition: &'a Definition,
    data: &Value,
    address: &str,
) -> Option<Located<'a>> {
    match target(definition, address)? {
        Target::Field(field) => Some(Located {
            label: field.label.clone(),
            cell: Cell::Field(field),
        }),
        Target::Row {
            list,
            host,
            property,
        } => {
            let index = data[&list.id]
                .as_array()?
                .iter()
                .position(|item| item["id"].as_str() == Some(host))?;
            Some(row(list, index, property))
        }
        Target::Line { list, host } => Some(Located {
            label: format!("the row removed from {}", list.label),
            cell: Cell::Line {
                list,
                host: host.to_owned(),
            },
        }),
    }
}

/// How a message names the cell at `address`: its label as the page shows it when it is
/// on the sheet, else (a row removed, or restored under a new host ID) its property and
/// list. Used where a refusal lists the members of a bundle that changed.
pub(crate) fn label(definition: &Definition, data: &Value, address: &str) -> String {
    if let Some(located) = locate(definition, data, address) {
        return located.label;
    }
    match target(definition, address) {
        Some(Target::Row { list, property, .. }) => {
            format!("{} of a removed row of {}", property.label, list.label)
        }
        _ => "A field".to_owned(),
    }
}

impl Located<'_> {
    /// The list and host ID of a whole-row address (`row:`), else `None`.
    pub(crate) fn line(&self) -> Option<(&Field, &str)> {
        match &self.cell {
            Cell::Line { list, host } => Some((list, host)),
            _ => None,
        }
    }
}

/// List field `f` (not derived) and its in-scope property `p`: the row cell an indexed note
/// key or route names, before any row is looked up.
pub(crate) fn row_property(
    definition: &Definition,
    (f, p): (usize, usize),
) -> Option<(&Field, &ItemProperty)> {
    let list = definition
        .fields
        .get(f)
        .filter(|field| field.control == Control::List && field.formula.is_none())?;
    let property = super::values::properties(list).get(p)?;
    Some((list, property))
}

/// The location of property `property` in row `index` of list `list`, when the caller
/// already knows the row (rendering a saved list), without searching for it.
pub(crate) fn row<'a>(list: &'a Field, index: usize, property: &'a ItemProperty) -> Located<'a> {
    Located {
        label: format!("{}, row {} of {}", property.label, index + 1, list.label),
        cell: Cell::Row {
            list,
            index,
            property,
        },
    }
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
        Cell::Line { list, ref host } => Some(NoteValue::Row(
            data[&list.id]
                .as_array()?
                .iter()
                .any(|item| item["id"].as_str() == Some(host)),
        )),
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
        // A row comes back through [`restore`], never as a cell value.
        Cell::Line { .. } => return Err(refused()),
    }
    Ok(())
}

/// The form value naming a stored note: `<revision>:<address>`.
pub(crate) fn key(stored: &Stored) -> String {
    format!("{}:{}", stored.revision, stored.note.address)
}

/// What a note key names after its revision.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Named<'a> {
    /// A typed address (`field:`, `cell:` or `row:`).
    Address(&'a str),
    /// A row cell by indices `<f>.<r>.<p>` (slice 1.5b): list field `f`, row `r` of the
    /// revision the page was read at (its `base`), property `p`. A sheet page names row cells
    /// this way, so a switch or Undo button does not repeat three 64-character IDs (plan D7).
    Indexed { f: usize, r: usize, p: usize },
}

/// Parses a note key: a canonical positive revision, then a `field:`, `cell:` or `row:`
/// address of at most 256 bytes with a non-empty rest, or a row cell's canonical indices.
pub(crate) fn parse_key(raw: &str) -> Option<(i64, Named<'_>)> {
    let (revision, address) = raw.split_once(':')?;
    let revision = super::values::integer(revision).ok().filter(|r| *r > 0)?;
    let typed = ["field:", "cell:", "row:"].iter().any(|prefix| {
        address
            .strip_prefix(prefix)
            .is_some_and(|rest| !rest.is_empty())
    });
    if typed {
        return (address.len() <= MAX_ADDRESS_BYTES).then_some((revision, Named::Address(address)));
    }
    let mut parts = address.split('.').map(super::values::index);
    let (f, r, p) = (parts.next()??, parts.next()??, parts.next()??);
    parts
        .next()
        .is_none()
        .then_some((revision, Named::Indexed { f, r, p }))
}

/// The page key of a row cell: `<revision>:<f>.<r>.<p>` ([`Named::Indexed`]).
pub(crate) fn indexed_key(revision: i64, (f, r, p): (usize, usize, usize)) -> String {
    format!("{revision}:{f}.{r}.{p}")
}

/// The typed address of row cell `(f, r, p)` in `data` (the revision a page was read at):
/// list field `f` (not derived), its row `r`, its in-scope property `p`; `None` otherwise.
pub(crate) fn indexed_address(
    definition: &Definition,
    data: &Value,
    (f, r, p): (usize, usize, usize),
) -> Option<String> {
    let (list, property) =
        row_property(definition, (f, p)).filter(|(_, property)| in_scope(property.control))?;
    let host = data[&list.id].as_array()?.get(r)?["id"].as_str()?;
    Some(super::notes::cell_address(&list.id, host, &property.id))
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
/// that value after. Scribbles, new cells and row notes are not ([`removal_undoable`]).
pub(crate) fn undoable(stored: &Stored, current: Option<&NoteValue>) -> bool {
    let note = &stored.note;
    matches!(
        note.op,
        Op::Literal
            | Op::Sum
            | Op::SignLiteral
            | Op::Switched
            | Op::Confirmed
            | Op::Reminded
            | Op::Reverted
            | Op::Erased
    ) && note
        .before
        .as_ref()
        .is_some_and(|before| before != &note.after && !matches!(before, NoteValue::Row(_)))
        && !matches!(note.after, NoteValue::Row(_))
        && current == Some(&note.after)
}

/// Whether a row note offers Undo (D-V, §3f): a removal (`row present` → `row absent`)
/// whose row is still absent (`current`). The Undo that restored it is not undoable: the
/// row came back as a new row, which Remove removes.
pub(crate) fn removal_undoable(stored: &Stored, current: Option<&NoteValue>) -> bool {
    let note = &stored.note;
    note.op == Op::Literal
        && note.before == Some(NoteValue::Row(true))
        && note.after == NoteValue::Row(false)
        && current == Some(&NoteValue::Row(false))
}

/// The note a row removal writes on the row's address (D-V).
pub(crate) fn removal_note(list: &str, host: &str) -> Note {
    Note {
        address: super::notes::row_address(list, host),
        op: Op::Literal,
        raw: String::new(),
        before: Some(NoteValue::Row(true)),
        after: NoteValue::Row(false),
        target: None,
    }
}

/// Undo of a removal (§3f): the row `host` as it was in `before` (the revision before the
/// removal) goes back into `data` at its old index, clamped to the current length, with the
/// new host ID `r<next>-1` (removed IDs never return, `snapshots.rs`). A full list is
/// refused with a message; a row missing from `before` is `None` (nothing to restore).
pub(crate) fn restore(
    list: &Field,
    host: &str,
    before: &Value,
    data: &mut Value,
    next: i64,
) -> Option<Result<(), AppError>> {
    let (index, item) = before[&list.id]
        .as_array()?
        .iter()
        .enumerate()
        .find(|(_, item)| item["id"].as_str() == Some(host))?;
    let rows = data[&list.id].as_array_mut()?;
    if rows.len() >= crate::definitions::formula::MAX_LIST_ITEMS {
        return Some(Err(AppError::Invalid(
            "This list already holds 256 rows, the most a list can hold; remove a row first, then Undo.",
        )));
    }
    let mut item = item.clone();
    item["id"] = Value::String(format!("r{next}-1"));
    rows.insert(index.min(rows.len()), item);
    Some(Ok(()))
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
