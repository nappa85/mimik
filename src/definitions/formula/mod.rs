//! Derived fields and advisory constraints as CEL ("CEL profile mimik-1").
//!
//! Expressions use the maintained `cel` crate, restricted at compile time to an allowlisted
//! subset with static types, depth/nesting/cost bounds and Mimik's custom aggregate functions.
//! Results are advisory: they never write sheets, approve evidence or establish legality.
mod convert;
mod error;
mod functions;
mod profile;
#[cfg(test)]
mod tests;

pub use error::Code;

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value as Json};
use std::{
    collections::{BTreeMap, BTreeSet},
    panic::{AssertUnwindSafe, catch_unwind},
    sync::{Arc, LazyLock},
    time::{Duration, Instant},
};

/// Profile identity; any change to admitted semantics requires a new profile.
pub const PROFILE: &str = "cel-mimik-1";
pub const MAX_SOURCE_BYTES: usize = 1024;
/// Guard passed to the `cel` parser's own recursion counter.
pub const MAX_PARSER_DEPTH: u16 = 32;
/// Maximum node depth of the macro-expanded AST, checked iteratively.
pub const MAX_AST_DEPTH: usize = 64;
/// One list-size constant for storage caps, evaluation checks and the static cost budget.
pub const MAX_LIST_ITEMS: usize = 256;
pub const MAX_TEXT_CHARS: usize = 20_000;
/// Aggregate bound on converted input (string bytes plus a fixed charge per value), equal to
/// the stored-sheet body cap, so per-value caps cannot add up to hundreds of megabytes.
pub const MAX_INPUT_BYTES: usize = 1024 * 1024;
pub const MAX_FIELDS: usize = 256;
pub const MAX_LISTS: usize = 32;
pub const MAX_PROPERTIES: usize = 32;
pub const MAX_OPTIONS: usize = 128;
pub const MAX_FORMULAS: usize = 384;
/// Static work units per definition (see `docs/derived-formulas-plan.md` for calibration).
pub const MAX_COST: u64 = 2_000_000;
/// Parsing and evaluation run on a dedicated thread with this stack: the ANTLR-generated parser
/// and `cel`'s recursive evaluator overflow a 2 MiB stack in debug builds well inside the
/// depth limits (measured: 12 nested parentheses, a 61-term chain).
pub const FORMULA_STACK_BYTES: usize = 16 * 1024 * 1024;
/// Wall-clock safety net between formulas; the static budget is the deterministic bound.
pub const DEADLINE: Duration = Duration::from_millis(500);

/// Declared value type of a field, item property or derived result.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case", deny_unknown_fields)]
pub enum ValueType {
    Int,
    Bool,
    /// Text with a maximum length in Unicode scalar values.
    Text(usize),
    /// Exact option IDs.
    Enum(Vec<String>),
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct FieldDecl {
    pub id: String,
    #[serde(rename = "type")]
    pub ty: ValueType,
    #[serde(default)]
    pub nullable: bool,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct PropertyDecl {
    pub id: String,
    #[serde(rename = "type")]
    pub ty: ValueType,
    #[serde(default)]
    pub optional: bool,
}

/// A list of records. Every record also carries a host-managed `id` string.
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ListDecl {
    pub id: String,
    pub properties: Vec<PropertyDecl>,
}

#[derive(Clone, Debug, Default, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Declarations {
    #[serde(default)]
    pub fields: Vec<FieldDecl>,
    #[serde(default)]
    pub lists: Vec<ListDecl>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct DerivedSpec {
    pub id: String,
    #[serde(rename = "type")]
    pub ty: ValueType,
    pub expr: String,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ConstraintSpec {
    pub id: String,
    pub expr: String,
}

/// Everything needed to compile one sheet's formulas, independent of the definition envelope.
#[derive(Clone, Debug, Default, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct SheetSpec {
    pub declarations: Declarations,
    #[serde(default)]
    pub derived: Vec<DerivedSpec>,
    #[serde(default)]
    pub constraints: Vec<ConstraintSpec>,
}

/// A compile-time rejection: which formula (or declaration) and why.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct Rejection {
    pub code: Code,
    pub target: String,
}

/// Input values did not match the declarations; nothing was evaluated.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct InputError {
    pub code: Code,
    pub path: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(untagged)]
pub enum Scalar {
    Int(i64),
    Bool(bool),
    Text(String),
}

/// Constraint tri-state. `Error` is never counted as passing or violated.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Outcome {
    Pass,
    Violated,
    Error(Code),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Evaluation {
    pub derived: Vec<(String, Result<Scalar, Code>)>,
    pub constraints: Vec<(String, Outcome)>,
}

impl Evaluation {
    /// Stable JSON shape shared by the conformance fixture and the `formula_eval` CLI.
    #[must_use]
    pub fn to_json(&self) -> Json {
        let derived: Map<String, Json> = self
            .derived
            .iter()
            .map(|(id, result)| {
                let value = match result {
                    Ok(value) => serde_json::json!({ "value": value }),
                    Err(code) => serde_json::json!({ "error": code }),
                };
                (id.clone(), value)
            })
            .collect();
        let constraints: Map<String, Json> = self
            .constraints
            .iter()
            .map(|(id, outcome)| {
                let value = match outcome {
                    Outcome::Pass => Json::from("pass"),
                    Outcome::Violated => Json::from("violated"),
                    Outcome::Error(code) => serde_json::json!({ "error": code }),
                };
                (id.clone(), value)
            })
            .collect();
        serde_json::json!({ "derived": derived, "constraints": constraints })
    }
}

/// Which namespace a formula ID belongs to: derived fields and constraints are separate.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FormulaKind {
    Derived,
    Constraint,
}

/// A derived value that differs between two evaluations.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DerivedChange {
    pub id: String,
    pub before: Result<Scalar, Code>,
    pub after: Result<Scalar, Code>,
}

/// A constraint whose outcome differs between two evaluations.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ConstraintChange {
    pub id: String,
    pub before: Outcome,
    pub after: Outcome,
}

/// How a constraint change reads. An error on either side is `Unknown`: never a new or a
/// resolved violation.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Transition {
    NewViolation,
    Resolved,
    Unknown,
}

impl ConstraintChange {
    #[must_use]
    pub fn transition(&self) -> Transition {
        match (self.before, self.after) {
            (Outcome::Pass, Outcome::Violated) => Transition::NewViolation,
            (Outcome::Violated, Outcome::Pass) => Transition::Resolved,
            _ => Transition::Unknown,
        }
    }
}

/// What changing a sheet from `before` to `after` would change. Pure: it writes nothing.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Preview {
    /// Changed derived values, in evaluation order.
    pub derived: Vec<DerivedChange>,
    /// Constraints whose outcome changed, in definition order.
    pub constraints: Vec<ConstraintChange>,
}

impl Preview {
    fn with(&self, transition: Transition) -> Vec<&str> {
        self.constraints
            .iter()
            .filter(|change| change.transition() == transition)
            .map(|change| change.id.as_str())
            .collect()
    }
    #[must_use]
    pub fn new_violations(&self) -> Vec<&str> {
        self.with(Transition::NewViolation)
    }
    #[must_use]
    pub fn resolved_violations(&self) -> Vec<&str> {
        self.with(Transition::Resolved)
    }
    /// Changed constraints with an error on either side.
    #[must_use]
    pub fn unknown(&self) -> Vec<&str> {
        self.with(Transition::Unknown)
    }
}

/// Evaluates `before` and `after` and reports what differs. Library-only (no route or CLI
/// yet). Each side is subject to [`DEADLINE`], so under load a change can read as `Unknown`.
///
/// # Errors
///
/// Returns the [`InputError`] of whichever side does not match the declarations.
pub fn preview(compiled: &Compiled, before: &Json, after: &Json) -> Result<Preview, InputError> {
    let old = compiled.evaluate(before)?;
    let new = compiled.evaluate(after)?;
    Ok(Preview {
        derived: old
            .derived
            .into_iter()
            .zip(new.derived)
            .filter(|((_, before), (_, after))| before != after)
            .map(|((id, before), (_, after))| DerivedChange { id, before, after })
            .collect(),
        constraints: old
            .constraints
            .into_iter()
            .zip(new.constraints)
            .filter(|((_, before), (_, after))| before != after)
            .map(|((id, before), (_, after))| ConstraintChange { id, before, after })
            .collect(),
    })
}

/// Test-only fault injection at the panic boundaries; compiles to nothing otherwise.
macro_rules! fault_point {
    ($kind:literal, $id:expr) => {
        #[cfg(test)]
        tests::faults::hit($kind, $id);
    };
}

static ENV: LazyLock<Arc<cel::Env>> =
    LazyLock::new(|| Arc::new(cel::Env::stdlib().with_optional_support(false)));

struct Formula {
    id: String,
    /// `None` for a constraint.
    ty: Option<ValueType>,
    expr: cel::IdedExpr,
    /// Derived fields this formula reads.
    deps: BTreeSet<String>,
    /// Declared scalar fields and `(list, property)` values it reads (slice 1.4).
    reads: BTreeSet<String>,
    item_reads: BTreeSet<(String, String)>,
}

impl Formula {
    /// Whether the formula reads an unknown integer it cannot evaluate (slice 1.4).
    fn reads_unknown(&self, unknowns: &convert::Unknowns) -> bool {
        !self.reads.is_disjoint(&unknowns.fields)
            || !self.item_reads.is_disjoint(&unknowns.properties)
    }
}

/// Compiled, admitted formulas in topological order. Thread-safe for reuse across requests.
pub struct Compiled {
    lookup: profile::Lookup,
    derived: Vec<Formula>,
    constraints: Vec<Formula>,
    cost: u64,
}

const _: () = {
    const fn assert_send_sync<T: Send + Sync>() {}
    assert_send_sync::<Compiled>();
    assert_send_sync::<cel::IdedExpr>();
    assert_send_sync::<cel::Env>();
};

impl Compiled {
    /// Admits a sheet's formulas against its declarations.
    ///
    /// # Errors
    ///
    /// Returns the first [`Rejection`]: invalid declarations or identifiers, a formula outside
    /// the profile (syntax, construct, type, depth, nesting), a derived-field cycle or a
    /// definition exceeding [`MAX_COST`].
    pub fn new(spec: &SheetSpec) -> Result<Self, Rejection> {
        on_formula_stack(|| Self::new_here(spec)).unwrap_or_else(|code| {
            Err(Rejection {
                code,
                target: String::new(),
            })
        })
    }

    fn new_here(spec: &SheetSpec) -> Result<Self, Rejection> {
        let lookup = profile::Lookup::new(spec)?;
        let mut derived = Vec::new();
        let mut cost: u64 = 0;
        for item in &spec.derived {
            fault_point!("compile", &item.id);
            let (formula, units) = profile::compile(&lookup, &item.id, &item.expr, Some(&item.ty))?;
            derived.push(formula_of(&item.id, Some(item.ty.clone()), formula));
            cost = cost.saturating_add(units);
        }
        let mut constraints = Vec::new();
        for item in &spec.constraints {
            let (formula, units) = profile::compile(&lookup, &item.id, &item.expr, None)?;
            constraints.push(formula_of(&item.id, None, formula));
            cost = cost.saturating_add(units);
        }
        if cost > MAX_COST {
            return Err(Rejection {
                code: Code::CostLimit,
                target: String::new(),
            });
        }
        let derived = order(derived)?;
        Ok(Self {
            lookup,
            derived,
            constraints,
            cost,
        })
    }

    /// Derived field IDs in evaluation order.
    #[must_use]
    pub fn derived_order(&self) -> Vec<&str> {
        self.derived.iter().map(|item| item.id.as_str()).collect()
    }

    /// Expression nodes across every compiled formula, for memory sizing in tests.
    #[cfg(test)]
    pub(crate) fn node_count(&self) -> usize {
        let mut stack: Vec<&cel::IdedExpr> = self
            .derived
            .iter()
            .chain(&self.constraints)
            .map(|formula| &formula.expr)
            .collect();
        let mut nodes = 0;
        while let Some(expr) = stack.pop() {
            nodes += 1;
            stack.extend(profile::children_for_tests(expr));
        }
        nodes
    }

    /// For a formula reported `dependency_failed` in `evaluation`, the first derived field it
    /// reads, in evaluation order, whose own result is an error. `None` for any other result.
    #[must_use]
    pub fn failed_dependency(
        &self,
        evaluation: &Evaluation,
        kind: FormulaKind,
        id: &str,
    ) -> Option<&str> {
        let (formulas, code) = match kind {
            FormulaKind::Derived => (
                &self.derived,
                evaluation
                    .derived
                    .iter()
                    .find(|(item, _)| item == id)
                    .and_then(|(_, result)| result.as_ref().err().copied()),
            ),
            FormulaKind::Constraint => (
                &self.constraints,
                evaluation
                    .constraints
                    .iter()
                    .find(|(item, _)| item == id)
                    .and_then(|(_, outcome)| match outcome {
                        Outcome::Error(code) => Some(*code),
                        Outcome::Pass | Outcome::Violated => None,
                    }),
            ),
        };
        if code? != Code::DependencyFailed {
            return None;
        }
        let formula = formulas.iter().find(|formula| formula.id == id)?;
        self.derived
            .iter()
            .filter(|dependency| formula.deps.contains(&dependency.id))
            .find(|dependency| {
                evaluation
                    .derived
                    .iter()
                    .any(|(item, result)| item == &dependency.id && result.is_err())
            })
            .map(|dependency| dependency.id.as_str())
    }

    /// Static work units of this definition, at most [`MAX_COST`].
    #[must_use]
    pub fn cost(&self) -> u64 {
        self.cost
    }

    /// Evaluates every derived field and constraint against canonical input values.
    ///
    /// # Errors
    ///
    /// Returns [`InputError`] when values do not match the declarations exactly (unknown or
    /// missing keys, out-of-range integers, nulls in items, oversized lists, bad enum values).
    pub fn evaluate(&self, values: &Json) -> Result<Evaluation, InputError> {
        self.evaluate_with_deadline(values, DEADLINE)
    }

    pub(crate) fn evaluate_with_deadline(
        &self,
        values: &Json,
        deadline: Duration,
    ) -> Result<Evaluation, InputError> {
        on_formula_stack(|| self.evaluate_here(values, deadline))
            .unwrap_or_else(|code| Ok(self.failed_all(code)))
    }

    fn evaluate_here(&self, values: &Json, deadline: Duration) -> Result<Evaluation, InputError> {
        let inputs = convert::inputs(&self.lookup, values)?;
        let unknowns = convert::unknowns(&self.lookup, values);
        let started = Instant::now();
        let mut root = cel::Context::with_env(Arc::clone(&ENV));
        if functions::register(&mut root).is_err() {
            return Ok(self.failed_all(Code::InternalError));
        }
        let mut scope = root.new_inner_scope();
        for (name, value) in inputs {
            scope.add_variable_from_value(name, value);
        }
        let mut failed = BTreeSet::new();
        let mut derived = Vec::new();
        for formula in &self.derived {
            fault_point!("evaluate", &formula.id);
            let result = if started.elapsed() >= deadline {
                Err(Code::DeadlineExceeded)
            } else if formula.deps.iter().any(|dep| failed.contains(dep)) {
                Err(Code::DependencyFailed)
            } else if formula.reads_unknown(&unknowns) {
                Err(Code::NullValue)
            } else {
                run(&formula.id, &formula.expr, &scope)
                    .and_then(|value| convert::result(formula.ty.as_ref(), &value))
            };
            match &result {
                Ok(value) => {
                    scope.add_variable_from_value(formula.id.clone(), convert::to_cel(value));
                }
                Err(_) => {
                    failed.insert(formula.id.clone());
                }
            }
            derived.push((formula.id.clone(), result));
        }
        let mut constraints = Vec::new();
        for formula in &self.constraints {
            let outcome = if started.elapsed() >= deadline {
                Outcome::Error(Code::DeadlineExceeded)
            } else if formula.deps.iter().any(|dep| failed.contains(dep)) {
                Outcome::Error(Code::DependencyFailed)
            } else if formula.reads_unknown(&unknowns) {
                Outcome::Error(Code::NullValue)
            } else {
                match run(&formula.id, &formula.expr, &scope) {
                    Ok(cel::Value::Bool(true)) => Outcome::Pass,
                    Ok(cel::Value::Bool(false)) => Outcome::Violated,
                    Ok(cel::Value::Null) => Outcome::Error(Code::NullValue),
                    Ok(_) => Outcome::Error(Code::TypeMismatch),
                    Err(code) => Outcome::Error(code),
                }
            };
            constraints.push((formula.id.clone(), outcome));
        }
        Ok(Evaluation {
            derived,
            constraints,
        })
    }

    fn failed_all(&self, code: Code) -> Evaluation {
        Evaluation {
            derived: self
                .derived
                .iter()
                .map(|item| (item.id.clone(), Err(code)))
                .collect(),
            constraints: self
                .constraints
                .iter()
                .map(|item| (item.id.clone(), Outcome::Error(code)))
                .collect(),
        }
    }
}

fn formula_of(id: &str, ty: Option<ValueType>, compiled: profile::Checked) -> Formula {
    Formula {
        id: id.to_owned(),
        ty,
        expr: compiled.expr,
        deps: compiled.deps,
        reads: compiled.reads,
        item_reads: compiled.item_reads,
    }
}

/// Runs `work` on a scoped thread with [`FORMULA_STACK_BYTES`] of stack. A panic or a failed
/// spawn fails closed as `internal_error`.
fn on_formula_stack<T: Send>(work: impl FnOnce() -> T + Send) -> Result<T, Code> {
    std::thread::scope(|scope| {
        std::thread::Builder::new()
            .name("mimik-formula".into())
            .stack_size(FORMULA_STACK_BYTES)
            .spawn_scoped(scope, work)
            .map_err(|_| Code::InternalError)?
            .join()
            .map_err(|_| Code::InternalError)
    })
}

/// Per-formula panic boundary: a panic in `work` fails closed as `internal_error` for that
/// formula only.
fn guarded<T>(work: impl FnOnce() -> Result<T, Code>) -> Result<T, Code> {
    catch_unwind(AssertUnwindSafe(work)).unwrap_or(Err(Code::InternalError))
}

/// Runs one expression inside the per-formula panic boundary.
fn run(id: &str, expr: &cel::IdedExpr, scope: &cel::Context<'_, '_>) -> Result<cel::Value, Code> {
    let _ = id; // Read only by the test-only fault point.
    guarded(|| {
        fault_point!("run", id);
        cel::Value::resolve(expr, scope).map_err(|error| error::from_execution(&error))
    })
}

/// Kahn's algorithm with declaration-order tie-breaking.
fn order(derived: Vec<Formula>) -> Result<Vec<Formula>, Rejection> {
    let ids: BTreeMap<String, usize> = derived
        .iter()
        .enumerate()
        .map(|(index, item)| (item.id.clone(), index))
        .collect();
    let mut pending: Vec<usize> = derived.iter().map(|item| item.deps.len()).collect();
    let mut dependents: Vec<Vec<usize>> = vec![Vec::new(); derived.len()];
    for (index, item) in derived.iter().enumerate() {
        for dep in &item.deps {
            dependents[ids[dep]].push(index);
        }
    }
    let mut ready: BTreeSet<usize> = (0..derived.len()).filter(|&i| pending[i] == 0).collect();
    let mut sequence = Vec::with_capacity(derived.len());
    while let Some(next) = ready.pop_first() {
        sequence.push(next);
        for &dependent in &dependents[next] {
            pending[dependent] -= 1;
            if pending[dependent] == 0 {
                ready.insert(dependent);
            }
        }
    }
    if sequence.len() != derived.len() {
        let target = derived
            .iter()
            .enumerate()
            .find(|(index, _)| !sequence.contains(index))
            .map(|(_, item)| item.id.clone())
            .unwrap_or_default();
        return Err(Rejection {
            code: Code::FormulaCycle,
            target,
        });
    }
    let mut slots: Vec<Option<Formula>> = derived.into_iter().map(Some).collect();
    Ok(sequence
        .into_iter()
        .filter_map(|index| slots[index].take())
        .collect())
}
