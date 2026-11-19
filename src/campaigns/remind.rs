//! What a reminder sends and when one is offered (assisted sheet plan 3c-1, "Trigger" and
//! "Context"): the trigger on a stored note, and the request built from the snapshot the
//! reminder is bound to. The model never sees booleans, text, multiline values, notes, keys or
//! other properties.
use super::catalogue::EntryLinks;
use super::notes::{NoteValue, Op, Stored};
use crate::assistant::contract::{self, ItemKind, ReminderDecrease, ReminderField, ReminderRow};
use crate::assistant::reminder::{EntryBinding, FieldDraft, MAX_ROWS, ReminderRequest, RowDraft};
use crate::definitions::{Control, Definition};
use serde_json::Value;

/// The decrease a stored note records, when it triggers a reminder: a note of op `literal`,
/// `sum`, `sign_literal`, `switched` or `confirmed` (never `reverted` or `reminded`) on a
/// top-level field address with a known `before` greater than a known `after`. Returns
/// `(before, after)`.
pub(crate) fn trigger(stored: &Stored) -> Option<(i64, i64)> {
    let note = &stored.note;
    if !note.address.starts_with("field:")
        || !matches!(
            note.op,
            Op::Literal | Op::Sum | Op::SignLiteral | Op::Switched | Op::Confirmed
        )
    {
        return None;
    }
    match (&note.before, &note.after) {
        (Some(NoteValue::Int(before)), NoteValue::Int(after)) if before > after => {
            Some((*before, *after))
        }
        _ => None,
    }
}

/// Whether a reminder of the decrease note `stored` was already applied: a current `reminded`
/// note bound to its revision (`target`) with the decrease's own typed text (`raw`, which an
/// apply copies), found among the sheet's current notes. A revision holds one note per
/// address, so a save that decreased several fields at once is told apart by the text; two
/// decreases of one save typed alike are not (`docs/limitations.md`, REMIND-APPLIED).
pub(crate) fn applied<'a>(mut current: impl Iterator<Item = &'a Stored>, stored: &Stored) -> bool {
    current.any(|other| {
        other.note.op == Op::Reminded
            && other.note.target == Some(stored.revision)
            && other.note.raw == stored.note.raw
    })
}

/// The reminder request for the decrease `stored` records on the top-level integer field
/// `id`, from `data` (the snapshot the reminder is bound to): the other stored integer and
/// enum fields in definition order, and the names of the rows of lists with a catalogue member,
/// each with the link `links` gives it (its entry text is attached later, by the route).
/// `None` when `id` is not a stored integer field of the pin.
pub(crate) fn build(
    definition: &Definition,
    data: &Value,
    stored: &Stored,
    (before, after): (i64, i64),
    links: &EntryLinks,
) -> Option<ReminderRequest> {
    let id = stored.note.address.strip_prefix("field:")?;
    let decreased = definition.fields.iter().find(|field| {
        field.id == id && field.control == Control::Integer && field.formula.is_none()
    })?;
    let mut fields = Vec::new();
    for field in &definition.fields {
        let kind = match field.control {
            Control::Integer => ItemKind::Integer,
            Control::Enumeration => ItemKind::Enum,
            _ => continue,
        };
        if field.formula.is_some() || !contract::nameable(&field.id, &decreased.id) {
            continue;
        }
        let value = match (kind, data.get(&field.id)) {
            (ItemKind::Integer, Some(Value::Number(number))) => number.as_i64().map(Value::from),
            (ItemKind::Enum, Some(Value::String(option))) => Some(Value::from(option.as_str())),
            _ => None,
        };
        fields.push(FieldDraft {
            item: ReminderField {
                id: field.id.clone(),
                label: field.label.clone(),
                kind,
                value,
            },
            address: super::notes::field_address(&field.id),
            options: field
                .options
                .iter()
                .flatten()
                .map(|option| option.id.clone())
                .collect(),
        });
    }
    let mut rows = Vec::new();
    for (index, list) in definition
        .fields
        .iter()
        .enumerate()
        .filter(|(_, field)| field.control == Control::List && field.formula.is_none())
    {
        let Some(name) = super::values::properties(list)
            .iter()
            .find(|property| property.catalogue.is_some())
        else {
            continue;
        };
        for item in data
            .get(&list.id)
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
        {
            let (Some(host), Some(text)) = (
                item["id"].as_str(),
                item.get(&name.id).and_then(Value::as_str),
            ) else {
                continue;
            };
            if text.is_empty() || rows.len() >= MAX_ROWS {
                continue;
            }
            rows.push(RowDraft {
                row: ReminderRow {
                    id: format!("r{}", rows.len() + 1),
                    list: list.label.clone(),
                    name: text.to_owned(),
                    entry: None,
                },
                address: super::notes::row_address(&list.id, host),
                entry: links
                    .get(&(index, host.to_owned()))
                    .map(|link| EntryBinding {
                        catalogue: link.catalogue.clone(),
                        version: link.version,
                        key: link.key.clone(),
                        name: String::new(),
                        badge: String::new(),
                        cut: false,
                    }),
            });
        }
    }
    Some(ReminderRequest {
        decrease: ReminderDecrease {
            id: decreased.id.clone(),
            label: decreased.label.clone(),
            before,
            after,
            phrase: stored.note.raw.clone(),
        },
        address: stored.note.address.clone(),
        fields,
        rows,
    })
}
