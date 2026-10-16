//! Read-only change summary of one private sheet revision against the one before it (plan D8).
//!
//! It compares stored data only (derived results are recomputed, never diffed) and names what
//! changed, never the old or new values. List rows are matched by their host IDs, which label
//! row slots, not items (plan D3).
use crate::definitions::{Control, Definition, Field};
use serde_json::Value;
use std::collections::{HashMap, HashSet};

/// At most this many lines are rendered; the rest are counted in the overflow line.
pub(crate) const SUMMARY_LINES: usize = 200;
/// At most this many property labels are listed per edited row.
pub(crate) const LINE_LABELS: usize = 8;

/// The textual address of one list row property, scoped to a sheet and its immutable pin.
pub(crate) fn address(list: &str, entry: &str, property: Option<&str>) -> String {
    match property {
        Some(property) => format!("{list}/{entry}/{property}"),
        None => format!("{list}/{entry}"),
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Kind {
    Created,
    Frozen,
    Name,
    Notes,
    Field,
    Added,
    Removed,
    Edited,
    /// Every property of a retained row of a list with two or more properties changed.
    Reused,
    Moved,
}

/// One rendered line: a field or list label, the main address, and for edited rows the
/// first `LINE_LABELS` changed properties with their addresses.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Line {
    pub kind: Kind,
    pub label: String,
    pub address: String,
    pub properties: Vec<(String, String)>,
    /// Changed properties beyond `properties`.
    pub more: usize,
}

/// Exact per-list counts, always complete.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct Totals {
    pub label: String,
    pub added: usize,
    pub removed: usize,
    pub edited: usize,
    pub moved: usize,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Summary {
    pub revision: i64,
    pub lines: Vec<Line>,
    /// Lines past `SUMMARY_LINES`, not rendered.
    pub hidden: usize,
    pub totals: Vec<Totals>,
}

impl Line {
    /// The line's kind as a template tag.
    pub(crate) fn tag(&self) -> &'static str {
        match self.kind {
            Kind::Created => "created",
            Kind::Frozen => "frozen",
            Kind::Name => "name",
            Kind::Notes => "notes",
            Kind::Field => "field",
            Kind::Added => "added",
            Kind::Removed => "removed",
            Kind::Edited => "edited",
            Kind::Reused => "reused",
            Kind::Moved => "moved",
        }
    }
}

/// What a history page shows above the rows.
pub(crate) enum Changes {
    Summary(Summary),
    /// Why there is no summary.
    Unavailable(&'static str),
}

impl Changes {
    pub(crate) fn summary(&self) -> Option<&Summary> {
        match self {
            Self::Summary(summary) => Some(summary),
            Self::Unavailable(_) => None,
        }
    }
    pub(crate) fn unavailable(&self) -> Option<&str> {
        match self {
            Self::Unavailable(reason) => Some(reason),
            Self::Summary(_) => None,
        }
    }
}

impl Summary {
    pub(crate) fn previous(&self) -> i64 {
        self.revision - 1
    }
    /// Totals of lists with at least one change.
    pub(crate) fn changed_totals(&self) -> impl Iterator<Item = &Totals> {
        self.totals
            .iter()
            .filter(|t| t.added + t.removed + t.edited + t.moved > 0)
    }
}

fn line(kind: Kind, label: &str, address: String) -> Line {
    Line {
        kind,
        label: label.to_owned(),
        address,
        properties: Vec::new(),
        more: 0,
    }
}

fn items(value: &Value) -> Vec<&serde_json::Map<String, Value>> {
    value
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(Value::as_object)
        .collect()
}

fn id(item: &serde_json::Map<String, Value>) -> &str {
    item.get("id").and_then(Value::as_str).unwrap_or_default()
}

/// The positions (indices into `sequence`) outside one longest strictly increasing
/// subsequence of `sequence`. Among the longest subsequences the one whose positions are
/// lexicographically **greatest** is kept, so the positions reported are those that ended up
/// earlier (plan D8, RE3-2). O(n²) with n ≤ 256.
pub(crate) fn outside_increasing(sequence: &[usize]) -> Vec<usize> {
    let n = sequence.len();
    // longest[i]: the longest increasing subsequence starting at position i.
    let mut longest = vec![1_usize; n];
    for i in (0..n).rev() {
        for j in i + 1..n {
            if sequence[j] > sequence[i] {
                longest[i] = longest[i].max(longest[j] + 1);
            }
        }
    }
    let Some(&length) = longest.iter().max() else {
        return Vec::new();
    };
    let mut kept = HashSet::new();
    let mut need = length;
    let mut last: Option<usize> = None;
    while need > 0 {
        let start = last.map_or(0, |last| last + 1);
        let chosen = (start..n)
            .rev()
            .find(|&j| longest[j] == need && last.is_none_or(|last| sequence[j] > sequence[last]));
        let Some(chosen) = chosen else { break };
        kept.insert(chosen);
        last = Some(chosen);
        need -= 1;
    }
    (0..n).filter(|position| !kept.contains(position)).collect()
}

struct ListDiff {
    added: Vec<Line>,
    removed: Vec<Line>,
    edited: Vec<Line>,
    moved: Vec<Line>,
    totals: Totals,
}

fn list_diff(field: &Field, previous: &Value, current: &Value) -> ListDiff {
    let properties = crate::campaigns::values::properties(field);
    let before = items(previous);
    let after = items(current);
    let before_index: HashMap<&str, usize> = before
        .iter()
        .enumerate()
        .map(|(position, item)| (id(item), position))
        .collect();
    let after_ids: HashSet<&str> = after.iter().map(|item| id(item)).collect();
    let row = |entry: &str| address(&field.id, entry, None);
    let mut diff = ListDiff {
        added: Vec::new(),
        removed: Vec::new(),
        edited: Vec::new(),
        moved: Vec::new(),
        totals: Totals {
            label: field.label.clone(),
            ..Totals::default()
        },
    };
    let mut retained = Vec::new();
    for item in &after {
        let entry = id(item);
        let Some(&position) = before_index.get(entry) else {
            diff.added.push(line(Kind::Added, &field.label, row(entry)));
            continue;
        };
        retained.push((entry, position));
        let old = before[position];
        let changed: Vec<_> = properties
            .iter()
            .filter(|property| old.get(&property.id) != item.get(&property.id))
            .collect();
        if changed.is_empty() {
            continue;
        }
        if properties.len() >= 2 && changed.len() == properties.len() {
            diff.edited
                .push(line(Kind::Reused, &field.label, row(entry)));
        } else {
            let mut edited = line(Kind::Edited, &field.label, row(entry));
            edited.properties = changed
                .iter()
                .take(LINE_LABELS)
                .map(|property| {
                    (
                        property.label.clone(),
                        address(&field.id, entry, Some(&property.id)),
                    )
                })
                .collect();
            edited.more = changed.len().saturating_sub(LINE_LABELS);
            diff.edited.push(edited);
        }
    }
    for item in &before {
        let entry = id(item);
        if !after_ids.contains(entry) {
            diff.removed
                .push(line(Kind::Removed, &field.label, row(entry)));
        }
    }
    let positions: Vec<usize> = retained.iter().map(|(_, position)| *position).collect();
    for moved in outside_increasing(&positions) {
        diff.moved
            .push(line(Kind::Moved, &field.label, row(retained[moved].0)));
    }
    diff.totals.added = diff.added.len();
    diff.totals.removed = diff.removed.len();
    diff.totals.edited = diff.edited.len();
    diff.totals.moved = diff.moved.len();
    diff
}

/// The change summary of `current` (revision N) against `previous` (revision N − 1; `None`
/// for revision 1). Both are validated stored snapshots of one sheet and one pin.
pub(crate) fn diff(
    previous: Option<&super::snapshots::PrivateSnapshot>,
    current: &super::snapshots::PrivateSnapshot,
    definition: &Definition,
) -> Summary {
    let mut lines = Vec::new();
    let mut totals = Vec::new();
    let lists = definition
        .fields
        .iter()
        .filter(|field| field.control == Control::List);
    match previous {
        None => {
            let rows: usize = lists
                .clone()
                .map(|field| items(&current.data[&field.id]).len())
                .sum();
            let count = lists.count();
            lines.push(line(
                Kind::Created,
                &format!("{rows} rows in {count} lists"),
                String::new(),
            ));
        }
        Some(_) if current.state == "frozen" => {
            lines.push(line(Kind::Frozen, "", String::new()));
        }
        Some(previous) => {
            if previous.name != current.name {
                lines.push(line(Kind::Name, "", String::new()));
            }
            if previous.notes != current.notes {
                lines.push(line(Kind::Notes, "", String::new()));
            }
            for field in definition
                .fields
                .iter()
                .filter(|field| field.formula.is_none() && field.control != Control::List)
            {
                if previous.data.get(&field.id) != current.data.get(&field.id) {
                    lines.push(line(Kind::Field, &field.label, field.id.clone()));
                }
            }
            for field in lists {
                let list = list_diff(field, &previous.data[&field.id], &current.data[&field.id]);
                lines.extend(list.added);
                lines.extend(list.removed);
                lines.extend(list.edited);
                lines.extend(list.moved);
                totals.push(list.totals);
            }
        }
    }
    let hidden = lines.len().saturating_sub(SUMMARY_LINES);
    lines.truncate(SUMMARY_LINES);
    Summary {
        revision: current.revision,
        lines,
        hidden,
        totals,
    }
}

#[cfg(test)]
mod tests {
    use super::{Kind, Line, address, diff, outside_increasing};
    use crate::campaigns::snapshots::PrivateSnapshot;
    use serde_json::{Value, json};

    fn definition() -> crate::definitions::Definition {
        let pin = crate::definitions::private_pin::for_tests(
            &crate::campaigns::tests::list_tests::list_definition(),
        );
        pin.definition().clone()
    }

    fn snapshot(revision: i64, data: Value) -> PrivateSnapshot {
        PrivateSnapshot {
            encoding: 1,
            sheet_id: 1,
            campaign_id: 1,
            game_id: "synthetic-game".into(),
            edition_id: "first".into(),
            custodian_id: 2,
            kind: "runtime".into(),
            pin: crate::campaigns::snapshots::Pin::Runtime {
                version_id: 1,
                version: 1,
                content_hash: "0".repeat(64),
                schema: 1,
            },
            revision,
            state: "active".into(),
            name: "Sheet".into(),
            notes: String::new(),
            data,
        }
    }

    fn item(id: &str, name: &str) -> Value {
        json!({"id": id, "name": name, "quantity": 1, "equipped": false})
    }

    fn sheet(equipment: Vec<Value>, tags: Vec<Value>) -> Value {
        json!({"label": "x", "equipment": Value::Array(equipment), "tags": Value::Array(tags)})
    }

    fn kinds(lines: &[Line]) -> Vec<(Kind, &str)> {
        lines
            .iter()
            .map(|line| (line.kind, line.address.as_str()))
            .collect()
    }

    #[test]
    fn history_address_grammar() {
        assert_eq!(
            address("equipment", "r1-2", Some("quantity")),
            "equipment/r1-2/quantity"
        );
        assert_eq!(address("equipment", "r1-2", None), "equipment/r1-2");
    }

    #[test]
    fn diff_added_removed_edited_scalars_name_notes() {
        let definition = definition();
        let before = snapshot(1, sheet(vec![item("r1-1", "A"), item("r1-2", "B")], vec![]));
        let mut changed_b = item("r1-2", "B");
        changed_b["quantity"] = 5.into();
        let mut after = snapshot(
            2,
            sheet(
                vec![changed_b, item("r2-1", "New")],
                vec![json!({"id": "r2-1", "tag": "t"})],
            ),
        );
        after.name = "Renamed".into();
        after.notes = "Notes".into();
        after.data["label"] = "y".into();
        let summary = diff(Some(&before), &after, &definition);
        assert_eq!(
            kinds(&summary.lines),
            [
                (Kind::Name, ""),
                (Kind::Notes, ""),
                (Kind::Field, "label"),
                (Kind::Added, "equipment/r2-1"),
                (Kind::Removed, "equipment/r1-1"),
                (Kind::Edited, "equipment/r1-2"),
                (Kind::Added, "tags/r2-1"),
            ]
        );
        let edited = &summary.lines[5];
        assert_eq!(
            edited.properties,
            [("Quantity".to_owned(), "equipment/r1-2/quantity".to_owned())]
        );
        assert_eq!(summary.totals[0].added, 1);
        assert_eq!(summary.totals[0].removed, 1);
        assert_eq!(summary.totals[0].edited, 1);
        assert_eq!(summary.totals[0].moved, 0, "r1-2 is the only retained row");
        assert_eq!(summary.hidden, 0);
    }

    #[test]
    fn diff_optional_presence_is_an_edit() {
        let definition = definition();
        let before = snapshot(1, sheet(vec![item("r1-1", "A")], vec![]));
        let mut set = item("r1-1", "A");
        set["armor"] = 0.into();
        let after = snapshot(2, sheet(vec![set.clone()], vec![]));
        let summary = diff(Some(&before), &after, &definition);
        assert_eq!(kinds(&summary.lines), [(Kind::Edited, "equipment/r1-1")]);
        assert_eq!(summary.lines[0].properties[0].1, "equipment/r1-1/armor");
        // And back: set → not set.
        let unset = diff(Some(&after), &snapshot(3, before.data.clone()), &definition);
        assert_eq!(kinds(&unset.lines), [(Kind::Edited, "equipment/r1-1")]);
    }

    fn moved(before: &[&str], after: &[&str]) -> Vec<String> {
        let definition = definition();
        let rows = |ids: &[&str]| ids.iter().map(|id| item(id, "same")).collect();
        let summary = diff(
            Some(&snapshot(1, sheet(rows(before), vec![]))),
            &snapshot(2, sheet(rows(after), vec![])),
            &definition,
        );
        summary
            .lines
            .iter()
            .filter(|line| line.kind == Kind::Moved)
            .map(|line| line.address.trim_start_matches("equipment/").to_owned())
            .collect()
    }

    #[test]
    fn diff_moved_is_lis_with_deterministic_ties() {
        // One row to the top reports exactly that row.
        assert_eq!(moved(&["a", "b", "c", "d"], &["d", "a", "b", "c"]), ["d"]);
        // One row to the bottom by two or more places reports that row.
        assert_eq!(moved(&["a", "b", "c"], &["b", "c", "a"]), ["a"]);
        // RE3-2 ties: the row that ended up earlier is reported.
        assert_eq!(moved(&["a", "b"], &["b", "a"]), ["b"]);
        assert_eq!(moved(&["a", "b", "c"], &["b", "a", "c"]), ["b"]);
        assert_eq!(moved(&["a", "b", "c"], &["a", "c", "b"]), ["c"]);
        // No net change, and a full reversal (all but one moved, the last kept).
        assert!(moved(&["a", "b", "c"], &["a", "b", "c"]).is_empty());
        assert_eq!(moved(&["a", "b", "c"], &["c", "b", "a"]), ["c", "b"]);
        // Removals and additions do not count as moves.
        assert!(moved(&["a", "b", "c"], &["x", "a", "c"]).is_empty());
        assert_eq!(outside_increasing(&[]), Vec::<usize>::new());
        assert_eq!(outside_increasing(&[2, 0, 1]), [0]);
    }

    #[test]
    fn diff_reused_slot_all_properties_changed() {
        let definition = definition();
        let before = snapshot(1, sheet(vec![item("r1-1", "A")], vec![]));
        let after = snapshot(
            2,
            sheet(
                vec![
                    json!({"id": "r1-1", "name": "Other", "quantity": 9, "armor": 1,
                            "kind": "light", "equipped": true}),
                ],
                vec![],
            ),
        );
        let summary = diff(Some(&before), &after, &definition);
        assert_eq!(kinds(&summary.lines), [(Kind::Reused, "equipment/r1-1")]);
        assert!(summary.lines[0].properties.is_empty());
    }

    #[test]
    fn diff_one_property_list_change_is_a_plain_edit() {
        let definition = definition();
        let before = snapshot(1, sheet(vec![], vec![json!({"id": "r1-1", "tag": "a"})]));
        let after = snapshot(2, sheet(vec![], vec![json!({"id": "r1-1", "tag": "b"})]));
        let summary = diff(Some(&before), &after, &definition);
        assert_eq!(kinds(&summary.lines), [(Kind::Edited, "tags/r1-1")]);
        assert_eq!(summary.lines[0].properties[0].1, "tags/r1-1/tag");
    }

    #[test]
    fn diff_create_and_freeze_lines() {
        let definition = definition();
        let created = snapshot(
            1,
            sheet(
                vec![item("r1-1", "A"), item("r1-2", "B")],
                vec![json!({"id": "r1-1", "tag": "t"})],
            ),
        );
        let summary = diff(None, &created, &definition);
        assert_eq!(summary.lines.len(), 1);
        assert_eq!(summary.lines[0].kind, Kind::Created);
        assert_eq!(summary.lines[0].label, "3 rows in 2 lists");
        let mut frozen = created.clone();
        frozen.revision = 2;
        frozen.state = "frozen".into();
        let summary = diff(Some(&created), &frozen, &definition);
        assert_eq!(kinds(&summary.lines), [(Kind::Frozen, "")]);
    }
}
