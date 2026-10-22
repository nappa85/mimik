//! Reading a model reply (assisted sheet plan §1.4, "Output contract"): JSON, schema v2
//! (`jsonschema`), typed decoding, then per-option domain checks: references, literal
//! provenance and checked `i128` evaluation with an `i64` result. A reply that fails the
//! schema or decoding shows nothing; an option that fails a domain check is dropped alone.
use super::contract::{MAX_DEPTH, MAX_OPTIONS, MAX_TEXT_CHARS, SCHEMA};
use super::lexer;
use serde::Deserialize;
use serde_json::Value;
use std::collections::{BTreeMap, BTreeSet};
use std::sync::LazyLock;

static VALIDATOR: LazyLock<Result<jsonschema::Validator, ()>> = LazyLock::new(|| {
    jsonschema::draft202012::options()
        .offline()
        .build(&SCHEMA)
        .map_err(|_| ())
});

/// An operator of the expression tree.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum BinOp {
    Add,
    Sub,
    Mul,
    Div,
}

/// How a division rounds; `Exact` drops the option on a remainder.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum Round {
    Floor,
    Ceil,
    Exact,
}

/// A node of the expression tree, as the schema allows it.
#[derive(Clone, Debug, PartialEq, Eq, Deserialize)]
#[serde(untagged, deny_unknown_fields)]
pub(crate) enum Node {
    Lit {
        lit: i64,
    },
    Ref {
        #[serde(rename = "ref")]
        alias: String,
    },
    Op {
        op: BinOp,
        #[serde(default)]
        round: Option<Round>,
        l: Box<Node>,
        r: Box<Node>,
    },
}

/// What a supplied alias stands for.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Supplied {
    /// The typed address (`field:<id>` or `cell:<list>/<host>/<property>`).
    pub address: String,
    /// Only integers may be referenced; an enum is context for reading.
    pub integer: bool,
    /// The value the request showed; `None` is unknown (D-Z).
    pub value: Option<i64>,
}

/// What a reply is checked against: the request's phrase, the target's value and the
/// aliases it supplied (`t` included).
#[derive(Clone, Debug)]
pub(crate) struct Checks {
    pub phrase: String,
    pub target_value: Option<i64>,
    pub supplied: BTreeMap<String, Supplied>,
}

/// Why an option was dropped.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) enum Drop {
    /// A node that does not decode (a literal outside `i64`, a division without rounding).
    Shape,
    TooDeep,
    Explanation,
    Provenance,
    UnsuppliedRef,
    EnumRef,
    UnknownRef,
    Overflow,
    DivisionByZero,
    Inexact,
}

/// A validated option: its tree, explanation, value and the references it used.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Candidate {
    pub expr: Node,
    pub explanation: String,
    pub value: i64,
    /// Each alias the tree referenced, once, with what it stands for.
    pub refs: BTreeMap<String, Supplied>,
}

/// A reply that passed the schema and decoding.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum Reading {
    /// The options that passed every check, in reply order, and the reasons for the rest.
    Options {
        shown: Vec<Candidate>,
        dropped: Vec<Drop>,
    },
    /// Display-only readings: never applicable.
    Ambiguous(Vec<String>),
}

/// Why a whole reply shows nothing.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Invalid {
    /// Not JSON (truncated, prose, empty).
    Json,
    /// Not valid against schema v2.
    Schema,
    /// Valid against the schema but not the typed reply (cannot happen with schema v2; a
    /// defence against a schema edit that loosens it).
    Decode,
}

#[derive(Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
enum Reply {
    Options { options: Vec<Value> },
    Ambiguous { readings: Vec<String> },
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawOption {
    expr: Value,
    explanation: String,
}

/// Reads a reply's text.
pub(crate) fn read(text: &str, checks: &Checks) -> Result<Reading, Invalid> {
    let value: Value = serde_json::from_str(text).map_err(|_| Invalid::Json)?;
    let validator = VALIDATOR.as_ref().map_err(|()| Invalid::Schema)?;
    if !validator.is_valid(&value) {
        return Err(Invalid::Schema);
    }
    match serde_json::from_value::<Reply>(value).map_err(|_| Invalid::Decode)? {
        Reply::Ambiguous { readings } => {
            let fits = (1..=MAX_OPTIONS).contains(&readings.len())
                && readings.iter().all(|text| text_fits(text));
            if fits {
                Ok(Reading::Ambiguous(readings))
            } else {
                Err(Invalid::Decode)
            }
        }
        Reply::Options { options } => {
            if options.len() > MAX_OPTIONS {
                return Err(Invalid::Decode);
            }
            let allowed = allowed_literals(checks);
            let mut shown = Vec::new();
            let mut dropped = Vec::new();
            for option in options {
                match candidate(option, checks, &allowed) {
                    Ok(candidate) => shown.push(candidate),
                    Err(reason) => dropped.push(reason),
                }
            }
            Ok(Reading::Options { shown, dropped })
        }
    }
}

fn text_fits(text: &str) -> bool {
    !text.is_empty() && text.chars().count() <= MAX_TEXT_CHARS
}

/// The literal magnitudes provenance admits: the phrase's numbers, the target's value,
/// and with a fraction or percent word 2, 3, 4 and 100.
fn allowed_literals(checks: &Checks) -> BTreeSet<u64> {
    let mut allowed: BTreeSet<u64> = lexer::phrase_numbers(&checks.phrase).into_iter().collect();
    if let Some(value) = checks.target_value {
        allowed.insert(value.unsigned_abs());
    }
    if lexer::has_fraction_word(&checks.phrase) {
        allowed.extend(lexer::FRACTION_LITERALS);
    }
    allowed
}

fn candidate(option: Value, checks: &Checks, allowed: &BTreeSet<u64>) -> Result<Candidate, Drop> {
    let option: RawOption = serde_json::from_value(option).map_err(|_| Drop::Shape)?;
    let expr: Node = serde_json::from_value(option.expr).map_err(|_| Drop::Shape)?;
    if depth(&expr) > MAX_DEPTH {
        return Err(Drop::TooDeep);
    }
    if !text_fits(&option.explanation) {
        return Err(Drop::Explanation);
    }
    let mut refs = BTreeMap::new();
    let value = evaluate(&expr, checks, allowed, &mut refs)?;
    let value = i64::try_from(value).map_err(|_| Drop::Overflow)?;
    Ok(Candidate {
        expr,
        explanation: option.explanation,
        value,
        refs,
    })
}

/// Operator levels below `node` (a leaf is 0).
fn depth(node: &Node) -> usize {
    match node {
        Node::Lit { .. } | Node::Ref { .. } => 0,
        Node::Op { l, r, .. } => 1 + depth(l).max(depth(r)),
    }
}

fn evaluate(
    node: &Node,
    checks: &Checks,
    allowed: &BTreeSet<u64>,
    refs: &mut BTreeMap<String, Supplied>,
) -> Result<i128, Drop> {
    match node {
        Node::Lit { lit } => {
            if allowed.contains(&lit.unsigned_abs()) {
                Ok(i128::from(*lit))
            } else {
                Err(Drop::Provenance)
            }
        }
        Node::Ref { alias } => {
            let supplied = checks.supplied.get(alias).ok_or(Drop::UnsuppliedRef)?;
            if !supplied.integer {
                return Err(Drop::EnumRef);
            }
            let value = supplied.value.ok_or(Drop::UnknownRef)?;
            refs.insert(alias.clone(), supplied.clone());
            Ok(i128::from(value))
        }
        Node::Op { op, round, l, r } => {
            let left = evaluate(l, checks, allowed, refs)?;
            let right = evaluate(r, checks, allowed, refs)?;
            match (op, round) {
                (BinOp::Add, None) => left.checked_add(right).ok_or(Drop::Overflow),
                (BinOp::Sub, None) => left.checked_sub(right).ok_or(Drop::Overflow),
                (BinOp::Mul, None) => left.checked_mul(right).ok_or(Drop::Overflow),
                (BinOp::Div, Some(round)) => divide(left, right, *round),
                _ => Err(Drop::Shape),
            }
        }
    }
}

/// Checked division with the requested rounding, on negatives too.
pub(crate) fn divide(left: i128, right: i128, round: Round) -> Result<i128, Drop> {
    if right == 0 {
        return Err(Drop::DivisionByZero);
    }
    let quotient = left.checked_div(right).ok_or(Drop::Overflow)?;
    let remainder = left.checked_rem(right).ok_or(Drop::Overflow)?;
    if remainder == 0 {
        return Ok(quotient);
    }
    // Truncation rounds towards zero: the exact quotient is negative when the signs differ.
    let negative = (left < 0) != (right < 0);
    match round {
        Round::Exact => Err(Drop::Inexact),
        Round::Floor if negative => quotient.checked_sub(1).ok_or(Drop::Overflow),
        Round::Ceil if !negative => quotient.checked_add(1).ok_or(Drop::Overflow),
        Round::Floor | Round::Ceil => Ok(quotient),
    }
}

/// The expression as the page shows it, with `−`, `×` and `÷`, references by `name`
/// (the label of what an alias stands for) and parentheses around nested operators.
pub(crate) fn render(node: &Node, name: &dyn Fn(&str) -> String) -> String {
    fn inner(node: &Node, name: &dyn Fn(&str) -> String, nested: bool) -> String {
        match node {
            Node::Lit { lit } if *lit < 0 => format!("\u{2212}{}", lit.unsigned_abs()),
            Node::Lit { lit } => lit.to_string(),
            Node::Ref { alias } => name(alias),
            Node::Op { op, round, l, r } => {
                let symbol = match op {
                    BinOp::Add => "+",
                    BinOp::Sub => "\u{2212}",
                    BinOp::Mul => "\u{d7}",
                    BinOp::Div => "\u{f7}",
                };
                let suffix = match round {
                    Some(Round::Floor) => " (rounded down)",
                    Some(Round::Ceil) => " (rounded up)",
                    Some(Round::Exact) | None => "",
                };
                let text = format!(
                    "{} {symbol} {}{suffix}",
                    inner(l, name, true),
                    inner(r, name, true)
                );
                if nested { format!("({text})") } else { text }
            }
        }
    }
    inner(node, name, false)
}

#[cfg(test)]
#[path = "output_tests.rs"]
mod tests;
