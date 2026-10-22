use super::*;
use cel::{ExecutionError, Value};
use serde::Deserialize;
use serde_json::json;
use sha2::{Digest, Sha256};

const FIXTURE: &str = include_str!("../../../schemas/formula-conformance-v1.json");
/// Hash lock of the golden fixture. Changing it requires a reviewed profile decision. Until the
/// first release profile 1 itself may still change (user decision D-Y; content revision 2,
/// assisted sheet slice 1.4); from the first release on, an outcome change, including one under
/// a `cel`/`antlr4rust` bump, means `formula_profile` 2, not an edit here.
const FIXTURE_SHA256: &str = "5af7ce8f1610ca4a41a2a8ecd6b0f387b60c0989a7ef8287f116d5c925cb6b40";

#[derive(Deserialize)]
#[serde(untagged)]
enum DeclarationsRef {
    Named(String),
    Inline(Declarations),
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ExpressionCase {
    id: String,
    declarations: String,
    #[serde(rename = "type")]
    ty: ValueType,
    expr: String,
    #[serde(default)]
    values: Option<Json>,
    expect: Json,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct SheetCase {
    id: String,
    declarations: DeclarationsRef,
    derived: Vec<DerivedSpec>,
    constraints: Vec<ConstraintSpec>,
    values: Json,
    expect: Json,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Fixture {
    version: u32,
    profile: String,
    note: String,
    declarations: BTreeMap<String, Declarations>,
    base_values: BTreeMap<String, Json>,
    expressions: Vec<ExpressionCase>,
    sheets: Vec<SheetCase>,
}

fn fixture() -> Fixture {
    serde_json::from_str(FIXTURE).unwrap()
}

/// Runs one sheet and renders the outcome in the fixture's `expect` vocabulary.
fn outcome(spec: &SheetSpec, values: &Json) -> Json {
    match Compiled::new(spec) {
        Err(rejection) => json!({ "reject": rejection.code }),
        Ok(compiled) => match compiled.evaluate(values) {
            Err(error) => json!({ "input_error": error.code }),
            Ok(evaluation) => {
                let mut rendered = evaluation.to_json();
                rendered["order"] = json!(compiled.derived_order());
                rendered
            }
        },
    }
}

fn merged(base: &Json, overrides: Option<&Json>) -> Json {
    let mut values = base.clone();
    if let (Some(Json::Object(overrides)), Json::Object(target)) = (overrides, &mut values) {
        for (key, value) in overrides {
            target.insert(key.clone(), value.clone());
        }
    }
    values
}

fn expression_spec(fixture: &Fixture, case: &ExpressionCase) -> SheetSpec {
    SheetSpec {
        declarations: fixture.declarations[&case.declarations].clone(),
        derived: vec![DerivedSpec {
            id: "result".into(),
            ty: case.ty.clone(),
            expr: case.expr.clone(),
        }],
        constraints: Vec::new(),
    }
}

fn expression_outcome(fixture: &Fixture, case: &ExpressionCase) -> Json {
    let values = merged(
        &fixture.base_values[&case.declarations],
        case.values.as_ref(),
    );
    let rendered = outcome(&expression_spec(fixture, case), &values);
    match rendered.get("derived") {
        Some(derived) => derived["result"].clone(),
        None => rendered,
    }
}

fn sheet_spec(fixture: &Fixture, case: &SheetCase) -> SheetSpec {
    SheetSpec {
        declarations: match &case.declarations {
            DeclarationsRef::Named(name) => fixture.declarations[name].clone(),
            DeclarationsRef::Inline(declarations) => declarations.clone(),
        },
        derived: case.derived.clone(),
        constraints: case.constraints.clone(),
    }
}

#[test]
fn conformance_fixture_is_hash_locked() {
    let fixture = fixture();
    assert!(fixture.note.contains("hash-locked"));
    let digest = hex::encode(Sha256::digest(FIXTURE.as_bytes()));
    assert_eq!(
        digest, FIXTURE_SHA256,
        "golden fixture changed without a profile review"
    );
}

#[test]
fn conformance_expressions() {
    let fixture = fixture();
    assert_eq!((fixture.version, fixture.profile.as_str()), (1, PROFILE));
    let failures: Vec<String> = fixture
        .expressions
        .iter()
        .filter_map(|case| {
            let actual = expression_outcome(&fixture, case);
            (actual != case.expect).then(|| {
                format!(
                    "{}: expected {} got {actual} ({})",
                    case.id, case.expect, case.expr
                )
            })
        })
        .collect();
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

#[test]
fn conformance_sheets() {
    let fixture = fixture();
    let failures: Vec<String> = fixture
        .sheets
        .iter()
        .filter_map(|case| {
            let actual = outcome(&sheet_spec(&fixture, case), &case.values);
            (actual != case.expect)
                .then(|| format!("{}: expected {} got {actual}", case.id, case.expect))
        })
        .collect();
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

#[test]
fn fixture_covers_every_code_not_tested_below() {
    let fixture = fixture();
    let uncovered: Vec<&str> = Code::ALL
        .iter()
        .map(|code| code.as_str())
        .filter(|code| {
            // Limits and engine-internal paths are covered by the Rust-only tests below.
            !matches!(
                *code,
                "source_too_long"
                    | "depth_exceeded"
                    | "cost_limit"
                    | "collection_too_large"
                    | "input_too_large"
                    | "deadline_exceeded"
                    | "internal_error"
            ) && !FIXTURE.contains(&format!("\"{code}\""))
        })
        .collect();
    assert!(uncovered.is_empty(), "uncovered codes: {uncovered:?}");
    assert!(fixture.expressions.len() + fixture.sheets.len() >= 150);
}

/// Allowlist validation runs on the expanded AST; the macro shapes it recognizes must match
/// the user-written macro calls one-to-one.
#[test]
fn expanded_comprehensions_match_unexpanded_macro_calls() {
    fn count(expr: &cel::IdedExpr, own: &dyn Fn(&cel::IdedExpr) -> bool) -> usize {
        usize::from(own(expr))
            + super::profile::children_for_tests(expr)
                .into_iter()
                .map(|child| count(child, own))
                .sum::<usize>()
    }
    let is_macro = |expr: &cel::IdedExpr| {
        matches!(
            &expr.expr,
            cel::common::ast::Expr::Call(call)
                if call.target.is_some()
                    && matches!(call.func_name.as_str(), "filter" | "map" | "exists" | "all")
        )
    };
    let is_comprehension =
        |expr: &cel::IdedExpr| matches!(&expr.expr, cel::common::ast::Expr::Comprehension(_));
    let fixture = fixture();
    let mut checked = 0;
    for case in &fixture.expressions {
        if Compiled::new(&expression_spec(&fixture, case)).is_err() {
            continue;
        }
        let (raw, expanded) = on_formula_stack(|| {
            let raw = cel::Env::default().parser().parse(&case.expr).unwrap();
            let expanded = ENV.parser().parse(&case.expr).unwrap();
            (count(&raw, &is_macro), count(&expanded, &is_comprehension))
        })
        .unwrap();
        assert_eq!(raw, expanded, "{}", case.id);
        checked += 1;
    }
    assert!(checked > 50);
}

fn basic() -> SheetSpec {
    SheetSpec {
        declarations: fixture().declarations["basic"].clone(),
        ..SheetSpec::default()
    }
}

fn basic_values() -> Json {
    fixture().base_values["basic"].clone()
}

fn derived(expr: &str) -> SheetSpec {
    let mut spec = basic();
    spec.derived.push(DerivedSpec {
        id: "result".into(),
        ty: ValueType::Int,
        expr: expr.into(),
    });
    spec
}

fn reject_code(spec: &SheetSpec) -> Option<Code> {
    Compiled::new(spec).err().map(|rejection| rejection.code)
}

fn eval_one(expr: &str, values: &Json) -> Result<Scalar, Code> {
    let compiled = Compiled::new(&derived(expr)).unwrap();
    compiled.evaluate(values).unwrap().derived[0].1.clone()
}

#[test]
fn source_length_limit_counts_utf8_bytes() {
    let base = "vitality + 1 == 4 ? 1 : 0";
    let at = format!("{base}{}", " ".repeat(MAX_SOURCE_BYTES - base.len()));
    assert!(Compiled::new(&derived(&at)).is_ok());
    assert_eq!(
        reject_code(&derived(&format!("{at} "))),
        Some(Code::SourceTooLong)
    );
    // 1,025 bytes but far fewer characters: the limit is bytes.
    let literal = format!("name == \"{}\" ? 1 : 0 ", "é".repeat(503));
    assert_eq!(literal.len(), 1025);
    assert_eq!(reject_code(&derived(&literal)), Some(Code::SourceTooLong));
}

#[test]
fn parser_and_ast_depth_limits() {
    let parens = |n: usize| format!("{}1{}", "(".repeat(n), ")".repeat(n));
    assert!(Compiled::new(&derived(&parens(8))).is_ok());
    assert_eq!(
        reject_code(&derived(&parens(40))),
        Some(Code::DepthExceeded)
    );
    // CEL builds left-deep binary trees: a flat chain beyond the AST depth bound is rejected.
    let chain = |n: usize| format!("1{}", " + 1".repeat(n - 1));
    assert!(Compiled::new(&derived(&chain(MAX_AST_DEPTH))).is_ok());
    assert_eq!(
        reject_code(&derived(&chain(MAX_AST_DEPTH + 1))),
        Some(Code::DepthExceeded)
    );
}

/// Callers keep the default 2 MiB stack (Tokio blocking threads, test threads); formulas at
/// every admitted limit still compile and evaluate because work runs on the formula stack.
#[test]
fn admitted_limits_compile_and_evaluate_from_a_small_stack() {
    let sources = [
        format!("vitality{}", " + 1".repeat(MAX_AST_DEPTH - 1)),
        format!("{}vitality{}", "(".repeat(30), ")".repeat(30)),
        format!("{}1", "flag ? 1 : ".repeat(30)),
        format!("{}vitality{}", "max(1, ".repeat(20), ")".repeat(20)),
    ];
    let handle = std::thread::Builder::new()
        .stack_size(2 * 1024 * 1024)
        .spawn(move || {
            sources
                .iter()
                .map(|source| {
                    Compiled::new(&derived(source))
                        .map(|compiled| {
                            compiled.evaluate(&basic_values()).unwrap().derived[0]
                                .1
                                .clone()
                        })
                        .map_err(|rejection| rejection.code)
                })
                .collect::<Vec<_>>()
        })
        .unwrap();
    let results = handle.join().expect("no stack overflow");
    assert!(
        results
            .iter()
            .all(|result| matches!(result, Ok(Ok(_)) | Err(Code::DepthExceeded))),
        "{results:?}"
    );
    assert!(matches!(results[0], Ok(Ok(Scalar::Int(_)))), "{results:?}");
}

#[test]
fn ast_depth_walk_is_iterative_and_exact() {
    let (fits, exceeds) = on_formula_stack(|| {
        let expr = ENV
            .parser()
            .parse(&format!("1{}", " + 1".repeat(199)))
            .unwrap();
        // A sum of n terms has depth n: a left-deep chain of n - 1 calls plus a leaf.
        (
            super::profile::depth_exceeds(&expr, 200),
            super::profile::depth_exceeds(&expr, 199),
        )
    })
    .unwrap();
    assert!(!fits && exceeds);
}

#[test]
fn pathological_inputs_are_bounded_on_a_small_stack() {
    let inputs = vec![
        format!("{}1{}", "(".repeat(511), ")".repeat(511)),
        "(".repeat(1024),
        format!("{}true", "!".repeat(1020)),
        format!("{}1", "-".repeat(1020)),
        format!("{}1{}", "[".repeat(511), "]".repeat(511)),
        format!("1{}", "+1".repeat(511)),
        ")(*&^%$#@!".repeat(102),
        format!("{}1", "true ? 1 : ".repeat(93)),
        format!("{}1{}", "f(".repeat(340), ")".repeat(340)),
        format!("{}equipment{}", "size(".repeat(168), ")".repeat(168)),
    ];
    let handle = std::thread::Builder::new()
        .stack_size(2 * 1024 * 1024)
        .spawn(move || {
            inputs
                .iter()
                .map(|input| (input.len(), reject_code(&derived(input))))
                .collect::<Vec<_>>()
        })
        .unwrap();
    let results = handle.join().expect("no stack overflow or panic");
    assert!(
        results.iter().all(|(len, _)| *len <= MAX_SOURCE_BYTES),
        "{results:?}"
    );
    // `!!!…true` and `---…1` collapse in the parser: bounded, so either outcome is fine.
    let codes: Vec<Option<Code>> = results.into_iter().map(|(_, code)| code).collect();
    let deep = Some(Code::DepthExceeded);
    assert_eq!(
        codes,
        vec![
            deep,
            deep,
            Some(Code::TypeMismatch),
            None,
            deep,
            deep,
            Some(Code::ParseError),
            deep,
            deep,
            deep
        ]
    );
}

#[test]
fn list_size_limit_is_shared_with_storage() {
    let mut values = basic_values();
    let items: Vec<Json> = (0..=MAX_LIST_ITEMS)
        .map(|index| {
            json!({"id": format!("i-{index}"), "kind": "tool", "equipped": false, "weight": 1})
        })
        .collect();
    values["equipment"] = Json::Array(items[..MAX_LIST_ITEMS].to_vec());
    assert_eq!(
        eval_one("sum(equipment.map(item, item.weight))", &values),
        Ok(Scalar::Int(256))
    );
    values["equipment"] = Json::Array(items);
    let compiled = Compiled::new(&derived("size(equipment)")).unwrap();
    assert_eq!(
        compiled.evaluate(&values).unwrap_err().code,
        Code::CollectionTooLarge
    );
}

/// Full-length text compared inside a full list: the most expensive admitted pattern.
fn budget_spec(formulas: usize) -> SheetSpec {
    let mut spec = SheetSpec {
        declarations: Declarations {
            fields: vec![FieldDecl {
                id: "note".into(),
                ty: ValueType::Text(MAX_TEXT_CHARS),
                nullable: false,
            }],
            lists: vec![ListDecl {
                id: "pages".into(),
                properties: vec![PropertyDecl {
                    id: "text".into(),
                    ty: ValueType::Text(MAX_TEXT_CHARS),
                    optional: false,
                }],
            }],
        },
        ..SheetSpec::default()
    };
    for index in 0..formulas {
        spec.constraints.push(ConstraintSpec {
            id: format!("c{index}"),
            expr: "pages.exists(item, item.text == note)".into(),
        });
    }
    spec
}

fn largest_admitted_budget_spec() -> SheetSpec {
    let one = Compiled::new(&budget_spec(1)).unwrap().cost();
    let count = usize::try_from(MAX_COST / one).unwrap();
    budget_spec(count)
}

#[test]
fn string_comparison_cost_counts_declared_length() {
    let one = Compiled::new(&budget_spec(1)).unwrap();
    // 256 items x (20,000 / 64 + 1) units for the comparison dominate this formula's cost.
    assert!(one.cost() > 256 * (20_000 / 64));
    let largest = largest_admitted_budget_spec();
    assert!(Compiled::new(&largest).is_ok());
    let over = budget_spec(largest.constraints.len() + 1);
    assert_eq!(reject_code(&over), Some(Code::CostLimit));
}

/// Calibration of [`MAX_COST`]: the largest admitted worst-case definition (budget-bound
/// comparisons of nearly equal text over a full list, at the aggregate input cap). The
/// assertion is on static cost; wall-clock time is only reported (release: see the plan).
#[test]
fn worst_case_admitted_definition_is_cost_bound() {
    let spec = largest_admitted_budget_spec();
    let compiled = Compiled::new(&spec).unwrap();
    assert!(compiled.cost() <= MAX_COST);
    assert!(compiled.cost() > MAX_COST - Compiled::new(&budget_spec(1)).unwrap().cost());
    let text = "x".repeat(4000);
    let mut almost = text.clone();
    almost.pop();
    almost.push('y');
    let pages: Vec<Json> = (0..MAX_LIST_ITEMS)
        .map(|index| json!({"id": format!("p-{index}"), "text": almost}))
        .collect();
    let values = json!({"note": text, "pages": pages});
    let started = Instant::now();
    let evaluation = compiled.evaluate(&values).unwrap();
    eprintln!(
        "calibration: {} formulas, cost {} of {MAX_COST}, evaluated in {:?}",
        spec.constraints.len(),
        compiled.cost(),
        started.elapsed()
    );
    assert!(
        evaluation
            .constraints
            .iter()
            .all(|(_, outcome)| *outcome == Outcome::Violated)
    );
}

#[test]
fn aggregate_input_size_is_bounded() {
    let spec = budget_spec(1);
    let compiled = Compiled::new(&spec).unwrap();
    let pages = |chars: usize| -> Json {
        Json::Array(
            (0..MAX_LIST_ITEMS)
                .map(|index| json!({"id": format!("p-{index}"), "text": "x".repeat(chars)}))
                .collect(),
        )
    };
    // Every value is within its own cap, but the sheet totals about 5 MB of text.
    let oversized = json!({"note": "x".repeat(MAX_TEXT_CHARS), "pages": pages(MAX_TEXT_CHARS)});
    assert_eq!(
        compiled.evaluate(&oversized).unwrap_err().code,
        Code::InputTooLarge
    );
    let fits = json!({"note": "x", "pages": pages(4000)});
    assert!(compiled.evaluate(&fits).is_ok());
}

#[test]
fn list_functions_are_charged_per_list_item() {
    let cost = |expr: &str| Compiled::new(&derived(expr)).unwrap().cost();
    let items = u64::try_from(MAX_LIST_ITEMS).unwrap();
    // Two AST nodes (call + list) plus one unit per possible list item.
    assert_eq!(cost("size(equipment)"), 2 + items);
    let map = cost("size(equipment.map(item, item.weight))") - 2 - items;
    assert_eq!(
        cost("sum(equipment.map(item, item.weight))"),
        2 + items + map
    );
    assert_eq!(
        cost("max_of(equipment.map(item, item.weight), 0)"),
        3 + items + map
    );
    assert_eq!(
        cost("min_of(equipment.map(item, item.weight), 0)"),
        3 + items + map
    );
}

/// Hand-built near-miss comprehensions must not be recognized as `filter`.
#[test]
fn filter_shape_rejects_wrong_accumulator_or_element() {
    use cel::common::ast::Expr;
    fn conditional(expr: &mut cel::IdedExpr) -> &mut cel::common::ast::CallExpr {
        let Expr::Comprehension(comprehension) = &mut expr.expr else {
            panic!("not a comprehension")
        };
        let Expr::Call(call) = &mut comprehension.loop_step.expr else {
            panic!("not a call")
        };
        call
    }
    fn recognized(expr: &cel::IdedExpr) -> bool {
        let Expr::Comprehension(comprehension) = &expr.expr else {
            panic!("not a comprehension")
        };
        super::profile::shape_recognized(comprehension)
    }
    let parsed = on_formula_stack(|| ENV.parser().parse("equipment.filter(item, item.equipped)"))
        .unwrap()
        .unwrap();
    assert!(recognized(&parsed));
    let mut wrong_accumulator = parsed.clone();
    conditional(&mut wrong_accumulator).args[2].expr = Expr::Ident("other".into());
    assert!(!recognized(&wrong_accumulator));
    let mut wrong_element = parsed;
    let append = &mut conditional(&mut wrong_element).args[1];
    let Expr::Call(add) = &mut append.expr else {
        panic!("not an append")
    };
    let Expr::List(list) = &mut add.args[1].expr else {
        panic!("not a list")
    };
    list.elements[0].expr = Expr::Ident("other".into());
    assert!(!recognized(&wrong_element));
}

#[test]
fn deadline_guard_reports_explicit_errors() {
    let mut spec = derived("vitality + 1");
    spec.constraints.push(ConstraintSpec {
        id: "c".into(),
        expr: "true".into(),
    });
    let compiled = Compiled::new(&spec).unwrap();
    let evaluation = compiled
        .evaluate_with_deadline(&basic_values(), Duration::ZERO)
        .unwrap();
    assert_eq!(evaluation.derived[0].1, Err(Code::DeadlineExceeded));
    assert_eq!(
        evaluation.constraints[0].1,
        Outcome::Error(Code::DeadlineExceeded)
    );
}

/// Deterministic differential test of integer arithmetic against a checked Rust reference
/// with CEL's truncating division. Run in debug and with `cargo test --release`, whose
/// profile has no overflow checks.
#[test]
fn integer_arithmetic_matches_checked_reference() {
    const EDGES: [i64; 9] = [
        i64::MIN,
        i64::MIN + 1,
        -3_037_000_500,
        -7,
        -1,
        0,
        1,
        7,
        i64::MAX,
    ];
    let mut state: u64 = 0x9e37_79b9_7f4a_7c15;
    let mut next = move || {
        state ^= state << 13;
        state ^= state >> 7;
        state ^= state << 17;
        state
    };
    let int = |id: &str| FieldDecl {
        id: id.into(),
        ty: ValueType::Int,
        nullable: false,
    };
    let spec = SheetSpec {
        declarations: Declarations {
            fields: vec![int("a"), int("b")],
            lists: Vec::new(),
        },
        derived: ["a + b", "a - b", "a * b", "a / b", "a % b", "-a"]
            .iter()
            .enumerate()
            .map(|(index, expr)| DerivedSpec {
                id: format!("r{index}"),
                ty: ValueType::Int,
                expr: (*expr).into(),
            })
            .collect(),
        constraints: Vec::new(),
    };
    let compiled = Compiled::new(&spec).unwrap();
    let pick = |random: u64| -> i64 {
        let index = usize::try_from(random % 12).unwrap();
        EDGES
            .get(index)
            .copied()
            .unwrap_or_else(|| i64::from_ne_bytes(random.to_ne_bytes()))
    };
    let divide = |a: i64, b: i64, op: fn(i64, i64) -> Option<i64>| {
        if b == 0 {
            Err(Code::DivisionByZero)
        } else {
            op(a, b).ok_or(Code::Overflow)
        }
    };
    for _ in 0..3000 {
        let (a, b) = (pick(next()), pick(next()));
        let expected: [Result<i64, Code>; 6] = [
            a.checked_add(b).ok_or(Code::Overflow),
            a.checked_sub(b).ok_or(Code::Overflow),
            a.checked_mul(b).ok_or(Code::Overflow),
            divide(a, b, i64::checked_div),
            divide(a, b, i64::checked_rem),
            a.checked_neg().ok_or(Code::Overflow),
        ];
        let evaluation = compiled.evaluate(&json!({"a": a, "b": b})).unwrap();
        for ((_, actual), expected) in evaluation.derived.iter().zip(expected) {
            assert_eq!(actual, &expected.map(Scalar::Int), "a={a} b={b}");
        }
    }
}

#[test]
fn json_integers_become_signed_cel_ints() {
    // `cel::to_value` would produce `uint` here and break `n + 1` (the spike pitfall).
    assert_eq!(
        eval_one("vitality + 1", &basic_values()),
        Ok(Scalar::Int(4))
    );
    let mut values = basic_values();
    values["vitality"] = json!(u64::MAX);
    let compiled = Compiled::new(&derived("vitality")).unwrap();
    assert_eq!(
        compiled.evaluate(&values).unwrap_err().code,
        Code::InvalidInput
    );
}

#[test]
fn execution_error_mapping_rows() {
    use super::error::from_execution;
    let one = Value::Int(1);
    let rows: Vec<(ExecutionError, Code)> = vec![
        (
            ExecutionError::Overflow("add", one.clone(), one.clone()),
            Code::Overflow,
        ),
        (
            ExecutionError::DivisionByZero(one.clone()),
            Code::DivisionByZero,
        ),
        (
            ExecutionError::RemainderByZero(one.clone()),
            Code::DivisionByZero,
        ),
        (ExecutionError::no_such_key("armor"), Code::MissingProperty),
        (
            ExecutionError::undeclared_reference("x"),
            Code::UnknownField,
        ),
        (
            ExecutionError::invalid_argument_count(1, 2),
            Code::ArityMismatch,
        ),
        (
            ExecutionError::function_error("sum", "overflow"),
            Code::Overflow,
        ),
        (
            ExecutionError::function_error("clamp", "invalid_argument"),
            Code::InvalidArgument,
        ),
        (
            ExecutionError::function_error("sum", "free text"),
            Code::InternalError,
        ),
        (
            ExecutionError::function_error("other", "overflow"),
            Code::InternalError,
        ),
        (
            ExecutionError::UnsupportedBinaryOperator("add", Value::Null, one.clone()),
            Code::NullValue,
        ),
        (
            ExecutionError::UnsupportedBinaryOperator("add", one.clone(), Value::Bool(true)),
            Code::TypeMismatch,
        ),
        (
            ExecutionError::ValuesNotComparable(Value::Null, one.clone()),
            Code::NullValue,
        ),
        (
            ExecutionError::ValuesNotComparable(one.clone(), Value::Bool(true)),
            Code::TypeMismatch,
        ),
        (
            ExecutionError::no_such_overload("_<_", vec!["null_type".into(), "int".into()]),
            Code::NullValue,
        ),
        (
            ExecutionError::no_such_overload("_<_", vec!["bool".into(), "int".into()]),
            Code::TypeMismatch,
        ),
        (
            ExecutionError::UnexpectedType {
                got: "a".into(),
                want: "b".into(),
            },
            Code::TypeMismatch,
        ),
        (
            ExecutionError::unsupported_target_type(one.clone()),
            Code::TypeMismatch,
        ),
        (
            ExecutionError::IndexOutOfBounds(one.clone()),
            Code::InternalError,
        ),
        (
            ExecutionError::InternalError("x".into()),
            Code::InternalError,
        ),
    ];
    for (error, code) in rows {
        assert_eq!(from_execution(&error), code, "{error:?}");
    }
}

#[test]
fn parse_error_mapping_distinguishes_the_recursion_guard() {
    use super::error::from_parse;
    // The raw parser needs the formula stack in debug builds (see FORMULA_STACK_BYTES).
    let codes = on_formula_stack(|| {
        let deep = format!("{}1{}", "(".repeat(40), ")".repeat(40));
        let deep = ENV
            .parser()
            .max_recursion_depth(MAX_PARSER_DEPTH)
            .parse(&deep)
            .unwrap_err();
        let syntax = ENV.parser().parse("1 +").unwrap_err();
        (from_parse(&deep), from_parse(&syntax))
    })
    .unwrap();
    assert_eq!(codes, (Code::DepthExceeded, Code::ParseError));
}

#[test]
fn code_text_round_trips() {
    for code in Code::ALL {
        assert_eq!(Code::parse(code.as_str()), Some(*code));
    }
}

/// Gate on the pinned engine: any version change fails here and requires the golden fixture
/// review described in the plan (rev 2 review amendments 6, 7 and 17).
#[test]
fn pinned_engine_versions_in_lockfile() {
    let lock = include_str!("../../../Cargo.lock");
    for (name, version) in [("cel", "0.15.0"), ("antlr4rust", "0.6.0")] {
        let entry = format!("name = \"{name}\"\nversion = \"{version}\"\n");
        assert!(
            lock.contains(&entry),
            "{name} must stay pinned at {version}"
        );
        assert_eq!(
            lock.matches(&format!("name = \"{name}\"\n")).count(),
            1,
            "{name}"
        );
    }
}

#[test]
fn compiled_definitions_are_reusable_across_threads() {
    let compiled = Arc::new(Compiled::new(&derived("vitality * 2")).unwrap());
    let handles: Vec<_> = (0..4)
        .map(|_| {
            let compiled = Arc::clone(&compiled);
            std::thread::spawn(move || compiled.evaluate(&basic_values()).unwrap())
        })
        .collect();
    for handle in handles {
        assert_eq!(handle.join().unwrap().derived[0].1, Ok(Scalar::Int(6)));
    }
}

/// Test-only fault injection keyed by `<point>:<formula id>`; ids are unique per test, so
/// parallel tests do not interfere.
pub(super) mod faults {
    use std::{collections::BTreeSet, sync::Mutex};

    static ARMED: Mutex<BTreeSet<String>> = Mutex::new(BTreeSet::new());

    pub(in super::super) fn arm(point: &str, id: &str) {
        ARMED.lock().unwrap().insert(format!("{point}:{id}"));
    }

    pub(in super::super) fn hit(point: &str, id: &str) {
        let armed = ARMED.lock().unwrap().contains(&format!("{point}:{id}"));
        assert!(!armed, "injected fault at {point}:{id}");
    }
}

#[test]
fn panic_boundaries_fail_closed() {
    assert_eq!(guarded::<()>(|| panic!("boom")), Err(Code::InternalError));
    assert_eq!(guarded(|| Ok(1)), Ok(1));
    assert_eq!(
        on_formula_stack::<()>(|| panic!("boom")),
        Err(Code::InternalError)
    );
}

#[test]
fn a_panicking_formula_fails_alone() {
    let mut spec = basic();
    for (id, expr) in [
        ("before", "vitality + 1"),
        ("panic_run_target", "vitality + 2"),
        ("after", "vitality + 3"),
    ] {
        spec.derived.push(DerivedSpec {
            id: id.into(),
            ty: ValueType::Int,
            expr: expr.into(),
        });
    }
    spec.constraints.push(ConstraintSpec {
        id: "still-checked".into(),
        expr: "vitality == 3".into(),
    });
    let compiled = Compiled::new(&spec).unwrap();
    faults::arm("run", "panic_run_target");
    let evaluation = compiled.evaluate(&basic_values()).unwrap();
    assert_eq!(
        evaluation.derived,
        vec![
            ("before".into(), Ok(Scalar::Int(4))),
            ("panic_run_target".into(), Err(Code::InternalError)),
            ("after".into(), Ok(Scalar::Int(6))),
        ]
    );
    assert_eq!(evaluation.constraints[0].1, Outcome::Pass);
}

#[test]
fn a_panic_while_compiling_or_evaluating_fails_closed() {
    let mut spec = derived("vitality");
    spec.derived[0].id = "panic_compile_target".into();
    faults::arm("compile", "panic_compile_target");
    assert_eq!(
        Compiled::new(&spec).err(),
        Some(Rejection {
            code: Code::InternalError,
            target: String::new(),
        })
    );
    let mut spec = derived("vitality");
    spec.derived[0].id = "panic_eval_target".into();
    spec.constraints.push(ConstraintSpec {
        id: "c".into(),
        expr: "true".into(),
    });
    let compiled = Compiled::new(&spec).unwrap();
    faults::arm("evaluate", "panic_eval_target");
    let evaluation = compiled.evaluate(&basic_values()).unwrap();
    assert_eq!(evaluation, compiled.failed_all(Code::InternalError));
    assert_eq!(evaluation.derived[0].1, Err(Code::InternalError));
    assert_eq!(
        evaluation.constraints[0].1,
        Outcome::Error(Code::InternalError)
    );
}

#[test]
fn maximum_depth_compiled_formulas_drop_on_a_small_stack() {
    let sources = [
        format!("vitality{}", " + 1".repeat(MAX_AST_DEPTH - 1)),
        format!("{}vitality{}", "(".repeat(30), ")".repeat(30)),
        format!("{}1", "flag ? 1 : ".repeat(30)),
    ];
    for source in &sources {
        let compiled = Compiled::new(&derived(source)).unwrap();
        std::thread::Builder::new()
            .stack_size(2 * 1024 * 1024)
            .spawn(move || drop(compiled))
            .unwrap()
            .join()
            .expect("no stack overflow while dropping");
    }
    let largest = Compiled::new(&largest_admitted_budget_spec()).unwrap();
    println!(
        "largest admitted budget definition (report-only): cost {}, expression nodes {}",
        largest.cost(),
        largest.node_count()
    );
}

/// `m` reads nullable `luck`; `b` reads `m`; `c` reads `b`; `both` reads `b` and `m`. The
/// declaration order differs from the evaluation order (`m`, `b`, `both`, `c`, `plain`).
fn chain() -> Compiled {
    let mut spec = basic();
    for (id, expr) in [
        ("both", "b + m + vitality"),
        ("c", "b + 1"),
        ("b", "m * 2"),
        ("m", "luck + 1"),
        ("plain", "vitality + 1"),
    ] {
        spec.derived.push(DerivedSpec {
            id: id.into(),
            ty: ValueType::Int,
            expr: expr.into(),
        });
    }
    for (id, expr) in [("b-positive", "b > 0"), ("vitality-cap", "vitality <= 5")] {
        spec.constraints.push(ConstraintSpec {
            id: id.into(),
            expr: expr.into(),
        });
    }
    Compiled::new(&spec).unwrap()
}

fn with(changes: &[(&str, Json)]) -> Json {
    let mut values = basic_values();
    for (id, value) in changes {
        values[*id] = value.clone();
    }
    values
}

#[test]
fn failed_dependency_names_the_failed_dependency_in_evaluation_order() {
    let compiled = chain();
    let evaluation = compiled.evaluate(&basic_values()).unwrap();
    let derived = |id: &str| {
        evaluation
            .derived
            .iter()
            .find(|(item, _)| item == id)
            .unwrap()
            .1
            .clone()
    };
    assert_eq!(
        compiled.derived_order(),
        vec!["m", "b", "both", "c", "plain"]
    );
    assert_eq!(derived("m"), Err(Code::NullValue));
    assert_eq!(derived("b"), Err(Code::DependencyFailed));
    assert_eq!(derived("c"), Err(Code::DependencyFailed));
    assert_eq!(derived("both"), Err(Code::DependencyFailed));
    let failed = |kind, id| compiled.failed_dependency(&evaluation, kind, id);
    assert_eq!(failed(FormulaKind::Derived, "b"), Some("m"));
    assert_eq!(failed(FormulaKind::Derived, "c"), Some("b"));
    // `both` reads `b` and `m`: `m` comes first in evaluation order, although `b` sorts
    // first in the dependency set.
    assert_eq!(failed(FormulaKind::Derived, "both"), Some("m"));
    assert_eq!(failed(FormulaKind::Constraint, "b-positive"), Some("b"));
    // A failure of its own, a value and a constraint outcome name no dependency.
    assert_eq!(failed(FormulaKind::Derived, "m"), None);
    assert_eq!(failed(FormulaKind::Derived, "plain"), None);
    assert_eq!(failed(FormulaKind::Constraint, "vitality-cap"), None);
    // Namespaces are separate: no derived field is called `b-positive`.
    assert_eq!(failed(FormulaKind::Derived, "b-positive"), None);
    assert_eq!(failed(FormulaKind::Constraint, "b"), None);
}

#[test]
fn deadline_cascade_reports_deadline_not_dependency_failed() {
    let compiled = chain();
    let evaluation = compiled
        .evaluate_with_deadline(&with(&[("luck", json!(1))]), Duration::ZERO)
        .unwrap();
    assert!(
        evaluation
            .derived
            .iter()
            .all(|(_, result)| *result == Err(Code::DeadlineExceeded))
    );
    for (id, _) in &evaluation.derived {
        assert_eq!(
            compiled.failed_dependency(&evaluation, FormulaKind::Derived, id),
            None
        );
    }
    assert_eq!(
        compiled.failed_dependency(&evaluation, FormulaKind::Constraint, "b-positive"),
        None
    );
}

#[test]
fn preview_reports_only_changed_derived_values() {
    let compiled = chain();
    let before = with(&[("luck", json!(1))]);
    let after = with(&[("luck", json!(1)), ("vitality", json!(4))]);
    let preview = preview(&compiled, &before, &after).unwrap();
    let changed: Vec<&str> = preview
        .derived
        .iter()
        .map(|item| item.id.as_str())
        .collect();
    // Only `both` and `plain` read `vitality`; changes are in evaluation order.
    assert_eq!(changed, vec!["both", "plain"]);
    assert_eq!(preview.derived[1].before, Ok(Scalar::Int(4)));
    assert_eq!(preview.derived[1].after, Ok(Scalar::Int(5)));
    let same = super::preview(&compiled, &before, &before).unwrap();
    assert!(same.derived.is_empty() && same.constraints.is_empty());
    // Unknown -> known luck turns every `luck` reader from an error into a value.
    let known = super::preview(&compiled, &basic_values(), &before).unwrap();
    let changed: Vec<&str> = known.derived.iter().map(|item| item.id.as_str()).collect();
    assert_eq!(changed, vec!["m", "b", "both", "c"]);
    assert_eq!(known.derived[0].before, Err(Code::NullValue));
}

#[test]
fn preview_reports_new_and_resolved_violations() {
    let compiled = chain();
    let passing = with(&[("luck", json!(1))]);
    let violating = with(&[("luck", json!(1)), ("vitality", json!(6))]);
    let new = preview(&compiled, &passing, &violating).unwrap();
    assert_eq!(new.new_violations(), vec!["vitality-cap"]);
    assert!(new.resolved_violations().is_empty() && new.unknown().is_empty());
    let resolved = preview(&compiled, &violating, &passing).unwrap();
    assert_eq!(resolved.resolved_violations(), vec!["vitality-cap"]);
    assert!(resolved.new_violations().is_empty() && resolved.unknown().is_empty());
    // An error on either side is unknown: never a new or a resolved violation.
    let negative = with(&[("luck", json!(-2))]);
    let violated = preview(&compiled, &passing, &negative).unwrap();
    assert_eq!(violated.new_violations(), vec!["b-positive"]);
    let to_error = preview(&compiled, &negative, &basic_values()).unwrap();
    assert_eq!(to_error.unknown(), vec!["b-positive"], "violated -> error");
    assert!(to_error.new_violations().is_empty() && to_error.resolved_violations().is_empty());
    let from_error = preview(&compiled, &basic_values(), &negative).unwrap();
    assert_eq!(
        from_error.unknown(),
        vec!["b-positive"],
        "error -> violated"
    );
    assert_eq!(from_error.new_violations(), Vec::<&str>::new());
    assert_eq!(from_error.resolved_violations(), Vec::<&str>::new());
    let pass_from_error = preview(&compiled, &basic_values(), &passing).unwrap();
    assert_eq!(
        pass_from_error.unknown(),
        vec!["b-positive"],
        "error -> pass"
    );
    assert_eq!(pass_from_error.resolved_violations(), Vec::<&str>::new());
    // Error -> error, even with a different code, is never a violation.
    let unchanged = preview(&compiled, &basic_values(), &basic_values()).unwrap();
    assert!(
        unchanged.constraints.is_empty(),
        "{:?}",
        unchanged.constraints
    );
}

#[test]
fn preview_rejects_bad_input_on_either_side() {
    let compiled = chain();
    let good = basic_values();
    let bad = with(&[("vitality", json!("three"))]);
    let expected = Err(InputError {
        code: Code::InvalidInput,
        path: "/vitality".into(),
    });
    assert_eq!(
        preview(&compiled, &bad, &good).map(|preview| preview.derived.len()),
        expected
    );
    assert_eq!(
        preview(&compiled, &good, &bad).map(|preview| preview.derived.len()),
        expected
    );
}

/// Report-only sizing of the largest admitted definition by expression nodes: the 256 KiB
/// envelope admits about 240 formulas of 1,024 bytes; a balanced sum of 256 literals is the
/// densest admitted shape (511 nodes in 1,021 bytes, depth 9).
#[test]
fn largest_admitted_definition_ast_size_report() {
    fn balanced(leaves: usize) -> String {
        if leaves == 1 {
            "1".into()
        } else {
            let left = leaves / 2;
            format!("({}+{})", balanced(left), balanced(leaves - left))
        }
    }
    let expr = balanced(256);
    assert!(expr.len() <= MAX_SOURCE_BYTES, "{}", expr.len());
    let mut spec = basic();
    for index in 0..240 {
        spec.derived.push(DerivedSpec {
            id: format!("d{index}"),
            ty: ValueType::Int,
            expr: expr.clone(),
        });
    }
    let compiled = Compiled::new(&spec).unwrap();
    println!(
        "densest admitted definition (report-only): {} formulas, {} expression nodes, \
         size_of::<IdedExpr>() = {} bytes, cost {}",
        compiled.derived_order().len(),
        compiled.node_count(),
        std::mem::size_of::<cel::IdedExpr>(),
        compiled.cost()
    );
}

/// Constraint outcomes of `exprs` over `values`, in order.
fn constraint_outcomes(exprs: &[&str], values: &Json) -> Vec<Outcome> {
    let mut spec = basic();
    spec.constraints = exprs
        .iter()
        .enumerate()
        .map(|(index, expr)| ConstraintSpec {
            id: format!("c{index}"),
            expr: (*expr).into(),
        })
        .collect();
    let compiled = Compiled::new(&spec).unwrap();
    let evaluation = compiled.evaluate(values).unwrap();
    evaluation
        .constraints
        .into_iter()
        .map(|(_, outcome)| outcome)
        .collect()
}

/// Basic values with `vitality` unknown and the coat's `weight` unknown (assisted sheet
/// slice 1.4: `null` is the unknown value of every integer cell).
fn unknown_values() -> Json {
    let mut values = basic_values();
    values["vitality"] = Json::Null;
    values["equipment"][0]["weight"] = Json::Null;
    values
}

/// Slice 1.4: `null` on a non-nullable integer field or an integer item property converts
/// to CEL `Null` instead of refusing the whole evaluation; other types still refuse it.
#[test]
fn unknown_integers_are_converted_and_other_formulas_compute() {
    let values = unknown_values();
    assert_eq!(eval_one("zero + 1", &values), Ok(Scalar::Int(1)));
    assert_eq!(eval_one("size(equipment)", &values), Ok(Scalar::Int(3)));
    assert_eq!(
        eval_one("sum(equipment.map(i, i.armor))", &{
            let mut values = values.clone();
            values["equipment"][2]["armor"] = json!(0);
            values
        }),
        Ok(Scalar::Int(4))
    );
    assert_eq!(eval_one("vitality + 1", &values), Err(Code::NullValue));
    assert_eq!(
        eval_one("sum(equipment.map(i, i.weight))", &values),
        Err(Code::NullValue)
    );
    for (key, value) in [
        ("name", Json::Null),
        ("flag", Json::Null),
        ("kind", Json::Null),
    ] {
        let mut values = basic_values();
        values[key] = value;
        let compiled = Compiled::new(&derived("zero")).unwrap();
        assert_eq!(
            compiled.evaluate(&values).unwrap_err().code,
            Code::InvalidInput,
            "{key}"
        );
    }
    let mut values = basic_values();
    values["equipment"][0]["equipped"] = Json::Null;
    let compiled = Compiled::new(&derived("zero")).unwrap();
    assert_eq!(
        compiled.evaluate(&values).unwrap_err().code,
        Code::InvalidInput
    );
}

/// Slice 1.4 (Sol rev-2 M5): a formula whose result is `null` reports `null_value`, the
/// "a value it uses is unknown (null)" wording, never `type_mismatch`.
#[test]
fn a_null_result_is_null_value_not_type_mismatch() {
    let values = unknown_values();
    assert_eq!(eval_one("vitality", &values), Err(Code::NullValue));
    assert_eq!(
        eval_one(
            "max_of(equipment.filter(i, i.id == \"coat-1\").map(i, i.weight), 0)",
            &values
        ),
        Err(Code::NullValue)
    );
    // A nullable integer while null is unknown too (D-Z).
    assert_eq!(eval_one("luck", &basic_values()), Err(Code::NullValue));
    assert_eq!(
        super::convert::result(Some(&ValueType::Int), &Value::Null),
        Err(Code::NullValue)
    );
    assert_eq!(
        super::convert::result(Some(&ValueType::Bool), &Value::Null),
        Err(Code::NullValue)
    );
}

/// Slice 1.4 (Sonnet m9): in CEL `null == 0` is `false` without an error, so `==`, `!=`,
/// a ternary or a short-circuit over an unknown could pass or fail a check instead of
/// showing "not evaluated". A formula that reads an unknown integer field (nullable or not,
/// D-Z) or integer item property is therefore not run: it reports `null_value`, wherever the read
/// sits. `has()` tests presence, not the value, so an unknown property is present.
#[test]
fn cel_forms_over_an_unknown_are_not_evaluated() {
    let values = unknown_values();
    let not_evaluated = Outcome::Error(Code::NullValue);
    assert_eq!(
        constraint_outcomes(
            &[
                "vitality == 0",
                "vitality != 0",
                "(vitality > 0 ? 1 : 2) == 1",
                "true || vitality == 0",
                "equipment.exists(i, i.weight == 0)",
                "equipment.all(i, i.weight != 0)",
                "equipment.all(i, has(i.weight))",
                "zero == 0",
            ],
            &values
        ),
        vec![
            not_evaluated,
            not_evaluated,
            not_evaluated,
            not_evaluated,
            not_evaluated,
            not_evaluated,
            Outcome::Pass,
            Outcome::Pass,
        ]
    );
    // The same forms over known values evaluate as before.
    assert_eq!(
        constraint_outcomes(
            &[
                "vitality == 0",
                "vitality != 0",
                "equipment.exists(i, i.weight == 0)"
            ],
            &basic_values()
        ),
        vec![Outcome::Violated, Outcome::Pass, Outcome::Violated]
    );
    // D-Z: a nullable integer gets the same guard (`luck` is null in the basic values), so
    // `== null` is no fallback for an unknown integer either.
    let forms = ["luck != 0", "luck in [1, 2]", "luck == null"];
    assert_eq!(
        constraint_outcomes(&forms, &basic_values()),
        vec![not_evaluated; 3]
    );
    assert_eq!(
        eval_one("luck == 0 ? 10 : 20", &basic_values()),
        Err(Code::NullValue)
    );
    let mut known = basic_values();
    known["luck"] = json!(1);
    assert_eq!(
        constraint_outcomes(&forms, &known),
        vec![Outcome::Pass, Outcome::Pass, Outcome::Violated]
    );
    assert_eq!(eval_one("luck == 0 ? 10 : 20", &known), Ok(Scalar::Int(20)));
}
