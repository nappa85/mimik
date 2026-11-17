//! Per-field and per-cell save units (assisted sheet slices 1.5a and 1.5b, plan §3a
//! "Per-cell save", D-C, D-S): the sheet's Name and General notes (`meta/name`,
//! `meta/notes`), each non-derived scalar field (`fields/{field_id}`), and for each list a
//! saved row's cell (`cells/{f}/{r}/{p}`), the new-row form (`lists/{f}/rows`) and a row's
//! Remove, Move up and Move down (`lists/{f}/rows/{r}/{remove|up|down}`), each saved on its
//! own from its own form; and (slice 1.6) a list's quick-add line (`lists/{f}/quick`).
//!
//! A unit form carries `base`, the revision its displayed value was read at. A save is a
//! conflict only when the unit's **protected value** differs between `base` and the live
//! revision (values, not intervening history: an A → B → A change is no conflict). The patch
//! is always applied to the live snapshot. List units name a row by its index at `base` and
//! are planned in `super::rows`.
use super::{snapshots::PrivateSnapshot, values};
use crate::{
    AppError,
    definitions::{Control, Definition, Field},
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
    /// Property `p` of row `r` (its index at the form's `base`) of list field `f`.
    Cell {
        f: usize,
        r: usize,
        p: usize,
    },
    /// The new-row form of list field `f`.
    NewRow(usize),
    /// The quick-add line of list field `f` (slice 1.6).
    QuickAdd(usize),
    /// A row action on row `r` (at `base`) of list field `f`.
    Row {
        f: usize,
        r: usize,
        action: RowAction,
    },
}

/// What a row action does (D-S, D-V).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum RowAction {
    Remove,
    Up,
    Down,
}

impl RowAction {
    pub(crate) fn parse(raw: &str) -> Option<Self> {
        match raw {
            "remove" => Some(Self::Remove),
            "up" => Some(Self::Up),
            "down" => Some(Self::Down),
            _ => None,
        }
    }
}

/// The outcome of a planned unit save.
pub(crate) enum Planned {
    /// The patched snapshot (and its notes) is ready for the writer.
    Write,
    /// The submission equals the saved value (D-U), or the row is already removed.
    Unchanged,
    /// The unit changed between `base` and L, or `base` is above L.
    Conflict,
    /// The value was refused (422).
    Invalid(&'static str),
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

/// List field `f` of the pin, when it is a list and not derived.
fn list(definition: &Definition, f: usize) -> Option<&Field> {
    definition
        .fields
        .get(f)
        .filter(|field| field.control == Control::List && field.formula.is_none())
}

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

    /// The unit a `cells/{f}/{r}/{p}` route names: canonical indices, `f` a list of the pin
    /// that is not derived, `p` one of its properties. The row is looked up at `base`.
    pub(crate) fn cell(definition: &Definition, (f, r, p): (&str, &str, &str)) -> Option<Self> {
        let (f, r, p) = (values::index(f)?, values::index(r)?, values::index(p)?);
        (p < values::properties(list(definition, f)?).len()).then_some(Self::Cell { f, r, p })
    }

    /// The unit a `lists/{f}/rows` route names.
    pub(crate) fn new_row(definition: &Definition, f: &str) -> Option<Self> {
        let f = values::index(f)?;
        list(definition, f).map(|_| Self::NewRow(f))
    }

    /// The unit a `lists/{f}/quick` route names: a list of the pin, not derived, whose
    /// definition declares a quick-add line.
    pub(crate) fn quick_add(definition: &Definition, f: &str) -> Option<Self> {
        let f = values::index(f)?;
        list(definition, f)?
            .items
            .as_ref()?
            .quick_add
            .as_ref()
            .map(|_| Self::QuickAdd(f))
    }

    /// The unit a `lists/{f}/rows/{r}/{remove|up|down}` route names.
    pub(crate) fn row(definition: &Definition, (f, r, action): (&str, &str, &str)) -> Option<Self> {
        let (f, r) = (values::index(f)?, values::index(r)?);
        let action = RowAction::parse(action)?;
        list(definition, f).map(|_| Self::Row { f, r, action })
    }

    /// The label shown in messages ("Current HP", "Quantity, row 2 of Equipment").
    pub(crate) fn label(self, definition: &Definition) -> String {
        let field = |f: usize| &definition.fields[f];
        match self {
            Self::Name => "Name".into(),
            Self::Notes => "General notes".into(),
            Self::Field(f) | Self::QuickAdd(f) => field(f).label.clone(),
            Self::Cell { f, r, p } => {
                super::assist::row(field(f), r, &values::properties(field(f))[p]).label
            }
            Self::NewRow(f) => format!("The new {} row", field(f).label),
            Self::Row { f, r, .. } => format!("Row {} of {}", r + 1, field(f).label),
        }
    }

    /// The exact key set of the unit's form.
    pub(crate) fn keys(self, definition: &Definition) -> HashSet<String> {
        let mut keys: HashSet<String> = match self {
            Self::NewRow(_) | Self::Row { .. } => ["csrf", "base"].map(String::from).into(),
            Self::QuickAdd(_) => ["csrf", "base", "text"].map(String::from).into(),
            _ => KEYS.map(String::from).into(),
        };
        match self {
            Self::Field(f) if definition.fields[f].nullable => {
                keys.insert("null".into());
            }
            Self::Cell { f, p, .. } if values::properties(&definition.fields[f])[p].optional => {
                keys.insert("set".into());
            }
            Self::NewRow(f) => {
                let key = values::key_property(&definition.fields[f]);
                for (p, property) in values::properties(&definition.fields[f]).iter().enumerate() {
                    if key == Some(p) {
                        continue;
                    }
                    keys.insert(format!("row-{p}"));
                    if property.optional {
                        keys.insert(format!("set-{p}"));
                    }
                }
            }
            _ => {}
        }
        keys
    }

    /// The protected value of a metadata or scalar unit in `snapshot` (plan §3a, the
    /// conflict table); list units compare rows in `super::rows`.
    pub(crate) fn protected<'a>(
        self,
        definition: &Definition,
        snapshot: &'a PrivateSnapshot,
    ) -> Protected<'a> {
        match self {
            Self::Name => Protected::Text(&snapshot.name),
            Self::Notes => Protected::Text(&snapshot.notes),
            Self::Field(f) => Protected::Value(snapshot.data.get(&definition.fields[f].id)),
            Self::Cell { .. } | Self::NewRow(_) | Self::QuickAdd(_) | Self::Row { .. } => {
                Protected::Value(None)
            }
        }
    }

    /// Applies the admitted form of a metadata or scalar unit to the live `snapshot`: its
    /// new value and, for a number, enum or yes/no field, its change note (into `pending`).
    /// Returns whether anything changes; a submission equal to the saved value that records
    /// no note is a no-op (D-U). Values are read exactly as the full form reads them
    /// (`values::scalar`, `name_notes`). List units are planned in `super::rows`.
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
            Self::Cell { .. } | Self::NewRow(_) | Self::QuickAdd(_) | Self::Row { .. } => {
                Err(AppError::Invalid("List units are planned by row."))
            }
        }
    }
}
