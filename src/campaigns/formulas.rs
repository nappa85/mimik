//! Advisory presentation of derived values and constraint checks on runtime sheets.
//!
//! Pure view model: it reads an [`Evaluation`] of stored data and never writes. Results are
//! advisory; they never block a save and are not a legality ruling.
use crate::definitions::{
    Control, Definition,
    formula::{Code, Evaluation, FormulaKind, InputError, Outcome, Scalar},
    private_pin::ImmutablePin,
};

/// Wording for codes that only admission can produce. Evaluation never reaches them; showing
/// it means a pin or engine invariant was broken.
const UNEXPECTED: &str = "unexpected formula state; report this sheet";

/// Human wording for every code. Exhaustive with no wildcard, so a new code needs reviewed
/// wording before it compiles.
pub(super) fn message(code: Code) -> &'static str {
    match code {
        Code::InvalidInput => "the saved values do not match the formula declarations",
        Code::InputTooLarge => "the saved values exceed the formula input size limit",
        Code::CollectionTooLarge => "a list has more items than formulas accept",
        Code::MissingProperty => "an optional item property it reads is absent",
        Code::NullValue => "a value it uses is unknown (null)",
        Code::TypeMismatch => "a value has a type the formula cannot use",
        Code::Overflow => "the integer result is out of range",
        Code::DivisionByZero => "it divides by zero",
        Code::InvalidArgument => "a function received an argument outside its accepted range",
        Code::InvalidResult => "the result is not a valid value for this field",
        Code::UnknownField => "it reads a field that is not available",
        Code::ArityMismatch => "a function was called with the wrong number of arguments",
        Code::DependencyFailed => "a derived value it uses could not be computed",
        Code::DeadlineExceeded => "the time limit was reached. Reload to retry",
        Code::InternalError => "an internal error occurred. Reload to retry",
        Code::ParseError
        | Code::SourceTooLong
        | Code::DepthExceeded
        | Code::NestedAggregate
        | Code::UnsupportedConstruct
        | Code::UnknownProperty
        | Code::UnknownEnumOption
        | Code::InvalidIdentifier
        | Code::ReservedIdentifier
        | Code::DuplicateIdentifier
        | Code::InvalidIterationVariable
        | Code::InvalidDeclaration
        | Code::FormulaCycle
        | Code::CostLimit => UNEXPECTED,
    }
}

/// One derived value as shown: never a default when it could not be computed.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) enum DerivedView {
    Value(String),
    Error { code: Code, message: String },
}

impl DerivedView {
    pub(super) fn is_value(&self) -> bool {
        matches!(self, Self::Value(_))
    }
    /// The value text, or the human error message.
    pub(super) fn text(&self) -> &str {
        match self {
            Self::Value(text) | Self::Error { message: text, .. } => text,
        }
    }
    pub(super) fn code(&self) -> &'static str {
        match self {
            Self::Value(_) => "",
            Self::Error { code, .. } => code.as_str(),
        }
    }
}

pub(super) struct DerivedItem {
    pub id: String,
    pub label: String,
    pub view: DerivedView,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum CheckStatus {
    Pass,
    Violated,
    NotEvaluated,
}

pub(super) struct CheckView {
    pub id: String,
    pub message: String,
    pub rule_ids: String,
    pub expr: String,
    pub status: CheckStatus,
    /// Empty unless the check could not be evaluated.
    pub code: &'static str,
    pub explanation: String,
}

impl CheckView {
    /// Status in words, never colour alone.
    pub(super) fn status_word(&self) -> &'static str {
        match self.status {
            CheckStatus::Pass => "Pass",
            CheckStatus::Violated => "Violated",
            CheckStatus::NotEvaluated => "Not evaluated",
        }
    }
}

/// The whole evaluation was refused: the stored values do not fit the engine's input
/// contract. Only [`InputError`] leads here; a failed formula thread is per-value
/// `internal_error`.
pub(super) struct Unavailable {
    pub code: &'static str,
    pub message: &'static str,
}

pub(super) struct FormulaView {
    /// Derived fields in definition order.
    pub derived: Vec<DerivedItem>,
    /// Constraints in definition order.
    pub checks: Vec<CheckView>,
    pub passed: usize,
    pub violated: usize,
    pub not_evaluated: usize,
    pub unavailable: Option<Unavailable>,
}

impl FormulaView {
    pub(super) fn value(&self, id: &str) -> Option<&DerivedView> {
        self.derived
            .iter()
            .find(|item| item.id == id)
            .map(|item| &item.view)
    }
    /// Violated checks first, then those that could not be evaluated.
    pub(super) fn attention(&self) -> Vec<&CheckView> {
        let mut checks: Vec<&CheckView> = self
            .checks
            .iter()
            .filter(|check| check.status != CheckStatus::Pass)
            .collect();
        checks.sort_by_key(|check| check.status != CheckStatus::Violated);
        checks
    }
    /// Derived values that could not be computed, as the flag counts them, and the first.
    pub(super) fn not_computed(&self) -> Option<(String, &DerivedItem)> {
        let mut failed = self.derived.iter().filter(|item| !item.view.is_value());
        let first = failed.next()?;
        Some((Doubt::NotComputed(1 + failed.count()).text(), first))
    }
    pub(super) fn passes(&self) -> Vec<&CheckView> {
        self.checks
            .iter()
            .filter(|check| check.status == CheckStatus::Pass)
            .collect()
    }
}

/// One reason a saved revision is "Not validated" (RR-3, ASSIST-4).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Doubt {
    /// Checks that ran and failed.
    Violated(usize),
    /// Checks that did not run: an unknown input (D-Z) or an evaluation error.
    NotEvaluated(usize),
    /// Derived values that could not be computed.
    NotComputed(usize),
    /// The engine refused the saved values as input, so nothing was evaluated.
    Unavailable,
    /// A supported runtime definition that declares no checks.
    NoChecks,
    /// A native or unsupported pin: no definition checks apply to it.
    NoCoverage,
}

impl Doubt {
    pub(super) fn text(self) -> String {
        let count = |count: usize, one: &str, many: &str| {
            format!("{count} {}", if count == 1 { one } else { many })
        };
        match self {
            Self::Violated(n) => count(n, "check violated", "checks violated"),
            Self::NotEvaluated(n) => count(n, "check not evaluated", "checks not evaluated"),
            Self::NotComputed(n) => count(
                n,
                "derived value not computed",
                "derived values not computed",
            ),
            Self::Unavailable => "derived values and checks unavailable".into(),
            Self::NoChecks => "its definition declares no checks".into(),
            Self::NoCoverage => "no rule checks cover its interpretation pin".into(),
        }
    }
    /// The page section that explains this reason; `sections` says whether the page has the
    /// derived-values and checks sections.
    pub(super) fn anchor(self, sections: bool) -> Option<&'static str> {
        if !sections {
            return None;
        }
        Some(match self {
            Self::NotComputed(_) | Self::Unavailable => "formula-results",
            _ => "advisory-checks",
        })
    }
}

/// The validity flag of one saved revision (RR-3, ASSIST-4): computed on view from the saved
/// revision's evaluation, like derived values, and never stored. It never blocks a save and
/// never hides or changes a value. No doubts means every declared check passed.
pub(super) struct Validity {
    pub doubts: Vec<Doubt>,
    /// Whether the page has the derived-values and checks sections the reasons link to.
    pub sections: bool,
}

impl Validity {
    /// `formulas` is the saved revision's evaluation (`None` when the pin has no formula
    /// layer); `runtime` says whether the pin is a supported runtime definition.
    pub(super) fn of(formulas: Option<&FormulaView>, runtime: bool) -> Self {
        let Some(formulas) = formulas else {
            return Self {
                doubts: vec![if runtime {
                    Doubt::NoChecks
                } else {
                    Doubt::NoCoverage
                }],
                sections: false,
            };
        };
        if formulas.unavailable.is_some() {
            return Self {
                doubts: vec![Doubt::Unavailable],
                sections: true,
            };
        }
        let not_computed = formulas
            .derived
            .iter()
            .filter(|item| !item.view.is_value())
            .count();
        let doubts = [
            (formulas.checks.is_empty(), Doubt::NoChecks),
            (formulas.violated > 0, Doubt::Violated(formulas.violated)),
            (
                formulas.not_evaluated > 0,
                Doubt::NotEvaluated(formulas.not_evaluated),
            ),
            (not_computed > 0, Doubt::NotComputed(not_computed)),
        ]
        .into_iter()
        .filter_map(|(doubt, reason)| doubt.then_some(reason))
        .collect();
        Self {
            doubts,
            sections: true,
        }
    }
    pub(super) fn validated(&self) -> bool {
        self.doubts.is_empty()
    }
    /// Each reason in words with the section that explains it.
    pub(super) fn links(&self) -> Vec<(String, Option<&'static str>)> {
        self.doubts
            .iter()
            .map(|doubt| (doubt.text(), doubt.anchor(self.sections)))
            .collect()
    }
    /// The doubts in words, separated by semicolons.
    #[cfg(test)]
    pub(super) fn summary(&self) -> String {
        self.doubts
            .iter()
            .map(|doubt| doubt.text())
            .collect::<Vec<_>>()
            .join("; ")
    }
}

pub(super) fn rule_list(ids: &[String]) -> String {
    if ids.is_empty() {
        "none cited".into()
    } else {
        ids.join(", ")
    }
}

fn label<'a>(definition: &'a Definition, id: &'a str) -> &'a str {
    definition
        .fields
        .iter()
        .find(|field| field.id == id)
        .map_or(id, |field| field.label.as_str())
}

/// Ints as decimals, bools as True/False (matching the boolean select), enums by option label.
fn display(definition: &Definition, id: &str, value: &Scalar) -> String {
    match value {
        Scalar::Int(value) => value.to_string(),
        Scalar::Bool(true) => "True".into(),
        Scalar::Bool(false) => "False".into(),
        Scalar::Text(text) => definition
            .fields
            .iter()
            .find(|field| field.id == id && field.control == Control::Enumeration)
            .and_then(|field| field.options.as_ref())
            .and_then(|options| options.iter().find(|option| &option.id == text))
            .map_or_else(|| text.clone(), |option| option.label.clone()),
    }
}

/// The human message; `dependency_failed` names the dependency that failed, which shows
/// its own error on the page.
fn explanation(code: Code, definition: &Definition, dependency: Option<&str>) -> String {
    match (code, dependency) {
        (Code::DependencyFailed, Some(id)) => format!(
            "it uses {}, which could not be computed",
            label(definition, id)
        ),
        _ => message(code).to_owned(),
    }
}

/// The view of an evaluation whose input was refused: nothing is computed.
fn refused(definition: &Definition, error: &InputError) -> FormulaView {
    FormulaView {
        derived: definition
            .fields
            .iter()
            .filter(|field| field.formula.is_some())
            .map(|field| DerivedItem {
                id: field.id.clone(),
                label: field.label.clone(),
                view: DerivedView::Error {
                    code: error.code,
                    message: "derived values are unavailable for this sheet".into(),
                },
            })
            .collect(),
        checks: Vec::new(),
        passed: 0,
        violated: 0,
        not_evaluated: 0,
        unavailable: Some(Unavailable {
            code: error.code.as_str(),
            message: message(error.code),
        }),
    }
}

/// Presents one evaluation of stored data, or the refusal of its input.
pub(super) fn present(pin: &ImmutablePin, result: Result<&Evaluation, &InputError>) -> FormulaView {
    let definition = pin.definition();
    let evaluation = match result {
        Ok(evaluation) => evaluation,
        Err(error) => return refused(definition, error),
    };
    let failed = |kind, id: &str| {
        pin.formulas()
            .and_then(|compiled| compiled.failed_dependency(evaluation, kind, id))
    };
    // A declared formula with no evaluation entry breaks an engine or pin invariant: it
    // fails closed as `internal_error` (shown, counted, flagged), never dropped.
    let missing_value = Err(Code::InternalError);
    let missing_check = Outcome::Error(Code::InternalError);
    let derived = definition
        .fields
        .iter()
        .filter(|field| field.formula.is_some())
        .map(|field| {
            let result = evaluation
                .derived
                .iter()
                .find(|(id, _)| id == &field.id)
                .map_or(&missing_value, |(_, result)| result);
            let view = match result {
                Ok(value) => DerivedView::Value(display(definition, &field.id, value)),
                Err(code) => DerivedView::Error {
                    code: *code,
                    message: explanation(
                        *code,
                        definition,
                        failed(FormulaKind::Derived, &field.id),
                    ),
                },
            };
            DerivedItem {
                id: field.id.clone(),
                label: label(definition, &field.id).to_owned(),
                view,
            }
        })
        .collect();
    let checks: Vec<CheckView> = definition
        .constraints
        .iter()
        .map(|constraint| {
            let outcome = evaluation
                .constraints
                .iter()
                .find(|(id, _)| id == &constraint.id)
                .map_or(&missing_check, |(_, outcome)| outcome);
            let (status, code, explanation) = match outcome {
                Outcome::Pass => (CheckStatus::Pass, "", String::new()),
                Outcome::Violated => (CheckStatus::Violated, "", String::new()),
                Outcome::Error(code) => (
                    CheckStatus::NotEvaluated,
                    code.as_str(),
                    self::explanation(
                        *code,
                        definition,
                        failed(FormulaKind::Constraint, &constraint.id),
                    ),
                ),
            };
            CheckView {
                id: constraint.id.clone(),
                message: constraint.message.clone(),
                rule_ids: rule_list(&constraint.rule_ids),
                expr: constraint.expr.clone(),
                status,
                code,
                explanation,
            }
        })
        .collect();
    let count = |status| checks.iter().filter(|check| check.status == status).count();
    FormulaView {
        passed: count(CheckStatus::Pass),
        violated: count(CheckStatus::Violated),
        not_evaluated: count(CheckStatus::NotEvaluated),
        derived,
        checks,
        unavailable: None,
    }
}

/// Non-deterministic failures are logged with sheet, pin version and formula IDs only, never
/// with values.
pub(super) fn log_failures(sheet_id: i64, pin_version: i64, evaluation: &Evaluation) {
    let derived = evaluation
        .derived
        .iter()
        .filter_map(|(id, result)| result.as_ref().err().map(|code| (id, *code)));
    let constraints = evaluation
        .constraints
        .iter()
        .filter_map(|(id, outcome)| match outcome {
            Outcome::Error(code) => Some((id, *code)),
            Outcome::Pass | Outcome::Violated => None,
        });
    for (id, code) in derived.chain(constraints) {
        if matches!(code, Code::DeadlineExceeded | Code::InternalError) {
            eprintln!(
                "Mimik formula evaluation: sheet {sheet_id}, pin version {pin_version}, formula {id}: {code}"
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::{Value as Json, json};
    use std::collections::BTreeSet;

    /// Codes evaluation can produce: input conversion (`convert`), runtime error mapping
    /// (`error::from_execution`), catalogue functions and the evaluation loop.
    const REACHABLE: &[Code] = &[
        Code::InvalidInput,
        Code::InputTooLarge,
        Code::CollectionTooLarge,
        Code::MissingProperty,
        Code::NullValue,
        Code::TypeMismatch,
        Code::Overflow,
        Code::DivisionByZero,
        Code::InvalidArgument,
        Code::InvalidResult,
        Code::UnknownField,
        Code::ArityMismatch,
        Code::DependencyFailed,
        Code::DeadlineExceeded,
        Code::InternalError,
    ];

    fn field(id: &str, label: &str, control: &str, nullable: bool) -> Json {
        json!({"id": id, "label": label, "control": control, "nullable": nullable,
               "mapping": {"state": "unresolved", "reason": "Synthetic"}})
    }

    fn derived(id: &str, label: &str, control: &str, expr: &str) -> Json {
        let mut value = field(id, label, control, false);
        value["formula"] = json!({"expr": expr, "rule_ids": ["r-1"],
                                  "evidence": {"state": "unresolved", "reason": "Synthetic"}});
        value
    }

    fn definition() -> Json {
        let mut tier = derived("tier", "Tier", "enum", "vitality > 3 ? \"high\" : \"low\"");
        tier["options"] =
            json!([{"id": "high", "label": "<b>High</b>"}, {"id": "low", "label": "Low"}]);
        let fields = vec![
            field("label", "<script>Label", "text", false),
            field("vitality", "Vitality", "integer", false),
            field("luck", "Luck", "integer", true),
            derived("pace", "Pace", "integer", "6 - vitality"),
            derived("lucky", "Lucky", "integer", "luck + 1"),
            derived("lucky_twice", "Lucky twice", "integer", "lucky * 2"),
            derived("lucky_more", "Lucky more", "integer", "lucky_twice + 1"),
            tier,
            derived("marked", "Marked", "boolean", "label == \"<script>\""),
            derived("echo", "Echo", "text", "label"),
        ];
        let ids: Vec<Json> = fields.iter().map(|field| field["id"].clone()).collect();
        json!({
            "contract_version": 1, "game_id": "synthetic-game", "edition_id": "first",
            "definition_id": "formula-view", "title": "Formula view", "sources": [],
            "fields": fields,
            "groups": [{"id": "main", "label": "Main", "fields": ids, "column_hint": "main"}],
            "formula_profile": 1,
            "constraints": [
                {"id": "vitality-cap", "expr": "vitality <= 10", "rule_ids": [],
                 "message": "<script>alert(1)</script>",
                 "evidence": {"state": "unresolved", "reason": "Synthetic"}},
                {"id": "lucky-positive", "expr": "lucky > 0", "rule_ids": ["r-2", "r-1"],
                 "message": "Lucky must be positive.",
                 "evidence": {"state": "unresolved", "reason": "Synthetic"}}
            ]
        })
    }

    fn pin() -> ImmutablePin {
        crate::definitions::private_pin::for_tests(&definition())
    }

    fn view(values: &Json) -> FormulaView {
        let pin = pin();
        let evaluation = pin.evaluate(values).unwrap();
        present(&pin, evaluation.as_ref())
    }

    #[test]
    fn every_code_has_a_human_message() {
        let mut seen = BTreeSet::new();
        for &code in Code::ALL {
            let text = message(code);
            assert!(!text.trim().is_empty(), "{code}");
            assert!(!text.contains('_'), "{code} shows a raw code");
            if REACHABLE.contains(&code) {
                assert_ne!(text, UNEXPECTED, "{code} is reachable");
                assert!(seen.insert(text), "{code} shares wording");
            } else {
                assert_eq!(text, UNEXPECTED, "{code} is admission-only");
            }
        }
        assert_eq!(seen.len(), REACHABLE.len());
    }

    #[test]
    fn derived_states_present_value_error_and_dependency_failed() {
        let known = view(&json!({"label": "<script>", "vitality": 4, "luck": 2}));
        assert_eq!(known.value("pace"), Some(&DerivedView::Value("2".into())));
        assert_eq!(known.value("lucky"), Some(&DerivedView::Value("3".into())));
        assert_eq!(
            known.value("lucky_twice"),
            Some(&DerivedView::Value("6".into()))
        );
        assert_eq!(
            known.value("tier"),
            Some(&DerivedView::Value("<b>High</b>".into())),
            "enums by option label"
        );
        assert_eq!(
            known.value("marked"),
            Some(&DerivedView::Value("True".into()))
        );
        assert_eq!(
            known.value("echo"),
            Some(&DerivedView::Value("<script>".into())),
            "text unchanged; escaping is the template's job"
        );
        let unknown = view(&json!({"label": "x", "vitality": 1, "luck": null}));
        assert_eq!(
            unknown.value("marked"),
            Some(&DerivedView::Value("False".into()))
        );
        assert_eq!(
            unknown.value("tier"),
            Some(&DerivedView::Value("Low".into()))
        );
        let lucky = unknown.value("lucky").unwrap();
        assert!(!lucky.is_value());
        assert_eq!(lucky.code(), "null_value");
        assert_eq!(lucky.text(), message(Code::NullValue));
        let twice = unknown.value("lucky_twice").unwrap();
        assert_eq!(twice.code(), "dependency_failed");
        assert!(!twice.is_value());
        assert_eq!(
            unknown.derived.len(),
            7,
            "every derived field, in definition order"
        );
        assert_eq!(unknown.derived[0].id, "pace");
    }

    #[test]
    fn dependency_failed_names_the_dependency_by_label() {
        let unknown = view(&json!({"label": "x", "vitality": 1, "luck": null}));
        let twice = unknown.value("lucky_twice").unwrap();
        assert_eq!(twice.code(), "dependency_failed");
        assert_eq!(twice.text(), "it uses Lucky, which could not be computed");
        assert_eq!(
            unknown.value("lucky_more").unwrap().text(),
            "it uses Lucky twice, which could not be computed"
        );
        let check = unknown
            .checks
            .iter()
            .find(|check| check.id == "lucky-positive")
            .unwrap();
        assert_eq!(check.code, "dependency_failed");
        assert_eq!(
            check.explanation,
            "it uses Lucky, which could not be computed"
        );
    }

    #[test]
    fn constraint_tri_state_counts_never_count_errors() {
        let all_pass = view(&json!({"label": "x", "vitality": 4, "luck": 2}));
        assert_eq!(
            (all_pass.passed, all_pass.violated, all_pass.not_evaluated),
            (2, 0, 0)
        );
        let mixed = view(&json!({"label": "x", "vitality": 11, "luck": null}));
        assert_eq!(
            (mixed.passed, mixed.violated, mixed.not_evaluated),
            (0, 1, 1)
        );
        let attention = mixed.attention();
        assert_eq!(attention[0].id, "vitality-cap", "violations first");
        assert_eq!(attention[0].status_word(), "Violated");
        assert_eq!(attention[1].status_word(), "Not evaluated");
        assert_eq!(attention[1].code, "dependency_failed");
        assert!(mixed.passes().is_empty());
        assert_eq!(mixed.checks[1].rule_ids, "r-1, r-2");
        assert_eq!(mixed.checks[0].rule_ids, "none cited");
        assert_eq!(mixed.checks[1].expr, "lucky > 0");
    }

    #[test]
    fn deadline_and_internal_error_are_states_not_values() {
        let pin = pin();
        let mut evaluation = pin
            .evaluate(&json!({"label": "x", "vitality": 4, "luck": 2}))
            .unwrap()
            .unwrap();
        evaluation.derived[0].1 = Err(Code::DeadlineExceeded);
        evaluation.derived[1].1 = Err(Code::InternalError);
        evaluation.constraints[0].1 = Outcome::Error(Code::DeadlineExceeded);
        let view = present(&pin, Ok(&evaluation));
        let first = &view
            .derived
            .iter()
            .find(|item| item.id == evaluation.derived[0].0);
        let first = &first.unwrap().view;
        assert_eq!(first.code(), "deadline_exceeded");
        assert!(first.text().contains("Reload to retry"));
        let second = &view
            .derived
            .iter()
            .find(|item| item.id == evaluation.derived[1].0)
            .unwrap()
            .view;
        assert_eq!(second.code(), "internal_error");
        assert!(!second.is_value());
        assert_eq!(view.not_evaluated, 1);
        assert_eq!(view.passed, 1);
    }

    #[test]
    fn validity_flags_every_doubt_and_passes_only_full_coverage() {
        let pass = view(&json!({"label": "x", "vitality": 4, "luck": 2}));
        let validity = Validity::of(Some(&pass), true);
        assert!(validity.validated());
        assert!(validity.doubts.is_empty(), "{:?}", validity.doubts);
        let violated = Validity::of(
            Some(&view(&json!({"label": "x", "vitality": 11, "luck": 2}))),
            true,
        );
        assert_eq!(violated.doubts, [Doubt::Violated(1)]);
        assert_eq!(
            violated.links(),
            [("1 check violated".to_owned(), Some("advisory-checks"))]
        );
        assert_eq!(violated.summary(), "1 check violated");
        let unknown = Validity::of(
            Some(&view(&json!({"label": "x", "vitality": 11, "luck": null}))),
            true,
        );
        assert_eq!(
            unknown.doubts,
            [
                Doubt::Violated(1),
                Doubt::NotEvaluated(1),
                Doubt::NotComputed(3)
            ]
        );
        assert_eq!(
            unknown.summary(),
            "1 check violated; 1 check not evaluated; 3 derived values not computed"
        );
        assert!(!unknown.validated());
        // Each reason links to the section that explains it (Sol review).
        let anchors: Vec<_> = unknown
            .links()
            .into_iter()
            .map(|(_, anchor)| anchor)
            .collect();
        assert_eq!(
            anchors,
            [
                Some("advisory-checks"),
                Some("advisory-checks"),
                Some("formula-results")
            ]
        );
        let (count, first) = view(&json!({"label": "x", "vitality": 11, "luck": null}))
            .not_computed()
            .map(|(count, first)| (count, first.id.clone()))
            .unwrap();
        assert_eq!(
            (count.as_str(), first.as_str()),
            ("3 derived values not computed", "lucky")
        );
        assert!(pass.not_computed().is_none());
    }

    #[test]
    fn validity_flags_errors_unavailable_input_and_missing_coverage() {
        let pin = pin();
        let mut evaluation = pin
            .evaluate(&json!({"label": "x", "vitality": 4, "luck": 2}))
            .unwrap()
            .unwrap();
        evaluation.constraints[1].1 = Outcome::Error(Code::DivisionByZero);
        let errored = present(&pin, Ok(&evaluation));
        assert_eq!(
            Validity::of(Some(&errored), true).doubts,
            [Doubt::NotEvaluated(1)]
        );
        let error = InputError {
            code: Code::InputTooLarge,
            path: "/label".into(),
        };
        let refused = Validity::of(Some(&present(&pin, Err(&error))), true);
        assert_eq!(refused.doubts, [Doubt::Unavailable]);
        assert_eq!(
            refused.links(),
            [(
                "derived values and checks unavailable".to_owned(),
                Some("formula-results")
            )]
        );
        let mut definition = definition();
        definition["constraints"] = json!([]);
        let pin = crate::definitions::private_pin::for_tests(&definition);
        let evaluation = pin
            .evaluate(&json!({"label": "x", "vitality": 4, "luck": 2}))
            .unwrap()
            .unwrap();
        let unchecked = Validity::of(Some(&present(&pin, Ok(&evaluation))), true);
        assert_eq!(unchecked.doubts, [Doubt::NoChecks]);
        assert_eq!(unchecked.links()[0].1, Some("advisory-checks"));
        let formula_free = Validity::of(None, true);
        assert_eq!(formula_free.doubts, [Doubt::NoChecks]);
        assert_eq!(formula_free.links()[0].1, None);
        assert_eq!(Validity::of(None, false).links()[0].1, None);
        let native = Validity::of(None, false);
        assert_eq!(native.doubts, [Doubt::NoCoverage]);
        assert_eq!(
            native.summary(),
            "no rule checks cover its interpretation pin"
        );
        assert_eq!(Doubt::Violated(2).text(), "2 checks violated");
        assert_eq!(Doubt::NotComputed(1).text(), "1 derived value not computed");
    }

    /// Third review: a declared formula with no evaluation entry (a broken engine or pin
    /// invariant) fails closed as `internal_error`: shown, counted and flagged, never
    /// dropped, so the sheet cannot look validated.
    #[test]
    fn a_formula_missing_from_the_evaluation_fails_closed() {
        let pin = pin();
        let mut evaluation = pin
            .evaluate(&json!({"label": "x", "vitality": 4, "luck": 2}))
            .unwrap()
            .unwrap();
        let missing_check = evaluation.constraints.remove(1).0;
        let missing_value = evaluation.derived.remove(0).0;
        let view = present(&pin, Ok(&evaluation));
        assert_eq!(view.checks.len(), 2, "every declared check is presented");
        assert_eq!((view.passed, view.violated, view.not_evaluated), (1, 0, 1));
        let check = view
            .checks
            .iter()
            .find(|check| check.id == missing_check)
            .unwrap();
        assert_eq!(check.status, CheckStatus::NotEvaluated);
        assert_eq!(check.code, "internal_error");
        assert_eq!(view.derived.len(), 7, "every declared derived value");
        assert_eq!(view.value(&missing_value).unwrap().code(), "internal_error");
        assert_eq!(
            Validity::of(Some(&view), true).doubts,
            [Doubt::NotEvaluated(1), Doubt::NotComputed(1)]
        );
    }

    #[test]
    fn input_error_presents_unavailable() {
        let pin = pin();
        let error = InputError {
            code: Code::InputTooLarge,
            path: "/label".into(),
        };
        let view = present(&pin, Err(&error));
        let unavailable = view.unavailable.as_ref().unwrap();
        assert_eq!(unavailable.code, "input_too_large");
        assert_eq!(unavailable.message, message(Code::InputTooLarge));
        assert!(view.checks.is_empty());
        assert_eq!((view.passed, view.violated, view.not_evaluated), (0, 0, 0));
        assert!(view.derived.iter().all(|item| !item.view.is_value()));
        assert_eq!(view.derived.len(), 7);
    }
}
