//! Formula profile admission for definition envelopes: list/derived field shapes, the
//! `formula_profile` marker, rule citations, evidence references and CEL compilation.
use super::{
    Constraint, Control, Definition, DefinitionError, Field, Mapping, Region, SourceRole,
    ValidationReport, admission::label, formula,
};
use std::collections::BTreeSet;

/// The only admitted profile; it names "cel-mimik-1" (see `formula::PROFILE`).
pub(super) const PROFILE: u32 = 1;
/// Text limits shared with the generated value schema (`campaigns::values`).
pub(crate) const TEXT_CHARS: usize = 8_000;
pub(crate) const MULTILINE_CHARS: usize = 20_000;

fn report(path: &str, code: &str) -> DefinitionError {
    ValidationReport::one(path, code)
}

/// CEL identifier: what formulas can reference. Hyphenated field IDs stay valid but are
/// not declared to the formula layer, so referencing them is `unknown_field`.
fn cel_identifier(value: &str) -> bool {
    value.as_bytes().first().is_some_and(u8::is_ascii_lowercase)
        && value
            .bytes()
            .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'_')
}

fn uses_profile(definition: &Definition) -> bool {
    !definition.constraints.is_empty()
        || definition.fields.iter().any(|field| {
            field.formula.is_some() || field.items.is_some() || field.control == Control::List
        })
}

fn choices_valid(options: &[super::Choice]) -> bool {
    let mut ids = BTreeSet::new();
    options.iter().all(|option| {
        crate::registry::valid_id(&option.id) && ids.insert(&option.id) && label(&option.label)
    })
}

fn items(index: usize, field: &Field, definition: &Definition) -> Result<(), DefinitionError> {
    let path = format!("/fields/{index}");
    match (field.control, &field.items) {
        (Control::List, None) => return Err(report(&path, "list-items-required")),
        (Control::List, Some(_)) if field.nullable => {
            return Err(report(&path, "list-must-not-be-nullable"));
        }
        (Control::List, Some(_)) if !cel_identifier(&field.id) => {
            return Err(report(&path, "list-id-must-be-identifier"));
        }
        (Control::List, Some(_)) if field.formula.is_some() => {
            return Err(report(&path, "formula-field-cannot-be-list"));
        }
        (Control::List, Some(_)) => {}
        (_, Some(_)) => return Err(report(&path, "items-only-supported-for-list")),
        (_, None) => return Ok(()),
    }
    let path = format!("{path}/items");
    let mut ids = BTreeSet::new();
    for property in field.items.iter().flat_map(|items| &items.properties) {
        if !cel_identifier(&property.id)
            || property.id == "id"
            || !ids.insert(property.id.as_str())
            || !label(&property.label)
        {
            return Err(report(&path, "invalid-or-duplicate-item-property"));
        }
        match (property.control, &property.options) {
            (Control::Enumeration, Some(options)) if choices_valid(options) => {}
            (Control::Enumeration, Some(_)) => {
                return Err(report(&path, "enum-choice-duplicate-or-invalid-id-label"));
            }
            (Control::Enumeration, None) => return Err(report(&path, "enum-options-required")),
            (Control::List, _) => return Err(report(&path, "nested-lists-unsupported")),
            (_, Some(_)) => return Err(report(&path, "options-only-supported-for-enum")),
            (_, None) => {}
        }
    }
    catalogue_members(&path, field, definition)?;
    match field
        .items
        .as_ref()
        .and_then(|items| items.quick_add.as_ref())
    {
        Some(quick) => quick_add(&format!("{path}/quick_add"), field, quick),
        None => Ok(()),
    }
}

/// The `catalogue` member (assisted sheet step 3a): on a text property, naming one of the
/// definition's linked catalogues; at most one per list; its `key` names another optional
/// text property of the list that has no member itself. Paths are exact, codes static.
fn catalogue_members(
    path: &str,
    field: &Field,
    definition: &Definition,
) -> Result<(), DefinitionError> {
    let properties: Vec<_> = field
        .items
        .iter()
        .flat_map(|items| &items.properties)
        .collect();
    let mut seen = false;
    for (j, property) in properties.iter().enumerate() {
        let Some(member) = &property.catalogue else {
            continue;
        };
        let at = format!("{path}/properties/{j}/catalogue");
        if property.control != Control::Text {
            return Err(report(&at, "catalogue-member-not-text"));
        }
        if !definition
            .catalogues
            .iter()
            .any(|link| link.id == member.id)
        {
            return Err(report(&at, "catalogue-member-unlinked"));
        }
        if std::mem::replace(&mut seen, true) {
            return Err(report(&at, "catalogue-member-repeated"));
        }
        let key = properties
            .iter()
            .enumerate()
            .find(|(k, other)| *k != j && other.id == member.key);
        let Some((_, key)) = key else {
            return Err(report(&at, "catalogue-key-unknown"));
        };
        if key.control != Control::Text || !key.optional || key.catalogue.is_some() {
            return Err(report(&at, "catalogue-key-invalid"));
        }
    }
    Ok(())
}

/// A list's quick-add line (assisted sheet slice 1.6, ASSIST-3) fills only the mapped name
/// (a text property) and count (an integer property); every other required property is left
/// unknown, which only an integer can be. So a required enum, yes/no, text or multiline
/// property other than the name refuses the member (no quick-add line for such a list).
fn quick_add(path: &str, field: &Field, quick: &super::QuickAdd) -> Result<(), DefinitionError> {
    let properties = field.items.iter().flat_map(|items| &items.properties);
    let control = |id: &str| {
        properties
            .clone()
            .find(|property| property.id == id)
            .map(|property| property.control)
    };
    if control(&quick.name) != Some(Control::Text) {
        return Err(report(path, "quick-add-name-not-text-property"));
    }
    let key = properties
        .clone()
        .find_map(|property| property.catalogue.as_ref())
        .map(|member| member.key.as_str());
    if key == Some(quick.name.as_str()) {
        return Err(report(path, "quick-add-name-is-catalogue-key"));
    }
    if let Some(quantity) = &quick.quantity
        && control(quantity) != Some(Control::Integer)
    {
        return Err(report(path, "quick-add-quantity-not-integer-property"));
    }
    for property in properties.filter(|property| !property.optional && property.id != quick.name) {
        match property.control {
            Control::Enumeration | Control::Boolean => {
                return Err(report(path, "quick-add-required-choice-property"));
            }
            Control::Text | Control::Multiline => {
                return Err(report(path, "quick-add-required-text-property"));
            }
            Control::Integer | Control::List => {}
        }
    }
    Ok(())
}

fn rule_ids(path: &str, ids: &[String]) -> Result<(), DefinitionError> {
    if ids.iter().all(|id| crate::registry::valid_id(id)) {
        Ok(())
    } else {
        Err(report(path, "invalid-rule-id"))
    }
}

/// Evidence must cite selected core or supplement sources: a sheet alone does not back a rule.
fn evidence<'a>(
    path: &str,
    mapping: &'a Mapping,
    definition: &Definition,
    distinct: &mut BTreeSet<&'a Region>,
) -> Result<(), DefinitionError> {
    let path = format!("{path}/evidence");
    match mapping {
        Mapping::Unresolved { reason } if !label(reason) => {
            Err(report(&path, "unresolved-reason-required"))
        }
        Mapping::Unresolved { .. } => Ok(()),
        // Rule evidence must cite something; the schema also requires one region.
        Mapping::Mapped { regions } if regions.is_empty() => {
            Err(report(&path, "evidence-regions-required"))
        }
        Mapping::Mapped { regions } => {
            for region in regions {
                let backed = definition.sources.iter().any(|source| {
                    source.source_id == region.source_id
                        && source
                            .roles
                            .iter()
                            .any(|role| matches!(role, SourceRole::Core | SourceRole::Supplement))
                });
                if !backed {
                    return Err(report(
                        &path,
                        "evidence-needs-selected-core-or-supplement-source",
                    ));
                }
                distinct.insert(region);
            }
            Ok(())
        }
    }
}

fn mapped(mapping: &Mapping) -> &[Region] {
    match mapping {
        Mapping::Mapped { regions } => regions,
        Mapping::Unresolved { .. } => &[],
    }
}

/// One source region a definition depends on, with the envelope path that cites it.
pub(super) struct Dependency<'a> {
    pub path: String,
    pub region: &'a Region,
    /// Formula/constraint rule evidence rather than field wiring.
    pub evidence: bool,
}

/// The single walk over every dependency: field wiring (reported at the legacy `/fields`
/// path) and formula/constraint evidence (reported at its exact path).
pub(super) fn dependencies(definition: &Definition) -> Vec<Dependency<'_>> {
    let mut found = Vec::new();
    for (index, field) in definition.fields.iter().enumerate() {
        found.extend(mapped(&field.mapping).iter().map(|region| Dependency {
            path: "/fields".into(),
            region,
            evidence: false,
        }));
        if let Some(formula) = &field.formula {
            found.extend(mapped(&formula.evidence).iter().map(|region| Dependency {
                path: format!("/fields/{index}/formula/evidence"),
                region,
                evidence: true,
            }));
        }
    }
    for (index, constraint) in definition.constraints.iter().enumerate() {
        found.extend(
            mapped(&constraint.evidence)
                .iter()
                .map(|region| Dependency {
                    path: format!("/constraints/{index}/evidence"),
                    region,
                    evidence: true,
                }),
        );
    }
    found
}

/// Every source region a definition depends on: field mappings and formula/constraint evidence.
pub(super) fn regions(definition: &Definition) -> impl Iterator<Item = &Region> {
    dependencies(definition).into_iter().map(|item| item.region)
}

/// Evidence regions of formulas and constraints only (not field wiring).
pub(super) fn evidence_regions(definition: &Definition) -> impl Iterator<Item = &Region> {
    dependencies(definition)
        .into_iter()
        .filter(|item| item.evidence)
        .map(|item| item.region)
}

/// IDs of derived fields and constraints whose evidence is unresolved, in definition order.
pub(super) fn unresolved_evidence(definition: &Definition) -> (Vec<String>, Vec<String>) {
    let unresolved = |mapping: &Mapping| matches!(mapping, Mapping::Unresolved { .. });
    (
        definition
            .fields
            .iter()
            .filter(|field| {
                field
                    .formula
                    .as_ref()
                    .is_some_and(|formula| unresolved(&formula.evidence))
            })
            .map(|field| field.id.clone())
            .collect(),
        definition
            .constraints
            .iter()
            .filter(|constraint| unresolved(&constraint.evidence))
            .map(|constraint| constraint.id.clone())
            .collect(),
    )
}

fn value_type(
    field_control: Control,
    options: Option<&Vec<super::Choice>>,
) -> Option<formula::ValueType> {
    Some(match field_control {
        Control::Text => formula::ValueType::Text(TEXT_CHARS),
        Control::Multiline => formula::ValueType::Text(MULTILINE_CHARS),
        Control::Integer => formula::ValueType::Int,
        Control::Boolean => formula::ValueType::Bool,
        Control::Enumeration => {
            formula::ValueType::Enum(options?.iter().map(|choice| choice.id.clone()).collect())
        }
        Control::List => return None,
    })
}

/// Input fields the formula layer does not declare (non-identifier IDs such as
/// `hit-points`). Their stored values are dropped before evaluation, because the engine
/// refuses every undeclared key.
pub(super) fn undeclared_inputs(definition: &Definition) -> impl Iterator<Item = &str> {
    definition
        .fields
        .iter()
        .filter(|field| field.formula.is_none() && !cel_identifier(&field.id))
        .map(|field| field.id.as_str())
}

/// Projects an admitted envelope onto the formula layer's declarations.
pub(super) fn formula_spec(definition: &Definition) -> formula::SheetSpec {
    let mut spec = formula::SheetSpec::default();
    for field in &definition.fields {
        let ty = value_type(field.control, field.options.as_ref());
        match (&field.formula, field.control, ty) {
            (Some(item), _, Some(ty)) => spec.derived.push(formula::DerivedSpec {
                id: field.id.clone(),
                ty,
                expr: item.expr.clone(),
            }),
            (None, Control::List, _) if cel_identifier(&field.id) => {
                spec.declarations.lists.push(formula::ListDecl {
                    id: field.id.clone(),
                    properties: field
                        .items
                        .iter()
                        .flat_map(|items| &items.properties)
                        .filter_map(|property| {
                            Some(formula::PropertyDecl {
                                id: property.id.clone(),
                                ty: value_type(property.control, property.options.as_ref())?,
                                optional: property.optional,
                            })
                        })
                        .collect(),
                });
            }
            (None, _, Some(ty)) if cel_identifier(&field.id) => {
                spec.declarations.fields.push(formula::FieldDecl {
                    id: field.id.clone(),
                    ty,
                    nullable: field.nullable,
                });
            }
            _ => {}
        }
    }
    spec.constraints = definition
        .constraints
        .iter()
        .map(|constraint| formula::ConstraintSpec {
            id: constraint.id.clone(),
            expr: constraint.expr.clone(),
        })
        .collect();
    spec
}

/// Points a formula-layer rejection at the envelope member it concerns.
fn rejection_path(definition: &Definition, rejection: &formula::Rejection) -> String {
    let target = rejection.target.as_str();
    if let Some(index) = definition
        .fields
        .iter()
        .position(|field| field.id == target && field.formula.is_some())
    {
        return format!("/fields/{index}/formula");
    }
    if let Some(index) = definition
        .fields
        .iter()
        .position(|field| field.id == target)
    {
        return format!("/fields/{index}");
    }
    if let Some(index) = definition
        .constraints
        .iter()
        .position(|constraint| constraint.id == target)
    {
        return format!("/constraints/{index}");
    }
    if let Some(index) = definition.fields.iter().position(|field| {
        field
            .items
            .iter()
            .flat_map(|items| &items.properties)
            .any(|property| property.id == target)
    }) {
        return format!("/fields/{index}/items");
    }
    "/".into()
}

fn constraint(index: usize, item: &Constraint) -> Result<(), DefinitionError> {
    let path = format!("/constraints/{index}");
    if !label(&item.message) {
        return Err(report(&path, "constraint-message-required"));
    }
    rule_ids(&path, &item.rule_ids)
}

/// Formula-profile domain checks, run after the existing envelope checks. Returns the
/// compiled formulas of a formula definition (`None` for a formula-free one); only immutable
/// pins keep them, every other caller drops them at once.
pub(super) fn check(definition: &Definition) -> Result<Option<formula::Compiled>, DefinitionError> {
    match (definition.formula_profile, uses_profile(definition)) {
        (None, false) => return Ok(None),
        (None, true) => return Err(report("/formula_profile", "formula-profile-required")),
        (Some(_), false) => {
            return Err(report(
                "/formula_profile",
                "formula-profile-without-formulas",
            ));
        }
        (Some(PROFILE), true) => {}
        (Some(_), true) => return Err(report("/formula_profile", "unsupported-formula-profile")),
    }
    let mut distinct: BTreeSet<&Region> = BTreeSet::new();
    for (index, field) in definition.fields.iter().enumerate() {
        items(index, field, definition)?;
        if let Some(item) = &field.formula {
            if field.nullable {
                return Err(report(
                    &format!("/fields/{index}"),
                    "formula-field-must-not-be-nullable",
                ));
            }
            let path = format!("/fields/{index}/formula");
            rule_ids(&path, &item.rule_ids)?;
            evidence(&path, &item.evidence, definition, &mut distinct)?;
        }
    }
    for (index, item) in definition.constraints.iter().enumerate() {
        constraint(index, item)?;
        evidence(
            &format!("/constraints/{index}"),
            &item.evidence,
            definition,
            &mut distinct,
        )?;
    }
    distinct.extend(regions(definition));
    if distinct.len() > 1024 {
        return Err(report("/fields", "mapping-dependency-limit"));
    }
    let spec = formula_spec(definition);
    formula::Compiled::new(&spec)
        .map(Some)
        .map_err(|rejection| {
            report(
                &rejection_path(definition, &rejection),
                &format!("formula-{}", rejection.code.as_str().replace('_', "-")),
            )
        })
}

/// Canonical form: rule citations are sets (sorted, deduplicated), evidence regions are
/// sorted; expression text, constraint order and property order are kept verbatim.
pub(super) fn canonicalize(definition: &mut Definition) {
    let normalize = |ids: &mut Vec<String>, mapping: &mut Mapping| {
        let set: BTreeSet<String> = ids.drain(..).collect();
        ids.extend(set);
        if let Mapping::Mapped { regions } = mapping {
            regions.sort();
        }
    };
    for field in &mut definition.fields {
        if let Some(item) = &mut field.formula {
            normalize(&mut item.rule_ids, &mut item.evidence);
        }
    }
    for item in &mut definition.constraints {
        normalize(&mut item.rule_ids, &mut item.evidence);
    }
}
