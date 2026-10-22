//! Read sets (assisted sheet plan §1.4, "Read sets"): per derived value and check, scalar
//! reads and `(list, property)` reads grouped by comprehension binder, closed through
//! derived dependencies.
use super::*;
use serde_json::json;

fn spec(derived: &[(&str, &str)], constraints: &[(&str, &str)]) -> SheetSpec {
    let declarations: Declarations = serde_json::from_value(json!({
        "fields": [
            {"id": "hp", "type": "int"}, {"id": "max_hp", "type": "int"},
            {"id": "armour", "type": "int"}, {"id": "luck", "type": "int"},
        ],
        "lists": [{"id": "gear", "properties": [
            {"id": "a", "type": "int"}, {"id": "b", "type": "int"},
            {"id": "c", "type": "int"}, {"id": "broken", "type": "bool", "optional": true},
        ]}],
    }))
    .unwrap();
    SheetSpec {
        declarations,
        derived: derived
            .iter()
            .map(|(id, expr)| DerivedSpec {
                id: (*id).into(),
                ty: ValueType::Int,
                expr: (*expr).into(),
            })
            .collect(),
        constraints: constraints
            .iter()
            .map(|(id, expr)| ConstraintSpec {
                id: (*id).into(),
                expr: (*expr).into(),
            })
            .collect(),
    }
}

fn sets(
    derived: &[(&str, &str)],
    constraints: &[(&str, &str)],
) -> Vec<(FormulaKind, String, ReadSet)> {
    Compiled::new(&spec(derived, constraints))
        .unwrap()
        .read_sets()
        .into_iter()
        .map(|(kind, id, set)| (kind, id.to_owned(), set))
        .collect()
}

fn one<'a>(all: &'a [(FormulaKind, String, ReadSet)], id: &str) -> &'a ReadSet {
    &all.iter().find(|(_, name, _)| name == id).unwrap().2
}

fn names(set: &std::collections::BTreeSet<String>) -> Vec<&str> {
    set.iter().map(String::as_str).collect()
}

#[test]
fn scalar_reads_per_derived_value_and_check() {
    let all = sets(&[("total", "hp + armour")], &[("cap", "hp <= max_hp")]);
    assert_eq!(names(&one(&all, "total").scalars), ["armour", "hp"]);
    assert_eq!(names(&one(&all, "cap").scalars), ["hp", "max_hp"]);
    assert_eq!(all[0].0, FormulaKind::Derived);
    assert_eq!(all[1].0, FormulaKind::Constraint);
}

#[test]
fn closure_through_derived_dependencies() {
    let all = sets(
        &[
            ("load", "sum(gear.map(i, i.a * i.b))"),
            ("spare", "max_hp - load"),
        ],
        &[("ok", "spare >= luck")],
    );
    let ok = one(&all, "ok");
    assert_eq!(names(&ok.scalars), ["luck", "max_hp"]);
    assert!(ok.reads_property("gear", "a"));
    assert_eq!(names(&ok.co_read_properties("gear", "a")), ["b"]);
}

#[test]
fn one_comprehension_co_reads_and_two_do_not() {
    let all = sets(
        &[
            ("same", "sum(gear.map(item, item.a + item.b))"),
            (
                "apart",
                "sum(gear.map(item, item.a)) + sum(gear.map(item, item.c))",
            ),
        ],
        &[],
    );
    let same = one(&all, "same");
    assert_eq!(names(&same.co_read_properties("gear", "a")), ["b"]);
    let apart = one(&all, "apart");
    assert!(apart.reads_property("gear", "a") && apart.reads_property("gear", "c"));
    assert!(apart.co_read_properties("gear", "a").is_empty());
    assert_eq!(apart.iterations.len(), 2);
}

#[test]
fn a_derived_iteration_and_its_dependent_iteration_stay_two() {
    let all = sets(
        &[("first", "sum(gear.map(item, item.a))")],
        &[("second", "first + sum(gear.map(item, item.c)) >= 0")],
    );
    let second = one(&all, "second");
    assert_eq!(second.iterations.len(), 2);
    assert!(second.co_read_properties("gear", "a").is_empty());
    assert!(second.co_read_properties("gear", "c").is_empty());
}

#[test]
fn binders_are_unique_across_the_definition() {
    let all = sets(
        &[
            ("x", "sum(gear.map(item, item.a))"),
            ("y", "sum(gear.map(item, item.b))"),
        ],
        &[("z", "gear.all(item, item.c >= 0)")],
    );
    let mut binders: Vec<u32> = all
        .iter()
        .flat_map(|(_, _, set)| set.iterations.keys().copied())
        .collect();
    binders.sort_unstable();
    binders.dedup();
    assert_eq!(binders.len(), 3);
}

#[test]
fn a_presence_test_is_a_co_read_but_not_an_item_read() {
    let spec = spec(
        &[(
            "usable",
            "sum(gear.filter(i, !has(i.broken)).map(j, j.a)) + size(gear.filter(k, has(k.broken) && k.b > 0))",
        )],
        &[],
    );
    let compiled = Compiled::new(&spec).unwrap();
    let set = &compiled.read_sets()[0].2;
    assert_eq!(names(&set.co_read_properties("gear", "b")), ["broken"]);
    // The unknown guard's item reads are unchanged: `has()` is not a value read.
    let item_reads = &compiled.derived[0].item_reads;
    assert!(!item_reads.contains(&("gear".to_owned(), "broken".to_owned())));
    assert!(item_reads.contains(&("gear".to_owned(), "b".to_owned())));
}

#[test]
fn the_unknown_guard_is_unchanged() {
    let compiled = Compiled::new(&spec(
        &[("load", "sum(gear.map(i, i.a))")],
        &[("cap", "hp <= max_hp")],
    ))
    .unwrap();
    let values = json!({"hp": 1, "max_hp": null, "armour": 0, "luck": 0,
        "gear": [{"id": "r1-1", "a": null, "b": 1, "c": 1}]});
    let evaluation = compiled.evaluate(&values).unwrap();
    assert_eq!(evaluation.derived[0].1, Err(Code::NullValue));
    assert_eq!(evaluation.constraints[0].1, Outcome::Error(Code::NullValue));
}

#[test]
fn a_filter_then_map_over_one_list_is_one_iteration() {
    let all = sets(
        &[
            ("kept", "sum(gear.filter(i, i.c > 0).map(j, j.a))"),
            (
                "twice",
                "sum(gear.filter(i, i.c > 0).filter(k, k.b > 0).map(j, j.a))",
            ),
        ],
        &[("some", "gear.filter(i, !has(i.broken)).exists(j, j.b > 0)")],
    );
    assert_eq!(
        names(&one(&all, "kept").co_read_properties("gear", "a")),
        ["c"]
    );
    assert_eq!(one(&all, "kept").iterations.len(), 1);
    assert_eq!(
        names(&one(&all, "twice").co_read_properties("gear", "a")),
        ["b", "c"]
    );
    assert_eq!(
        names(&one(&all, "some").co_read_properties("gear", "b")),
        ["broken"]
    );
    // Two comprehensions that are not chained stay two iterations.
    let apart = sets(
        &[("x", "sum(gear.map(i, i.a)) + size(gear.filter(j, j.c > 0))")],
        &[],
    );
    assert!(one(&apart, "x").co_read_properties("gear", "a").is_empty());
}
