//! The JSON answer of a per-unit save made by the sheet module (step 2a,
//! `docs/assisted-entry.md` "Script on sheet pages"): asked for with exactly one `Accept:
//! application/json` header on the Name, General notes, scalar field and row cell routes.
//! It carries the live revision, the saved unit as the page would render it (control text,
//! Known/Unknown or Set/Not set state, note, switch offer and Undo), the derived values, the
//! advisory checks and the validity flag (ASSIST-21), all computed from that revision by the
//! same view model as the page. Every integer and revision is a string, so a value above
//! 2^53 survives a JavaScript number (ASSIST-22).
use super::{AssistForms, FieldView, ListView, SheetPage, SheetView, error, evaluated, groups};
use crate::campaigns::{
    formulas::{CheckStatus, CheckView, FormulaView, Validity},
    unit::Unit,
};
use axum::{
    http::{HeaderMap, StatusCode, header},
    response::IntoResponse,
};
use serde_json::{Value, json};

/// Whether the request asks for the JSON answer: exactly one `Accept` header whose value is
/// `application/json`. Anything else gets the HTML page and its redirect (D-G).
pub(super) fn wanted(headers: &HeaderMap) -> bool {
    let mut values = headers.get_all(header::ACCEPT).iter();
    matches!(
        (values.next(), values.next()),
        (Some(value), None) if value.as_bytes() == b"application/json"
    )
}

/// What happened to the save.
#[derive(Clone, Copy)]
pub(super) enum Outcome {
    Saved,
    Unchanged,
    Conflict,
    Invalid,
}

impl Outcome {
    fn word(self) -> &'static str {
        match self {
            Self::Saved => "saved",
            Self::Unchanged => "unchanged",
            Self::Conflict => "conflict",
            Self::Invalid => "invalid",
        }
    }
    /// A refused save names its unit, its route and the saved value (`current`), never the
    /// control text: the draft stays in the control.
    fn refused(self) -> bool {
        matches!(self, Self::Conflict | Self::Invalid)
    }
}

/// The answer for `unit` from `view` (the live revision), holding the runtime permit while
/// the view model is built.
pub(super) async fn answer(
    view: SheetView,
    permit: tokio::sync::OwnedSemaphorePermit,
    (unit, host): (Unit, Option<String>),
    (outcome, message): (Outcome, String),
    status: StatusCode,
) -> super::ResultResponse {
    let text = tokio::task::spawn_blocking(move || {
        let _permit = permit;
        projection(&view, (unit, host.as_deref()), outcome, &message).map(|value| value.to_string())
    })
    .await
    .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?
    .map_err(error)?;
    let mut response = (status, [(header::CONTENT_TYPE, "application/json")], text).into_response();
    if outcome.refused() {
        response
            .extensions_mut()
            .insert(super::PrivateTrustedEditor);
    }
    response.extensions_mut().insert(SheetPage);
    Ok(response)
}

fn projection(
    view: &SheetView,
    (unit, host): (Unit, Option<&str>),
    outcome: Outcome,
    message: &str,
) -> Result<Value, crate::AppError> {
    let formulas = evaluated(view);
    let groups = groups(
        view,
        (None, None),
        formulas.as_ref(),
        (None, true),
        &mut AssistForms::default(),
    )?;
    let fields = groups.iter().flat_map(|group| &group.fields);
    let mut answer = unit_json(view, fields, (unit, host));
    if outcome.refused()
        && let Some(members) = answer.as_object_mut()
    {
        let current = current(members, integer(view, unit));
        members.retain(|key, _| key == "address" || key == "action");
        members.insert("current".into(), current.map_or(Value::Null, Value::String));
    }
    let validity = Validity::of(formulas.as_ref(), view.immutable.is_some());
    Ok(json!({
        "status": outcome.word(),
        "message": message,
        "revision": view.snapshot.revision.to_string(),
        "unit": answer,
        "derived": formulas.as_ref().map_or_else(Vec::new, derived),
        "not_computed": formulas
            .as_ref()
            .filter(|formulas| formulas.unavailable.is_none())
            .and_then(FormulaView::not_computed)
            .map(|(text, first)| json!({"text": text, "id": first.id, "label": first.label})),
        "checks": formulas
            .as_ref()
            .filter(|formulas| formulas.unavailable.is_none())
            .map(checks),
        "unavailable": formulas
            .as_ref()
            .and_then(|formulas| formulas.unavailable.as_ref())
            .map(|unavailable| json!({"code": unavailable.code, "message": unavailable.message})),
        "validity": {
            "validated": validity.validated(),
            "reasons": validity
                .links()
                .into_iter()
                .map(|(text, anchor)| json!({"text": text, "anchor": anchor}))
                .collect::<Vec<_>>(),
        },
    }))
}

/// The saved unit as the page renders it: its address, its route at this revision, its
/// control text and state, its note, switch offer and Undo.
fn unit_json<'a>(
    view: &SheetView,
    mut fields: impl Iterator<Item = &'a FieldView>,
    (unit, host): (Unit, Option<&str>),
) -> Value {
    let sheet = view.snapshot.sheet_id;
    let Some(pin) = view.immutable.as_ref() else {
        return Value::Null;
    };
    let definition = pin.definition();
    match unit {
        Unit::Name | Unit::Notes => {
            let (which, text) = if unit == Unit::Name {
                ("name", &view.snapshot.name)
            } else {
                ("notes", &view.snapshot.notes)
            };
            json!({
                "address": format!("meta:{which}"),
                "action": format!("/private-sheets/{sheet}/meta/{which}"),
                "value": text,
                "state": null,
                "note": null,
                "actions": null,
            })
        }
        Unit::Field(f) => {
            let id = &definition.fields[f].id;
            let shown = fields.find(|field| field.list.is_none() && field.key == f.to_string());
            let address = super::super::notes::field_address(id);
            let mut answer = json!({
                "address": address,
                "action": format!("/private-sheets/{sheet}/fields/{id}"),
            });
            if let Some(field) = shown {
                extend(
                    &mut answer,
                    (&field.value, field.nullable.then_some(&field.value_state)),
                    (field.note.as_ref(), field.actions.as_ref()),
                );
                note_key(view, &address, &mut answer);
            }
            answer
        }
        Unit::Cell { f, p, .. } => {
            let list_field = &definition.fields[f];
            let property = &super::values::properties(list_field)[p];
            let list: Option<&ListView> = fields
                .filter_map(|field| field.list.as_ref())
                .find(|list| list.f == f);
            let row = list.and_then(|list| {
                list.rows
                    .iter()
                    .find(|row| host.is_some_and(|host| row.id == host))
            });
            let mut answer = json!({
                "address": host.map(|host| super::super::notes::cell_address(&list_field.id, host, &property.id)),
                "action": row.map(|row| format!("/private-sheets/{sheet}/cells/{f}/{}/{p}", row.r)),
            });
            if let Some(cell) = row.and_then(|row| row.cells.get(p)) {
                extend(
                    &mut answer,
                    (&cell.value, property.optional.then_some(&cell.state)),
                    (cell.note.as_ref(), cell.actions.as_ref()),
                );
                if let Some(host) = host {
                    let address =
                        super::super::notes::cell_address(&list_field.id, host, &property.id);
                    note_key(view, &address, &mut answer);
                }
            }
            answer
        }
        Unit::NewRow(_) | Unit::QuickAdd(_) | Unit::Row { .. } => Value::Null,
    }
}

/// Adds `key` to the shown note of an `unresolved` scribble (step 2c-1): the stored note's
/// own revision and typed address (a row cell by host ID, never by index), which the
/// sheet module posts to start an interpretation. Additive: older clients ignore it.
fn note_key(view: &SheetView, address: &str, answer: &mut Value) {
    if answer["note"]["kind"] != "unresolved" {
        return;
    }
    if let Some(stored) = view
        .notes
        .current
        .get(address)
        .filter(|stored| stored.note.op == super::super::notes::Op::Unresolved)
    {
        answer["note"]["key"] = json!(super::super::assist::key(stored));
    }
}

/// The saved value a refusal shows next to the kept draft, as the page would read it: an
/// excerpt of the text, "? (unknown)", "not set", or the value a scribble kept; `None`
/// when the unit has no value at that revision (its row is gone).
fn current(unit: &serde_json::Map<String, Value>, integer: bool) -> Option<String> {
    let value = unit.get("value")?.as_str()?;
    let note = unit.get("note").filter(|note| note["kind"] == "unresolved");
    Some(match (unit.get("state").and_then(Value::as_str), note) {
        (Some("unknown"), _) => "? (unknown)".into(),
        (Some("absent"), _) => "not set".into(),
        (_, Some(note)) if note["unknown"] == true => "? (unknown)".into(),
        (_, Some(note)) => note["value"].as_str().unwrap_or_default().to_owned(),
        _ if value.is_empty() && integer => "? (unknown)".into(),
        _ if value.is_empty() => "(empty)".into(),
        _ => format!("\u{201c}{}\u{201d}", super::excerpt(value)),
    })
}

/// Whether the unit is an integer control: empty, it is unknown rather than empty.
fn integer(view: &SheetView, unit: Unit) -> bool {
    let Some(pin) = view.immutable.as_ref() else {
        return false;
    };
    let fields = &pin.definition().fields;
    let control = match unit {
        Unit::Field(f) => fields[f].control,
        Unit::Cell { f, p, .. } => super::values::properties(&fields[f])[p].control,
        _ => return false,
    };
    control == crate::definitions::Control::Integer
}

/// Adds a shown unit's control text, state, note and actions.
fn extend(
    answer: &mut Value,
    (value, state): (&str, Option<&String>),
    (note, actions): (Option<&super::NoteLine>, Option<&super::Actions>),
) {
    answer["value"] = json!(value);
    answer["state"] = json!(state);
    answer["note"] = note.map_or(Value::Null, |note| {
        json!({
            "kind": note.kind,
            "shown": note.shown(),
            "value": note.value,
            "unknown": note.unknown,
        })
    });
    answer["actions"] = actions.map_or(Value::Null, |actions| {
        json!({
            "note": actions.note,
            "offer": actions
                .offer
                .as_ref()
                .map(|offer| json!({"text": offer.text, "value": offer.value})),
            "undo": actions.undo,
        })
    });
}

fn derived(formulas: &FormulaView) -> Vec<Value> {
    if formulas.unavailable.is_some() {
        return Vec::new();
    }
    formulas
        .derived
        .iter()
        .map(|item| {
            json!({
                "id": item.id,
                "text": item.view.text(),
                "code": (!item.view.is_value()).then(|| item.view.code()),
            })
        })
        .collect()
}

/// The checks section: counts, then the checks needing attention (violated first) and the
/// passing ones, in the page's order.
fn checks(formulas: &FormulaView) -> Value {
    let item = |check: &CheckView| {
        let failed = !check.code.is_empty();
        json!({
            "id": check.id,
            "status": match check.status {
                CheckStatus::Pass => "pass",
                CheckStatus::Violated => "violated",
                CheckStatus::NotEvaluated => "not_evaluated",
            },
            "word": check.status_word(),
            "message": check.message,
            "code": failed.then_some(check.code),
            "explanation": failed.then_some(&check.explanation),
            "expr": check.expr,
            "rule_ids": check.rule_ids,
        })
    };
    json!({
        "violated": formulas.violated.to_string(),
        "not_evaluated": formulas.not_evaluated.to_string(),
        "passed": formulas.passed.to_string(),
        "items": formulas
            .attention()
            .into_iter()
            .chain(formulas.passes())
            .map(item)
            .collect::<Vec<_>>(),
    })
}
