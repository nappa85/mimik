//! Unit tests of the list units' pure parts (slice 1.5b).
use super::{MAX_ROW_NOTES, new_item};
use serde_json::json;
use std::collections::BTreeMap;

/// A list of `properties` integer properties, built without the definition decoder (which
/// admits at most 32 properties), so the new-row note cap is reachable.
fn integers(properties: usize) -> crate::definitions::Field {
    let properties: Vec<_> = (0..properties)
        .map(|p| json!({"id": format!("p{p}"), "label": "P", "control": "integer", "optional": false}))
        .collect();
    serde_json::from_value(json!({
        "id": "list", "label": "List", "control": "list", "nullable": false,
        "mapping": {"state": "unresolved", "reason": "test"},
        "items": {"properties": properties},
    }))
    .unwrap()
}

/// Every property unreadable: one `unresolved` note each.
fn scribbled(properties: usize) -> BTreeMap<String, String> {
    (0..properties)
        .map(|p| (format!("row-{p}"), "lots".to_owned()))
        .collect()
}

#[test]
fn the_new_row_note_cap_is_the_property_cap() {
    assert_eq!(MAX_ROW_NOTES, crate::definitions::formula::MAX_PROPERTIES);
    let current = BTreeMap::new();
    let (item, notes) = new_item(&integers(32), "r2-1", &scribbled(32), &current).unwrap();
    assert_eq!(notes.len(), 32);
    assert_eq!(item["p31"], json!(null), "unknown");
    let refused = new_item(&integers(33), "r2-1", &scribbled(33), &current);
    assert!(
        matches!(refused, Err(crate::AppError::Invalid(reason)) if reason.contains("more than 32")),
        "33 notes are refused"
    );
}
