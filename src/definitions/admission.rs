use super::{
    Control, Definition, DefinitionError, Diagnostic, MAX_BYTES, Mapping, SourceRole,
    ValidationReport, decode, safe_text,
};
use sha2::{Digest, Sha256};
use std::{
    collections::{BTreeMap, BTreeSet},
    sync::LazyLock,
};

static VALIDATOR: LazyLock<Result<jsonschema::Validator, ()>> = LazyLock::new(|| {
    let schema: serde_json::Value =
        serde_json::from_str(include_str!("../../schemas/definition-v1.json")).map_err(|_| ())?;
    jsonschema::draft202012::options()
        .offline()
        .with_pattern_options(jsonschema::PatternOptions::regex())
        .build(&schema)
        .map_err(|_| ())
});

pub(super) struct Admitted {
    pub definition: Definition,
    pub content: String,
    pub hash: String,
}

pub(super) fn hash(bytes: &[u8]) -> String {
    let mut hash = Sha256::new();
    hash.update(b"mimik-definition-v1\0");
    hash.update(bytes);
    hex::encode(hash.finalize())
}

pub(super) fn validate(bytes: &[u8]) -> Result<Admitted, DefinitionError> {
    let value = decode::parse(bytes)?;
    let validator = VALIDATOR
        .as_ref()
        .map_err(|()| DefinitionError::Configuration)?;
    let mut errors = validator.iter_errors(&value);
    let diagnostics: Vec<Diagnostic> = errors
        .by_ref()
        .take(32)
        .map(|error| Diagnostic {
            path: safe_text(error.instance_path().as_str()),
            code: safe_text(error.kind().keyword()),
        })
        .collect();
    let truncated = errors.next().is_some();
    drop(errors);
    if !diagnostics.is_empty() {
        return Err(DefinitionError::Invalid(ValidationReport {
            diagnostics,
            truncated,
        }));
    }
    let mut definition: Definition =
        serde_json::from_value(value).map_err(|_| DefinitionError::Configuration)?;
    domain(&definition)?;
    definition.sources.sort_by_key(|source| source.source_id);
    for source in &mut definition.sources {
        source.roles.sort();
    }
    for field in &mut definition.fields {
        if let Mapping::Mapped { regions } = &mut field.mapping {
            regions.sort();
        }
    }
    let content = serde_json::to_string(&definition).map_err(|_| DefinitionError::Configuration)?;
    if content.len() > MAX_BYTES {
        return Err(ValidationReport::one("/", "canonical-byte-limit"));
    }
    let hash = hash(content.as_bytes());
    Ok(Admitted {
        definition,
        content,
        hash,
    })
}

fn label(value: &str) -> bool {
    !value.trim().is_empty()
}

fn fields(definition: &Definition) -> Result<BTreeSet<&str>, DefinitionError> {
    let mut ids = BTreeSet::new();
    let mut choices = 0;
    for (index, field) in definition.fields.iter().enumerate() {
        let path = format!("/fields/{index}");
        if !crate::registry::valid_id(&field.id)
            || !ids.insert(field.id.as_str())
            || !label(&field.label)
        {
            return Err(ValidationReport::one(
                &path,
                "invalid-or-duplicate-field-id-or-label",
            ));
        }
        match (&field.control, &field.options) {
            (Control::Enumeration, Some(options)) => {
                choices += options.len();
                let mut ids = BTreeSet::new();
                if choices > 2048
                    || options.iter().any(|option| {
                        !crate::registry::valid_id(&option.id)
                            || !ids.insert(&option.id)
                            || !label(&option.label)
                    })
                {
                    return Err(ValidationReport::one(
                        &path,
                        "enum-choice-limit-or-duplicate-or-invalid-id-label",
                    ));
                }
            }
            (Control::Enumeration, None) => {
                return Err(ValidationReport::one(&path, "enum-options-required"));
            }
            (_, Some(_)) => {
                return Err(ValidationReport::one(
                    &path,
                    "options-only-supported-for-enum",
                ));
            }
            (_, None) => {}
        }
    }
    Ok(ids)
}

fn mappings(definition: &Definition) -> Result<(), DefinitionError> {
    let selected: BTreeMap<_, _> = definition
        .sources
        .iter()
        .map(|source| (source.source_id, source))
        .collect();
    let mut distinct = BTreeSet::new();
    for (index, field) in definition.fields.iter().enumerate() {
        let path = format!("/fields/{index}/mapping");
        match &field.mapping {
            Mapping::Unresolved { reason } if !label(reason) => {
                return Err(ValidationReport::one(&path, "unresolved-reason-required"));
            }
            Mapping::Mapped { regions } => {
                for region in regions {
                    if !selected
                        .get(&region.source_id)
                        .is_some_and(|source| source.roles.contains(&SourceRole::Sheet))
                    {
                        return Err(ValidationReport::one(
                            &path,
                            "mapping-needs-selected-sheet-source",
                        ));
                    }
                    distinct.insert(region);
                }
            }
            Mapping::Unresolved { .. } => {}
        }
    }
    if distinct.len() > 1024 {
        return Err(ValidationReport::one("/fields", "mapping-dependency-limit"));
    }
    Ok(())
}

fn domain(definition: &Definition) -> Result<(), DefinitionError> {
    for (path, id) in [
        ("/game_id", &definition.game_id),
        ("/edition_id", &definition.edition_id),
        ("/definition_id", &definition.definition_id),
    ] {
        if !crate::registry::valid_id(id) {
            return Err(ValidationReport::one(path, "invalid-stable-id"));
        }
    }
    if !label(&definition.title) {
        return Err(ValidationReport::one("/title", "title-required"));
    }
    let mut sources = BTreeSet::new();
    for source in &definition.sources {
        if !sources.insert(source.source_id)
            || !source
                .pdf_sha256
                .bytes()
                .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
        {
            return Err(ValidationReport::one(
                "/sources",
                "duplicate-source-or-invalid-lowercase-pdf-hash",
            ));
        }
    }
    let ids = fields(definition)?;
    let mut groups = BTreeSet::new();
    let mut referenced = BTreeSet::new();
    for group in &definition.groups {
        if !crate::registry::valid_id(&group.id)
            || !groups.insert(&group.id)
            || !label(&group.label)
            || group
                .fields
                .iter()
                .any(|id| !ids.contains(id.as_str()) || !referenced.insert(id.as_str()))
        {
            return Err(ValidationReport::one(
                "/groups",
                "invalid-duplicate-group-or-field-reference",
            ));
        }
    }
    if referenced != ids {
        return Err(ValidationReport::one(
            "/groups",
            "every-field-must-appear-exactly-once",
        ));
    }
    mappings(definition)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn example() -> serde_json::Value {
        serde_json::from_slice(include_bytes!("../../schemas/definition-example.json")).unwrap()
    }

    #[test]
    fn canonical_encoding_has_a_frozen_independent_hash_vector_and_null_spelling() {
        let first = validate(include_bytes!("../../schemas/definition-example.json")).unwrap();
        assert_eq!(
            first.hash,
            "89861020e920b01707b467c46b5b6e17bfcccf4bd0687e1f80b863ff91e1ad20"
        );
        let mut second = example();
        second["fields"][0]["help"] = serde_json::Value::Null;
        second["fields"][0]["options"] = serde_json::Value::Null;
        let second = validate(&serde_json::to_vec(&second).unwrap()).unwrap();
        assert_eq!(first.content, second.content);
        assert_eq!(first.hash, second.hash);
    }

    #[test]
    fn source_role_sets_are_normalized_while_presentation_and_choices_remain_ordered() {
        let mut first = example();
        first["sources"] = json!([
            {"source_id":2,"pdf_sha256":"0".repeat(64),"roles":["sheet","core"]},
            {"source_id":1,"pdf_sha256":"1".repeat(64),"roles":["supplement","core"]}
        ]);
        first["fields"][0]["control"] = "enum".into();
        first["fields"][0]["options"] =
            json!([{"id":"one","label":"One"},{"id":"two","label":"Two"}]);
        let mut second = first.clone();
        second["sources"].as_array_mut().unwrap().reverse();
        for source in second["sources"].as_array_mut().unwrap() {
            source["roles"].as_array_mut().unwrap().reverse();
        }
        let first_hash = validate(&serde_json::to_vec(&first).unwrap()).unwrap().hash;
        assert_eq!(
            first_hash,
            validate(&serde_json::to_vec(&second).unwrap())
                .unwrap()
                .hash
        );
        second["fields"][0]["options"]
            .as_array_mut()
            .unwrap()
            .reverse();
        assert_ne!(
            first_hash,
            validate(&serde_json::to_vec(&second).unwrap())
                .unwrap()
                .hash
        );
        let mut unicode = example();
        unicode["fields"][0]["label"] = "界\n\u{0001}".into();
        let admitted = validate(&serde_json::to_vec(&unicode).unwrap()).unwrap();
        assert_eq!(admitted.definition.fields[0].label, "界\n\u{0001}");
        assert_eq!(
            admitted.hash,
            validate(admitted.content.as_bytes()).unwrap().hash
        );
    }

    #[test]
    fn field_and_group_order_is_part_of_the_definition_identity() {
        let mut original = example();
        let mut extra = original["fields"][0].clone();
        extra["id"] = "notes".into();
        original["fields"].as_array_mut().unwrap().push(extra);
        original["groups"]
            .as_array_mut()
            .unwrap()
            .push(json!({"id":"aside","label":"Other","fields":["notes"],"column_hint":"sidebar"}));
        let hash = validate(&serde_json::to_vec(&original).unwrap())
            .unwrap()
            .hash;
        let mut reordered = original.clone();
        reordered["fields"].as_array_mut().unwrap().reverse();
        assert_ne!(
            hash,
            validate(&serde_json::to_vec(&reordered).unwrap())
                .unwrap()
                .hash
        );
        reordered = original;
        reordered["groups"].as_array_mut().unwrap().reverse();
        assert_ne!(
            hash,
            validate(&serde_json::to_vec(&reordered).unwrap())
                .unwrap()
                .hash
        );
    }
}
