//! What an interpretation job sends (assisted sheet plan §1.4, "Input", "Context" and
//! "Related fields"): the phrase of the target's current `unresolved` note, the target, and
//! context items from the snapshot the job is bound to.
//!
//! - Scalars: the other stored integer and enum fields, in definition order (D-F, D-J);
//!   with B at most 1,024 bytes only those some read set contains together with the target
//!   (for a row cell, with its property), and none without a formula layer (ASSIST-31,
//!   ASSIST-37).
//! - Same-row cells, on every path, for a row-cell target: the integer and enum cells of
//!   that row whose properties a read set reads in the same iteration as the target's.
//! - Never derived, text, multiline, boolean, notes or list cells of other rows.
use crate::assistant::contract::{
    self, EnvelopeItem, EnvelopeTarget, ItemKind, MAX_CELL_ALIASES, MAX_FIELD_ALIASES,
};
use crate::assistant::output::Supplied;
use crate::assistant::request::Draft;
use crate::definitions::{Control, Definition, Field, ItemProperty, formula::Compiled};
use serde_json::Value;
use std::collections::BTreeSet;

/// The bound at or below which only related scalars are sent.
pub(crate) const RELATED_ONLY_BYTES: usize = 1024;

/// The target of a job, as its typed address names it.
#[derive(Clone, Copy, Debug)]
pub(crate) enum Target<'a> {
    Field(&'a Field),
    Cell {
        list: &'a Field,
        host: &'a str,
        property: &'a ItemProperty,
    },
}

fn context_kind(control: Control) -> Option<ItemKind> {
    match control {
        Control::Integer => Some(ItemKind::Integer),
        Control::Enumeration => Some(ItemKind::Enum),
        _ => None,
    }
}

/// The integer value of a stored cell: `None` is unknown (`null`).
fn integer(value: Option<&Value>) -> Option<i64> {
    value.and_then(Value::as_i64)
}

/// The scalars that some read set contains together with the target.
fn related_scalars(compiled: Option<&Compiled>, target: Target<'_>) -> BTreeSet<String> {
    let Some(compiled) = compiled else {
        return BTreeSet::new();
    };
    compiled
        .read_sets()
        .into_iter()
        .filter(|(_, _, set)| match target {
            Target::Field(field) => set.scalars.contains(&field.id),
            Target::Cell { list, property, .. } => set.reads_property(&list.id, &property.id),
        })
        .flat_map(|(_, _, set)| set.scalars)
        .collect()
}

/// The properties of the target's list read in the same iteration as its property.
fn co_read_properties(
    compiled: Option<&Compiled>,
    list: &Field,
    property: &ItemProperty,
) -> BTreeSet<String> {
    let Some(compiled) = compiled else {
        return BTreeSet::new();
    };
    compiled
        .read_sets()
        .into_iter()
        .flat_map(|(_, _, set)| set.co_read_properties(&list.id, &property.id))
        .collect()
}

/// The draft of a job for `target` in `data` (the snapshot the job is bound to), with the
/// phrase read from the target's `unresolved` note and `label` its page label. `bytes` is
/// the generation's bound B.
pub(crate) fn draft(
    definition: &Definition,
    compiled: Option<&Compiled>,
    data: &Value,
    (target, address, label): (Target<'_>, &str, &str),
    phrase: &str,
    bytes: usize,
) -> Draft {
    let related = (bytes <= RELATED_ONLY_BYTES).then(|| related_scalars(compiled, target));
    let target_id = match target {
        Target::Field(field) => field.id.as_str(),
        Target::Cell { property, .. } => property.id.as_str(),
    };
    let target_value = match target {
        Target::Field(field) => integer(data.get(&field.id)),
        Target::Cell {
            list,
            host,
            property,
        } => integer(row(data, list, host).and_then(|item| item.get(&property.id))),
    };
    let mut items = Vec::new();
    let scalars = definition.fields.iter().filter(|field| {
        field.formula.is_none()
            && !matches!(target, Target::Field(own) if own.id == field.id)
            && context_kind(field.control).is_some()
            && related
                .as_ref()
                .is_none_or(|related| related.contains(&field.id))
    });
    for (n, field) in (1..=MAX_FIELD_ALIASES).zip(scalars) {
        let kind = context_kind(field.control).unwrap_or(ItemKind::Enum);
        let value = data.get(&field.id).cloned().unwrap_or(Value::Null);
        items.push((
            EnvelopeItem {
                alias: contract::field_alias(n).unwrap_or_default(),
                id: field.id.clone(),
                label: field.label.clone(),
                kind,
                value: value.clone(),
            },
            Supplied {
                address: super::notes::field_address(&field.id),
                integer: kind == ItemKind::Integer,
                value: integer(Some(&value)),
            },
        ));
    }
    if let Target::Cell {
        list,
        host,
        property,
    } = target
    {
        let co_read = co_read_properties(compiled, list, property);
        let item = row(data, list, host);
        let cells = super::values::properties(list).iter().filter(|other| {
            other.id != property.id
                && co_read.contains(&other.id)
                && context_kind(other.control).is_some()
                // An optional property that is not set has no value to read.
                && item.is_some_and(|item| item.get(&other.id).is_some())
        });
        for (n, other) in (1..=MAX_CELL_ALIASES).zip(cells) {
            let kind = context_kind(other.control).unwrap_or(ItemKind::Enum);
            let value = item
                .and_then(|item| item.get(&other.id))
                .cloned()
                .unwrap_or(Value::Null);
            items.push((
                EnvelopeItem {
                    alias: contract::cell_alias(n).unwrap_or_default(),
                    id: other.id.clone(),
                    label: other.label.clone(),
                    kind,
                    value: value.clone(),
                },
                Supplied {
                    address: super::notes::cell_address(&list.id, host, &other.id),
                    integer: kind == ItemKind::Integer,
                    value: integer(Some(&value)),
                },
            ));
        }
    }
    Draft {
        phrase: phrase.to_owned(),
        target: EnvelopeTarget {
            alias: contract::TARGET_ALIAS,
            id: target_id.to_owned(),
            label: Some(label.to_owned()),
            value: target_value,
        },
        target_address: address.to_owned(),
        items,
    }
}

/// The row with host ID `host` of `list` in `data`.
fn row<'a>(
    data: &'a Value,
    list: &Field,
    host: &str,
) -> Option<&'a serde_json::Map<String, Value>> {
    data.get(&list.id)?
        .as_array()?
        .iter()
        .find(|item| item["id"].as_str() == Some(host))?
        .as_object()
}

#[cfg(test)]
#[path = "interpret_tests.rs"]
mod tests;
