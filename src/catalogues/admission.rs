//! Catalogue admission: decode profile, trusted schema, serde, domain checks, then canonical
//! encoding and the content hash (plan D2), mirroring `definitions::admission`.
use super::{
    Catalogue, CatalogueCounts, CatalogueError, PropertyControl, PropertyValue, decode, invalid,
    report,
};
use crate::definitions::{Diagnostic, Region};
use sha2::{Digest, Sha256};
use std::{
    collections::{BTreeMap, BTreeSet},
    sync::LazyLock,
};

/// Distinct `(source, page, unit)` references in one catalogue (D5).
pub(super) const MAX_DISTINCT_UNITS: usize = 16_384;
/// Total UTF-8 bytes of every entry's owner text (plan §9).
pub(super) const MAX_OWNER_TEXT_TOTAL: usize = 1024 * 1024;

static VALIDATOR: LazyLock<Result<jsonschema::Validator, ()>> = LazyLock::new(|| {
    let schema: serde_json::Value =
        serde_json::from_str(include_str!("../../schemas/catalogue-v1.json")).map_err(|_| ())?;
    jsonschema::draft202012::options()
        .offline()
        .with_pattern_options(jsonschema::PatternOptions::regex())
        .build(&schema)
        .map_err(|_| ())
});

pub(super) struct Admitted {
    pub catalogue: Catalogue,
    pub content: String,
    pub hash: String,
    pub counts: CatalogueCounts,
}

pub(super) fn hash(bytes: &[u8]) -> String {
    let mut hash = Sha256::new();
    hash.update(b"mimik-catalogue-v1\0");
    hash.update(bytes);
    hex::encode(hash.finalize())
}

/// Envelope member names that may appear in a diagnostic path. Any other segment is
/// author-chosen (a property id or an unknown member) and is replaced by `*`.
const MEMBERS: &[&str] = &[
    "contract_version",
    "game_id",
    "edition_id",
    "catalogue_id",
    "title",
    "sources",
    "source_id",
    "pdf_sha256",
    "entry_types",
    "id",
    "label",
    "properties",
    "control",
    "options",
    "entries",
    "key",
    "type",
    "name",
    "retired",
    "description",
    "units",
    "page_id",
    "unit_id",
    "owner_text",
];

fn redacted_path(path: &str) -> String {
    path.split('/')
        .map(|segment| {
            if segment.is_empty()
                || segment.bytes().all(|byte| byte.is_ascii_digit())
                || MEMBERS.contains(&segment)
            {
                segment
            } else {
                "*"
            }
        })
        .collect::<Vec<_>>()
        .join("/")
}

/// Admit a catalogue envelope. Every rejection is a static code with an index-only path.
pub(super) fn validate(bytes: &[u8]) -> Result<Admitted, CatalogueError> {
    let value = decode::parse_catalogue(bytes)?;
    let validator = VALIDATOR
        .as_ref()
        .map_err(|()| CatalogueError::Configuration)?;
    let mut errors = validator.iter_errors(&value);
    let diagnostics: Vec<Diagnostic> = errors
        .by_ref()
        .take(32)
        .map(|error| Diagnostic {
            path: redacted_path(error.instance_path().as_str()),
            code: error.kind().keyword().into(),
        })
        .collect();
    let truncated = errors.next().is_some();
    drop(errors);
    if !diagnostics.is_empty() {
        return Err(CatalogueError::Invalid(report(diagnostics, truncated)));
    }
    let mut catalogue: Catalogue =
        serde_json::from_value(value).map_err(|_| CatalogueError::Configuration)?;
    let counts = domain(&mut catalogue)?;
    catalogue.sources.sort_by_key(|source| source.source_id);
    let content = serde_json::to_string(&catalogue).map_err(|_| CatalogueError::Configuration)?;
    if content.len() > decode::MAX_BYTES {
        return Err(invalid("/", "canonical-byte-limit"));
    }
    let hash = hash(content.as_bytes());
    Ok(Admitted {
        catalogue,
        content,
        hash,
        counts,
    })
}

fn label(value: &str) -> bool {
    !value.trim().is_empty()
}

fn types(catalogue: &Catalogue) -> Result<BTreeMap<&str, usize>, CatalogueError> {
    let mut ids = BTreeMap::new();
    for (index, entry_type) in catalogue.entry_types.iter().enumerate() {
        let path = format!("/entry_types/{index}");
        if !crate::registry::valid_id(&entry_type.id)
            || ids.insert(entry_type.id.as_str(), index).is_some()
            || !label(&entry_type.label)
        {
            return Err(invalid(&path, "invalid-or-duplicate-type-id-or-label"));
        }
        let mut properties = BTreeSet::new();
        for (number, property) in entry_type.properties.iter().enumerate() {
            let path = format!("{path}/properties/{number}");
            if !crate::registry::valid_id(&property.id)
                || !properties.insert(property.id.as_str())
                || !label(&property.label)
            {
                return Err(invalid(&path, "invalid-or-duplicate-property-id-or-label"));
            }
            match (property.control, &property.options) {
                (PropertyControl::Enumeration, Some(options)) => {
                    let mut choices = BTreeSet::new();
                    if options.iter().any(|option| {
                        !crate::registry::valid_id(&option.id)
                            || !choices.insert(option.id.as_str())
                            || !label(&option.label)
                    }) {
                        return Err(invalid(&path, "invalid-or-duplicate-option-id-or-label"));
                    }
                }
                (PropertyControl::Enumeration, None) => {
                    return Err(invalid(&path, "enum-options-required"));
                }
                (_, Some(_)) => return Err(invalid(&path, "options-only-supported-for-enum")),
                (_, None) => {}
            }
        }
    }
    Ok(ids)
}

/// Reorder `entry`'s properties into its type's declared order, checking the exact key set
/// and each value against its control.
fn properties(
    catalogue: &Catalogue,
    type_index: usize,
    entry: &mut super::Entry,
    path: &str,
) -> Result<(), CatalogueError> {
    let declared = &catalogue.entry_types[type_index].properties;
    let mut given: BTreeMap<String, PropertyValue> = std::mem::take(&mut entry.properties.0)
        .into_iter()
        .collect();
    let mut ordered = Vec::with_capacity(declared.len());
    for property in declared {
        let value = given
            .remove(&property.id)
            .ok_or_else(|| invalid(path, "property-key-set-mismatch"))?;
        let matches = match (&property.control, &value) {
            (PropertyControl::Text, PropertyValue::Text(_))
            | (PropertyControl::Integer, PropertyValue::Integer(_))
            | (PropertyControl::Boolean, PropertyValue::Boolean(_)) => true,
            (PropertyControl::Enumeration, PropertyValue::Text(choice)) => property
                .options
                .as_ref()
                .is_some_and(|options| options.iter().any(|option| option.id == *choice)),
            _ => false,
        };
        if !matches {
            return Err(invalid(path, "property-value-control-mismatch"));
        }
        ordered.push((property.id.clone(), value));
    }
    if !given.is_empty() {
        return Err(invalid(path, "property-key-set-mismatch"));
    }
    entry.properties.0 = ordered;
    Ok(())
}

fn domain(catalogue: &mut Catalogue) -> Result<CatalogueCounts, CatalogueError> {
    for (path, id) in [
        ("/game_id", &catalogue.game_id),
        ("/edition_id", &catalogue.edition_id),
        ("/catalogue_id", &catalogue.catalogue_id),
    ] {
        if !crate::registry::valid_id(id) {
            return Err(invalid(path, "invalid-stable-id"));
        }
    }
    if !label(&catalogue.title) {
        return Err(invalid("/title", "title-required"));
    }
    let mut sources = BTreeSet::new();
    for source in &catalogue.sources {
        if !sources.insert(source.source_id)
            || !source
                .pdf_sha256
                .bytes()
                .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
        {
            return Err(invalid(
                "/sources",
                "duplicate-source-or-invalid-lowercase-pdf-hash",
            ));
        }
    }
    let type_ids: BTreeMap<String, usize> = types(catalogue)?
        .into_iter()
        .map(|(id, index)| (id.to_owned(), index))
        .collect();
    let mut counts = CatalogueCounts {
        sources: catalogue.sources.len(),
        entry_types: catalogue.entry_types.len(),
        entries: catalogue.entries.len(),
        ..CatalogueCounts::default()
    };
    let mut keys = BTreeSet::new();
    let mut distinct: BTreeSet<Region> = BTreeSet::new();
    let mut owner_bytes = 0_usize;
    // Taken out so each entry can be reordered against the declared types; on rejection the
    // whole catalogue is discarded.
    let mut entries = std::mem::take(&mut catalogue.entries);
    for (index, entry) in entries.iter_mut().enumerate() {
        let path = format!("/entries/{index}");
        if !crate::registry::valid_id(&entry.key) || !keys.insert(entry.key.clone()) {
            return Err(invalid(&path, "invalid-or-duplicate-entry-key"));
        }
        if !label(&entry.name) {
            return Err(invalid(&path, "entry-name-required"));
        }
        let type_index = *type_ids
            .get(&entry.entry_type)
            .ok_or_else(|| invalid(&path, "entry-type-not-declared"))?;
        properties(catalogue, type_index, entry, &format!("{path}/properties"))?;
        counts.retired += usize::from(entry.retired);
        let path = format!("{path}/description");
        match (&entry.description.units, &entry.description.owner_text) {
            (Some(units), None) if (1..=decode::MAX_UNITS).contains(&units.len()) => {
                let mut own = BTreeSet::new();
                for unit in units {
                    if !own.insert(unit) {
                        return Err(invalid(&path, "duplicate-unit-in-entry"));
                    }
                    if !sources.contains(&unit.source_id) {
                        return Err(invalid(&path, "unit-source-not-listed"));
                    }
                    distinct.insert(unit.clone());
                }
                counts.unit_references += units.len();
            }
            (None, Some(text)) if label(text) && text.chars().count() <= decode::MAX_OWNER_TEXT => {
                owner_bytes = owner_bytes.saturating_add(text.len());
                counts.owner_written += 1;
            }
            _ => return Err(invalid(&path, "description-needs-units-xor-owner-text")),
        }
    }
    catalogue.entries = entries;
    if distinct.len() > MAX_DISTINCT_UNITS {
        return Err(invalid("/entries", "distinct-unit-limit"));
    }
    if owner_bytes > MAX_OWNER_TEXT_TOTAL {
        return Err(invalid("/entries", "owner-text-total-limit"));
    }
    counts.distinct_units = distinct.len();
    Ok(counts)
}
