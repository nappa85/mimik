//! List units (assisted sheet slices 1.5b and 1.6; `docs/assisted-entry.md`, D-C, D-S, D-V):
//! a saved row's cell, the new-row form, the quick-add line, and Remove, Move up and Move
//! down of one row, each saved on its own.
//!
//! A page names a row by its index at the revision it was read at (its `base`), so the
//! routes stay short (plan D7). Every plan first maps that index to the row's host ID in
//! `base`, then finds the host in the live revision L, and applies the change there:
//! - a cell protects its value and its row's existence (a row missing at `base` is 400; one
//!   removed since is a conflict that keeps the draft);
//! - the new-row form protects nothing (it only appends);
//! - Remove protects the row's existence and every property value (a removed row is "No
//!   change"; an edited one a conflict);
//! - Move protects the list's order of host IDs;
//! - the quick-add line (slice 1.6), like the new-row form, protects nothing.
//!
//! Values are read exactly as the full form reads them (`values::row_cell`, `row_item`).
use super::{
    notes::{Note, Stored},
    snapshots::PrivateSnapshot,
    unit::{Planned, RowAction},
    values,
};
use crate::{
    AppError,
    definitions::{Field, private_pin::ImmutablePin},
};
use axum::http::StatusCode;
use serde_json::Value;
use std::collections::BTreeMap;

/// The most notes one new-row form records: one per integer, enum or boolean property, and a
/// list has at most 32 properties (the definition decoder's cap).
pub(crate) const MAX_ROW_NOTES: usize = 32;

/// A list unit's plan and what its answer needs.
pub(crate) struct Plan {
    pub planned: Planned,
    /// The element a successful save scrolls back to.
    pub anchor: String,
    /// The host ID of a cell's row, so a refused draft is shown in that row (or, when the
    /// row is gone, as an orphaned draft).
    pub host: Option<String>,
    /// After a move, the Move button the page focuses (`?focus=`; an element fragment would
    /// block `autofocus`).
    pub focus: Option<String>,
}

impl Plan {
    fn new(planned: Planned, anchor: String) -> Self {
        Self {
            planned,
            anchor,
            host: None,
            focus: None,
        }
    }
}

fn items<'a>(data: &'a Value, list: &Field) -> &'a [Value] {
    data[&list.id].as_array().map_or(&[], Vec::as_slice)
}

fn host_at<'a>(data: &'a Value, list: &Field, r: usize) -> Option<&'a str> {
    items(data, list).get(r)?["id"].as_str()
}

/// The host ID of row `r` of list field `f` in `snapshot`.
pub(crate) fn host(
    pin: &ImmutablePin,
    snapshot: &PrivateSnapshot,
    (f, r): (usize, usize),
) -> Option<String> {
    host_at(&snapshot.data, &pin.definition().fields[f], r).map(str::to_owned)
}

/// The index of row `host` of list field `f` in `snapshot`.
pub(crate) fn host_index(
    pin: &ImmutablePin,
    snapshot: &PrivateSnapshot,
    f: usize,
    host: &str,
) -> Option<usize> {
    index_of(&snapshot.data, &pin.definition().fields[f], host)
}

fn index_of(data: &Value, list: &Field, host: &str) -> Option<usize> {
    items(data, list)
        .iter()
        .position(|item| item["id"].as_str() == Some(host))
}

fn order<'a>(data: &'a Value, list: &Field) -> Vec<Option<&'a str>> {
    items(data, list)
        .iter()
        .map(|item| item["id"].as_str())
        .collect()
}

/// A refused value or a refusal of the writer's checks.
fn refusal(failure: AppError) -> Result<Planned, StatusCode> {
    match failure {
        AppError::Invalid(reason) => Ok(Planned::Invalid(reason)),
        failure => Err(super::web::error(failure)),
    }
}

/// Cell `p` of row `r` (at `base`) of list field `f`, saved from its form (`value`, plus
/// `set` for an optional property) into `snapshot` (L). Order as for a field (slice 1.5a,
/// P5): a row missing at `base` is 400; a row removed since is a conflict that keeps the
/// draft; a submission equal to the saved value is "No change"; a value that differs between `base` and L is a conflict; then a
/// refused value is 422.
pub(crate) fn cell(
    pin: &ImmutablePin,
    (snapshot, base): (&mut PrivateSnapshot, &PrivateSnapshot),
    (f, r, p): (usize, usize, usize),
    (form, current): (&BTreeMap<String, String>, &BTreeMap<String, Stored>),
    pending: &mut Vec<Note>,
) -> Result<Plan, StatusCode> {
    let list = &pin.definition().fields[f];
    // The catalogue key property is never a control: its cell takes no value (step 3a).
    if values::key_property(list) == Some(p) {
        return Err(StatusCode::BAD_REQUEST);
    }
    let property = &values::properties(list)[p];
    let host = host_at(&base.data, list, r)
        .ok_or(StatusCode::BAD_REQUEST)?
        .to_owned();
    // A row removed since `base` takes no save; its draft is kept, never moved to the row now
    // at that index (review round 1).
    let Some(index) = index_of(&snapshot.data, list, &host) else {
        return Ok(Plan {
            planned: Planned::Conflict,
            anchor: format!("list-legend-{f}"),
            host: Some(host),
            focus: None,
        });
    };
    let saved = &items(&snapshot.data, list)[index];
    let stale = items(&base.data, list)[r].get(&property.id) != saved.get(&property.id);
    let mut recorder = values::Recorder::new(current);
    let read = values::row_cell(
        (
            super::notes::cell_address(&list.id, &host, &property.id),
            property,
        ),
        (&form["value"], form.get("set").map_or("", String::as_str)),
        values::Prior::of_item(Some(saved), &property.id),
        &mut recorder,
    );
    let patched = read.and_then(|value| {
        let mut data = snapshot.data.clone();
        let item = data[&list.id][index]
            .as_object_mut()
            .ok_or(AppError::Invalid("Invalid runtime sheet data."))?;
        match value {
            Some(value) => item.insert(property.id.clone(), value),
            None => item.remove(&property.id),
        };
        pin.validate_values(&data)?;
        Ok(data)
    });
    let planned = match patched {
        Ok(data) if data == snapshot.data && recorder.notes.is_empty() => Planned::Unchanged,
        _ if stale => Planned::Conflict,
        Ok(data) => {
            snapshot.data = data;
            *pending = recorder.notes;
            Planned::Write
        }
        Err(failure) => refusal(failure)?,
    };
    Ok(Plan {
        planned,
        anchor: format!("label-{f}-{index}-{p}"),
        host: Some(host),
        focus: None,
    })
}

/// The new row of a new-row form for `list` (its `row-<p>` and `set-<p>` pairs) as the
/// item `host` and its notes: a required number left blank is unknown, an unreadable one
/// unknown with its scribble (D-M), a required enum or yes/no must be chosen. At most
/// [`MAX_ROW_NOTES`] notes, else refused.
pub(crate) fn new_item(
    list: &Field,
    host: &str,
    form: &BTreeMap<String, String>,
    current: &BTreeMap<String, Stored>,
) -> Result<(Value, Vec<Note>), AppError> {
    let properties = values::properties(list);
    let pair = |key: String| form.get(&key).cloned().unwrap_or_default();
    let row = values::DraftRow {
        id: String::new(),
        values: (0..properties.len())
            .map(|p| pair(format!("row-{p}")))
            .collect(),
        states: properties
            .iter()
            .enumerate()
            .map(|(p, property)| {
                if property.optional {
                    pair(format!("set-{p}"))
                } else {
                    String::new()
                }
            })
            .collect(),
    };
    let mut recorder = values::Recorder::new(current);
    let item = values::row_item(list, host, &row, None, &mut recorder)?;
    if recorder.notes.len() > MAX_ROW_NOTES {
        return Err(AppError::Invalid(
            "This row would record more than 32 number, choice or yes/no notes at once (a size limit).",
        ));
    }
    Ok((item, recorder.notes))
}

/// The new-row form of list field `f`: one row appended to L with the host ID
/// `r<L+1>-1` (plan D3), refused at 256 rows. It protects nothing, so it is never stale.
pub(crate) fn new_row(
    pin: &ImmutablePin,
    snapshot: &mut PrivateSnapshot,
    f: usize,
    (form, current): (&BTreeMap<String, String>, &BTreeMap<String, Stored>),
    pending: &mut Vec<Note>,
) -> Result<Plan, StatusCode> {
    let list = &pin.definition().fields[f];
    let rows = items(&snapshot.data, list).len();
    let anchor = format!("row-{f}-{rows}");
    if rows >= crate::definitions::formula::MAX_LIST_ITEMS {
        return Ok(Plan::new(
            Planned::Invalid("This list already holds 256 rows, the most a list can hold."),
            anchor,
        ));
    }
    let Some(next) = snapshot.revision.checked_add(1) else {
        return Ok(Plan::new(
            Planned::Invalid("Sheet revision exhausted."),
            anchor,
        ));
    };
    let added = new_item(list, &format!("r{next}-1"), form, current).and_then(|(item, notes)| {
        let mut data = snapshot.data.clone();
        data[&list.id]
            .as_array_mut()
            .ok_or(AppError::Invalid("Invalid runtime sheet data."))?
            .push(item);
        pin.validate_values(&data)?;
        Ok((data, notes))
    });
    let planned = match added {
        Ok((data, notes)) => {
            snapshot.data = data;
            *pending = notes;
            Planned::Write
        }
        Err(failure) => refusal(failure)?,
    };
    Ok(Plan::new(planned, anchor))
}

/// The quick-add line of list field `f` (slice 1.6, ASSIST-3): `text` read by
/// `quick::read` and appended to L as one row with the host ID `r<L+1>-1`, like the new-row
/// form (append only, never stale). The mapped name and count are always filled, even when
/// optional; every other required property is an integer (admission) and unknown; every
/// other optional property is not set. No note. A blank line is "Nothing to add"; a full
/// list is refused before the line is read.
pub(crate) fn quick_add(
    pin: &ImmutablePin,
    snapshot: &mut PrivateSnapshot,
    f: usize,
    text: &str,
) -> Result<Plan, StatusCode> {
    let list = &pin.definition().fields[f];
    let quick = list
        .items
        .as_ref()
        .and_then(|items| items.quick_add.as_ref())
        .ok_or(StatusCode::BAD_REQUEST)?;
    let rows = items(&snapshot.data, list).len();
    let anchor = format!("quick-{f}");
    if rows >= crate::definitions::formula::MAX_LIST_ITEMS {
        return Ok(Plan::new(
            Planned::Invalid("This list already holds 256 rows, the most a list can hold."),
            anchor,
        ));
    }
    let item = match super::quick::read(text, quick.quantity.is_some()) {
        Ok(Some(item)) => item,
        Ok(None) => return Ok(Plan::new(Planned::Unchanged, anchor)),
        Err(reason) => return Ok(Plan::new(Planned::Invalid(reason), anchor)),
    };
    let Some(next) = snapshot.revision.checked_add(1) else {
        return Ok(Plan::new(
            Planned::Invalid("Sheet revision exhausted."),
            anchor,
        ));
    };
    let mut row = serde_json::Map::new();
    row.insert("id".into(), Value::String(format!("r{next}-1")));
    for property in values::properties(list) {
        let value = if property.id == quick.name {
            Value::String(item.name.to_owned())
        } else if Some(&property.id) == quick.quantity.as_ref() {
            item.quantity.map_or(Value::Null, Value::from)
        } else if property.optional {
            continue;
        } else {
            // Admission leaves only integers here: unknown (D-M).
            Value::Null
        };
        row.insert(property.id.clone(), value);
    }
    let mut data = snapshot.data.clone();
    rows_of(&mut data, list)?.push(Value::Object(row));
    let planned = match pin.validate_values(&data) {
        Ok(()) => {
            snapshot.data = data;
            Planned::Write
        }
        Err(failure) => refusal(failure)?,
    };
    Ok(Plan {
        // The line again, focused, for the next item.
        focus: matches!(planned, Planned::Write).then(|| format!("quick-{f}")),
        ..Plan::new(planned, anchor)
    })
}

/// Remove, Move up or Move down of row `r` (at `base`) of list field `f`, applied to L.
/// Remove protects the row's values: a row gone from L is "No change" (it is already
/// removed), an edited one a conflict; it writes the row's removal note (D-V). A move
/// protects the order of host IDs; moving the first row up or the last down is 400.
pub(crate) fn action(
    pin: &ImmutablePin,
    (snapshot, base): (&mut PrivateSnapshot, &PrivateSnapshot),
    (f, r, action): (usize, usize, RowAction),
    pending: &mut Vec<Note>,
) -> Result<Plan, StatusCode> {
    let list = &pin.definition().fields[f];
    let host = host_at(&base.data, list, r)
        .ok_or(StatusCode::BAD_REQUEST)?
        .to_owned();
    let mut data = snapshot.data.clone();
    let (planned, anchor) = match action {
        RowAction::Remove => {
            let anchor = format!("removed-{f}");
            let Some(index) = index_of(&snapshot.data, list, &host) else {
                return Ok(Plan::new(Planned::Unchanged, anchor));
            };
            if items(&base.data, list)[r] != items(&snapshot.data, list)[index] {
                return Ok(Plan::new(Planned::Conflict, anchor));
            }
            rows_of(&mut data, list)?.remove(index);
            *pending = vec![super::assist::removal_note(&list.id, &host)];
            (Planned::Write, anchor)
        }
        RowAction::Up | RowAction::Down => {
            let to = match action {
                RowAction::Up => r.checked_sub(1),
                _ => Some(r + 1).filter(|to| *to < items(&base.data, list).len()),
            }
            .ok_or(StatusCode::BAD_REQUEST)?;
            if order(&base.data, list) != order(&snapshot.data, list) {
                return Ok(Plan::new(Planned::Conflict, format!("row-{f}-{r}")));
            }
            rows_of(&mut data, list)?.swap(r, to);
            (Planned::Write, format!("row-{f}-{to}"))
        }
    };
    if let Err(failure) = pin.validate_values(&data) {
        return Ok(Plan::new(refusal(failure)?, anchor));
    }
    // The moved row's Move button in the same direction, or at an edge the other one (the
    // full form's focus continuation, plan D5).
    let rows = items(&data, list).len();
    let focus = match action {
        RowAction::Remove => None,
        RowAction::Up if r == 1 => Some(format!("down-{f}-0")),
        RowAction::Up => Some(format!("up-{f}-{}", r - 1)),
        RowAction::Down if r + 2 == rows => Some(format!("up-{f}-{}", r + 1)),
        RowAction::Down => Some(format!("down-{f}-{}", r + 1)),
    };
    snapshot.data = data;
    Ok(Plan {
        focus,
        ..Plan::new(planned, anchor)
    })
}

fn rows_of<'a>(data: &'a mut Value, list: &Field) -> Result<&'a mut Vec<Value>, StatusCode> {
    data[&list.id]
        .as_array_mut()
        .ok_or(StatusCode::INTERNAL_SERVER_ERROR)
}

#[cfg(test)]
#[path = "rows_tests.rs"]
mod tests;
