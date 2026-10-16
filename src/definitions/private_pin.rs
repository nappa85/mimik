//! Immutable interpretation admission is independent of current source evidence.
use super::{Definition, DefinitionError, WiringStatus, admission, formula, publication};
use crate::AppError;
use rusqlite::{Connection, OptionalExtension, params};
use std::time::Duration;

/// Wall-clock safety net for sheet evaluation. Route tests run under parallel CPU load, so
/// test builds use a generous seam; the engine's own deadline tests cover the production
/// value.
#[cfg(not(test))]
const EVALUATION_DEADLINE: Duration = formula::DEADLINE;
#[cfg(test)]
const EVALUATION_DEADLINE: Duration = Duration::from_mins(1);

pub(crate) struct RawPin {
    pub id: i64,
    pub version: i64,
    pub game: String,
    pub edition: String,
    pub content: String,
    pub hash: String,
}

pub(crate) struct ImmutablePin {
    pub raw: RawPin,
    data: admission::Admitted,
    validator: jsonschema::Validator,
    /// Compiled once per pin load by admission itself; `None` for formula-free definitions
    /// and for write-only pins.
    formulas: Option<formula::Compiled>,
    /// Loaded for writes only ([`validate_for_write`]): evaluation fails closed.
    write_only: bool,
}

pub(crate) fn load(db: &Connection, id: i64) -> Result<RawPin, AppError> {
    let (version,game,edition,content,hash):(i64,String,String,Option<String>,String)=db.query_row("SELECT version,CASE WHEN length(CAST(game_id AS BLOB))<=64 THEN game_id END,CASE WHEN length(CAST(edition_id AS BLOB))<=64 THEN edition_id END,CASE WHEN length(CAST(content AS BLOB))<=262144 THEN content END,CASE WHEN length(CAST(content_hash AS BLOB))=64 THEN content_hash END FROM definition_versions WHERE id=?1",[id],|r|Ok((r.get(0)?,r.get(1)?,r.get(2)?,r.get(3)?,r.get(4)?))).optional()?.ok_or(AppError::NotFound)?;
    Ok(RawPin {
        id,
        version,
        game,
        edition,
        content: content.ok_or(AppError::Invalid("Immutable definition exceeds limit."))?,
        hash,
    })
}

pub(crate) fn validate(raw: RawPin) -> Result<ImmutablePin, AppError> {
    validated(raw, false)
}

/// A pin for writes that never evaluate (freezing up to 32 sheets at once): admission
/// still compiles the formulas, but drops them at once, so peak memory stays one compiled
/// definition instead of 32. Evaluating such a pin fails closed as `internal_error`.
pub(crate) fn validate_for_write(raw: RawPin) -> Result<ImmutablePin, AppError> {
    validated(raw, true)
}

fn validated(raw: RawPin, write_only: bool) -> Result<ImmutablePin, AppError> {
    let (data, formulas) = admission::validate_compiled(raw.content.as_bytes())
        .map_err(|_| AppError::Invalid("Corrupt immutable definition."))?;
    let formulas = formulas.filter(|_| !write_only);
    if raw.version <= 0
        || data.content != raw.content
        || data.hash != raw.hash
        || data.definition.game_id != raw.game
        || data.definition.edition_id != raw.edition
    {
        return Err(AppError::Invalid("Immutable definition identity mismatch."));
    }
    let validator = crate::campaigns::values::schema(&data.definition)?;
    Ok(ImmutablePin {
        raw,
        data,
        validator,
        formulas,
        write_only,
    })
}

/// A pin built from a definition envelope without a database (tests only).
#[cfg(test)]
pub(crate) fn for_tests(definition: &serde_json::Value) -> ImmutablePin {
    let admitted = admission::validate(&serde_json::to_vec(definition).unwrap()).unwrap();
    validate(RawPin {
        id: 1,
        version: 1,
        game: admitted.definition.game_id.clone(),
        edition: admitted.definition.edition_id.clone(),
        content: admitted.content,
        hash: admitted.hash,
    })
    .unwrap_or_else(|_| panic!("test pin rejected"))
}

/// Host-managed item IDs (registry slug, unique per list) and exact i64 item integers; JSON
/// Schema `integer` alone also accepts u64 values above `i64::MAX`.
fn list_values(field: &super::Field, value: &serde_json::Value) -> Result<(), AppError> {
    let integers: Vec<&str> = field
        .items
        .iter()
        .flat_map(|items| &items.properties)
        .filter(|property| property.control == super::Control::Integer)
        .map(|property| property.id.as_str())
        .collect();
    let mut ids = std::collections::BTreeSet::new();
    for item in value.as_array().into_iter().flatten() {
        let id = item["id"].as_str().unwrap_or_default();
        if !crate::registry::valid_id(id) || !ids.insert(id) {
            return Err(AppError::Invalid("List item IDs must be unique slugs."));
        }
        for property in &integers {
            if item
                .get(*property)
                .is_some_and(|number| number.as_i64().is_none())
            {
                return Err(AppError::Invalid("Integer outside signed i64 range."));
            }
        }
    }
    Ok(())
}

impl ImmutablePin {
    pub(crate) fn validate_values(&self, value: &serde_json::Value) -> Result<(), AppError> {
        // Aggregate bound at the formula input boundary: per-value caps alone allow megabytes.
        // Formula-free pins keep their original value contract (no retroactive cap).
        if self.data.definition.formula_profile.is_some()
            && serde_json::to_vec(value)?.len() > super::formula::MAX_INPUT_BYTES
        {
            return Err(AppError::Invalid("Sheet values exceed the size limit."));
        }
        if !self.validator.is_valid(value) {
            return Err(AppError::Invalid("Pinned value validation failed."));
        }
        let out_of_range = || AppError::Invalid("Integer outside signed i64 range.");
        for field in &self.data.definition.fields {
            let field_value = &value[&field.id];
            match field.control {
                super::Control::Integer
                    if !field_value.is_null() && field_value.as_i64().is_none() =>
                {
                    return Err(out_of_range());
                }
                super::Control::List => list_values(field, field_value)?,
                _ => {}
            }
        }
        Ok(())
    }
    pub(crate) fn definition(&self) -> &Definition {
        &self.data.definition
    }
    pub(crate) fn formulas(&self) -> Option<&formula::Compiled> {
        self.formulas.as_ref()
    }
    /// Advisory evaluation of stored (already validated) sheet values; `None` for formula-free
    /// pins. Pure: it never writes, and model output never reaches it.
    ///
    /// Values of input fields the formula layer does not declare are projected away first;
    /// every other key, including any that is not a definition field, reaches the engine,
    /// which refuses it.
    pub(crate) fn evaluate(
        &self,
        values: &serde_json::Value,
    ) -> Option<Result<formula::Evaluation, formula::InputError>> {
        if self.write_only {
            return Some(Err(formula::InputError {
                code: formula::Code::InternalError,
                path: String::new(),
            }));
        }
        let compiled = self.formulas.as_ref()?;
        let mut hidden = super::formula_admission::undeclared_inputs(&self.data.definition)
            .filter(|id| values.get(*id).is_some())
            .peekable();
        Some(if hidden.peek().is_none() {
            compiled.evaluate_with_deadline(values, EVALUATION_DEADLINE)
        } else {
            let hidden: std::collections::BTreeSet<&str> = hidden.collect();
            let projected: serde_json::Map<String, serde_json::Value> = values
                .as_object()
                .into_iter()
                .flatten()
                .filter(|(key, _)| !hidden.contains(key.as_str()))
                .map(|(key, value)| (key.clone(), value.clone()))
                .collect();
            compiled.evaluate_with_deadline(&projected.into(), EVALUATION_DEADLINE)
        })
    }
    pub(crate) fn unchanged(&self, db: &Connection) -> Result<(), AppError> {
        let matches:bool=db.query_row("SELECT EXISTS(SELECT 1 FROM definition_versions WHERE id=?1 AND version=?2 AND game_id=?3 AND edition_id=?4 AND content_hash=?5 AND content=?6)",params![self.raw.id,self.raw.version,self.raw.game,self.raw.edition,self.raw.hash,self.raw.content],|r|r.get(0))?;
        if !matches {
            return Err(AppError::Invalid("Immutable definition changed."));
        }
        Ok(())
    }
    pub(crate) fn readiness(&self, db: &Connection) -> Result<WiringStatus, AppError> {
        self.unchanged(db)?;
        publication::private_readiness(db, self.raw.id, &self.data).map_err(|error| match error {
            DefinitionError::Storage(error) => error,
            _ => AppError::Invalid("Definition approval/evidence integrity error."),
        })
    }
}
