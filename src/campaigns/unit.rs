//! Per-field save units (assisted sheet slice 1.5a, plan §3a "Per-cell save", D-C): the
//! sheet's Name and General notes (`meta/name`, `meta/notes`) and each non-derived scalar
//! field (`fields/{field_id}`) save on their own, each from its own form.
//!
//! A unit form carries `base`, the revision its displayed value was read at. A save is a
//! conflict only when the unit's **protected value** differs between `base` and the live
//! revision (values, not intervening history: an A → B → A change is no conflict). The patch
//! is always applied to the live snapshot. List rows and cells are units of slice 1.5b.
use super::{snapshots::PrivateSnapshot, values};
use crate::{
    AppError,
    definitions::{Control, Definition},
};
use serde_json::Value;
use std::collections::{BTreeMap, HashSet};

/// One save unit of a runtime sheet.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Unit {
    Name,
    Notes,
    /// A scalar field: its index in the pinned definition.
    Field(usize),
}

/// What a conflict compares: the unit's value in one snapshot.
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum Protected<'a> {
    Text(&'a str),
    /// A scalar field's stored value (`None` when the snapshot does not hold it).
    Value(Option<&'a Value>),
}

/// The admitted keys of a unit form; a nullable field adds its Known/Unknown `null`.
const KEYS: [&str; 3] = ["csrf", "base", "value"];

impl Unit {
    /// The unit a `meta/{which}` route names.
    pub(crate) fn meta(which: &str) -> Option<Self> {
        match which {
            "name" => Some(Self::Name),
            "notes" => Some(Self::Notes),
            _ => None,
        }
    }

    /// The unit a `fields/{id}` route names: a field of the pin that is neither derived (a
    /// formula) nor a list (lists save by row and cell, slice 1.5b). `None` (400) for an ID
    /// outside the slug grammar, an ID the pin does not declare, a derived field or a list.
    pub(crate) fn field(definition: &Definition, id: &str) -> Option<Self> {
        if !crate::definitions::valid_field_id(id) {
            return None;
        }
        let (f, field) = definition
            .fields
            .iter()
            .enumerate()
            .find(|(_, field)| field.id == id)?;
        (field.formula.is_none() && field.control != Control::List).then_some(Self::Field(f))
    }

    /// The label shown in messages.
    pub(crate) fn label(self, definition: &Definition) -> &str {
        match self {
            Self::Name => "Name",
            Self::Notes => "General notes",
            Self::Field(f) => &definition.fields[f].label,
        }
    }

    /// The exact key set of the unit's form.
    pub(crate) fn keys(self, definition: &Definition) -> HashSet<&'static str> {
        let mut keys: HashSet<&'static str> = KEYS.into_iter().collect();
        if let Self::Field(f) = self
            && definition.fields[f].nullable
        {
            keys.insert("null");
        }
        keys
    }

    /// The unit's protected value in `snapshot` (plan §3a, the conflict table).
    pub(crate) fn protected<'a>(
        self,
        definition: &Definition,
        snapshot: &'a PrivateSnapshot,
    ) -> Protected<'a> {
        match self {
            Self::Name => Protected::Text(&snapshot.name),
            Self::Notes => Protected::Text(&snapshot.notes),
            Self::Field(f) => Protected::Value(snapshot.data.get(&definition.fields[f].id)),
        }
    }

    /// Applies the admitted unit form to the live `snapshot`: its new value and, for a number,
    /// enum or yes/no field, its change note (into `pending`). Returns whether anything
    /// changes; a submission equal to the saved value that records no note is a no-op (D-U).
    /// Values are read exactly as the full form reads them (`values::scalar`, `name_notes`).
    pub(crate) fn apply(
        self,
        pin: &crate::definitions::private_pin::ImmutablePin,
        snapshot: &mut PrivateSnapshot,
        (form, current): (
            &BTreeMap<String, String>,
            &BTreeMap<String, super::notes::Stored>,
        ),
        pending: &mut Vec<super::notes::Note>,
    ) -> Result<bool, AppError> {
        let value = &form["value"];
        match self {
            Self::Name => {
                let name = super::name_notes(value, &snapshot.notes)?;
                let changed = name != snapshot.name;
                snapshot.name = name;
                Ok(changed)
            }
            Self::Notes => {
                super::name_notes(&snapshot.name, value)?;
                let changed = *value != snapshot.notes;
                snapshot.notes.clone_from(value);
                Ok(changed)
            }
            Self::Field(f) => {
                let field = &pin.definition().fields[f];
                let mut recorder = values::Recorder::new(current);
                let stored = values::scalar(
                    field,
                    value,
                    form.get("null").map(String::as_str),
                    snapshot.data.get(&field.id),
                    &mut recorder,
                )?;
                let mut data = snapshot.data.clone();
                let object = data
                    .as_object_mut()
                    .ok_or(AppError::Invalid("Invalid runtime sheet data."))?;
                object.insert(field.id.clone(), stored);
                pin.validate_values(&data)?;
                let changed = data != snapshot.data || !recorder.notes.is_empty();
                snapshot.data = data;
                *pending = recorder.notes;
                Ok(changed)
            }
        }
    }
}
