//! Admission of the CEL profile: identifiers, depth, allowlist over the macro-expanded AST
//! (with the four recognized macro expansion shapes), static types and the work budget.
use super::{
    Code, MAX_AST_DEPTH, MAX_FIELDS, MAX_FORMULAS, MAX_LIST_ITEMS, MAX_LISTS, MAX_OPTIONS,
    MAX_PARSER_DEPTH, MAX_PROPERTIES, MAX_SOURCE_BYTES, MAX_TEXT_CHARS, Rejection, SheetSpec,
    ValueType, error,
};
use cel::IdedExpr;
use cel::common::ast::{CallExpr, ComprehensionExpr, Expr, LiteralValue, operators};
use std::collections::{BTreeMap, BTreeSet};

/// CEL reserved words, literals, stdlib/type names, macro names and the custom catalogue.
const RESERVED: &[&str] = &[
    "in",
    "as",
    "break",
    "const",
    "continue",
    "else",
    "for",
    "function",
    "if",
    "import",
    "let",
    "loop",
    "package",
    "namespace",
    "return",
    "var",
    "void",
    "while",
    "true",
    "false",
    "null",
    "int",
    "uint",
    "double",
    "bool",
    "string",
    "bytes",
    "list",
    "map",
    "type",
    "null_type",
    "dyn",
    "size",
    "has",
    "all",
    "exists",
    "exists_one",
    "filter",
    "optional",
    "sum",
    "max_of",
    "min_of",
    "clamp",
    "max",
    "min",
    "item",
    "id",
];
/// Extra work units charged per comprehension iteration for list accumulation.
const ACCUMULATE_UNITS: u64 = 4;
const ITEM_ID_CHARS: usize = 64;
/// String comparison work is charged per this many scalar values of the shorter declared side.
const STRING_CHARS_PER_UNIT: u64 = 64;

pub(super) struct Lookup {
    scalars: BTreeMap<String, (ValueType, bool)>,
    lists: BTreeMap<String, BTreeMap<String, ValueType>>,
    optional: BTreeMap<String, BTreeSet<String>>,
    derived: BTreeMap<String, ValueType>,
}

fn reject(code: Code, target: &str) -> Rejection {
    Rejection {
        code,
        target: target.to_owned(),
    }
}

/// Lowercase ASCII CEL identifier: `[a-z][a-z0-9_]{0,63}`.
fn identifier(value: &str) -> bool {
    (1..=64).contains(&value.len())
        && value.as_bytes()[0].is_ascii_lowercase()
        && value
            .bytes()
            .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'_')
}

/// Registry slug used for constraint IDs and item IDs.
pub(super) fn slug(value: &str) -> bool {
    (1..=64).contains(&value.len())
        && value.as_bytes()[0].is_ascii_lowercase()
        && value
            .bytes()
            .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-')
}

fn name(value: &str, seen: &mut BTreeSet<String>) -> Result<(), Rejection> {
    if !identifier(value) {
        return Err(reject(Code::InvalidIdentifier, value));
    }
    if RESERVED.contains(&value) {
        return Err(reject(Code::ReservedIdentifier, value));
    }
    if !seen.insert(value.to_owned()) {
        return Err(reject(Code::DuplicateIdentifier, value));
    }
    Ok(())
}

fn value_type(ty: &ValueType, target: &str) -> Result<(), Rejection> {
    let valid = match ty {
        ValueType::Int | ValueType::Bool => true,
        ValueType::Text(max) => (1..=MAX_TEXT_CHARS).contains(max),
        ValueType::Enum(options) => {
            let unique: BTreeSet<&String> = options.iter().collect();
            (1..=MAX_OPTIONS).contains(&options.len())
                && unique.len() == options.len()
                && options.iter().all(|option| slug(option))
        }
    };
    if valid {
        Ok(())
    } else {
        Err(reject(Code::InvalidDeclaration, target))
    }
}

impl Lookup {
    pub(super) fn new(spec: &SheetSpec) -> Result<Self, Rejection> {
        let declarations = &spec.declarations;
        if declarations.fields.len() > MAX_FIELDS
            || declarations.lists.len() > MAX_LISTS
            || spec.derived.len() + spec.constraints.len() > MAX_FORMULAS
        {
            return Err(reject(Code::InvalidDeclaration, ""));
        }
        let mut seen = BTreeSet::new();
        let mut lookup = Self {
            scalars: BTreeMap::new(),
            lists: BTreeMap::new(),
            optional: BTreeMap::new(),
            derived: BTreeMap::new(),
        };
        for field in &declarations.fields {
            name(&field.id, &mut seen)?;
            value_type(&field.ty, &field.id)?;
            lookup
                .scalars
                .insert(field.id.clone(), (field.ty.clone(), field.nullable));
        }
        for list in &declarations.lists {
            name(&list.id, &mut seen)?;
            if list.properties.len() > MAX_PROPERTIES {
                return Err(reject(Code::InvalidDeclaration, &list.id));
            }
            let mut properties = BTreeMap::new();
            let mut optional = BTreeSet::new();
            let mut names = BTreeSet::new();
            for property in &list.properties {
                name(&property.id, &mut names)?;
                value_type(&property.ty, &property.id)?;
                properties.insert(property.id.clone(), property.ty.clone());
                if property.optional {
                    optional.insert(property.id.clone());
                }
            }
            lookup.lists.insert(list.id.clone(), properties);
            lookup.optional.insert(list.id.clone(), optional);
        }
        for derived in &spec.derived {
            name(&derived.id, &mut seen)?;
            value_type(&derived.ty, &derived.id)?;
            lookup
                .derived
                .insert(derived.id.clone(), derived.ty.clone());
        }
        let mut constraints = BTreeSet::new();
        for constraint in &spec.constraints {
            if !slug(&constraint.id) {
                return Err(reject(Code::InvalidIdentifier, &constraint.id));
            }
            if !constraints.insert(constraint.id.as_str()) {
                return Err(reject(Code::DuplicateIdentifier, &constraint.id));
            }
        }
        Ok(lookup)
    }

    pub(super) fn scalar(&self, id: &str) -> Option<&(ValueType, bool)> {
        self.scalars.get(id)
    }

    pub(super) fn scalars(&self) -> impl Iterator<Item = (&String, &(ValueType, bool))> {
        self.scalars.iter()
    }

    pub(super) fn list(
        &self,
        id: &str,
    ) -> Option<(&BTreeMap<String, ValueType>, &BTreeSet<String>)> {
        Some((self.lists.get(id)?, self.optional.get(id)?))
    }

    pub(super) fn lists(&self) -> impl Iterator<Item = &String> {
        self.lists.keys()
    }

    fn defines(&self, id: &str) -> bool {
        self.scalars.contains_key(id)
            || self.lists.contains_key(id)
            || self.derived.contains_key(id)
    }
}

pub(super) struct Checked {
    pub expr: IdedExpr,
    pub deps: BTreeSet<String>,
    /// Declared scalar fields the formula reads.
    pub reads: BTreeSet<String>,
    /// `(list, property)` values the formula reads through an iteration variable; a
    /// `has()` presence test is not a value read.
    pub item_reads: BTreeSet<(String, String)>,
}

/// Static type of a sub-expression.
#[derive(Clone, Debug, PartialEq, Eq)]
enum Ty {
    Int,
    Bool,
    /// Max length in scalar values, enum options if known, literal text if a literal.
    Str {
        max: usize,
        options: Option<Vec<String>>,
        literal: Option<String>,
    },
    Null,
    Record(String),
    List(Box<Ty>),
}

#[derive(Clone, Debug)]
struct Typed {
    ty: Ty,
    nullable: bool,
}

impl Typed {
    fn of(ty: Ty) -> Self {
        Self {
            ty,
            nullable: false,
        }
    }
}

fn from_value_type(ty: &ValueType) -> Ty {
    match ty {
        ValueType::Int => Ty::Int,
        ValueType::Bool => Ty::Bool,
        ValueType::Text(max) => Ty::Str {
            max: *max,
            options: None,
            literal: None,
        },
        ValueType::Enum(options) => Ty::Str {
            max: options
                .iter()
                .map(|option| option.chars().count())
                .max()
                .unwrap_or(0),
            options: Some(options.clone()),
            literal: None,
        },
    }
}

/// Parses with the recursion guard and checks size/depth before any typing.
fn parse(source: &str) -> Result<IdedExpr, Code> {
    if source.len() > MAX_SOURCE_BYTES {
        return Err(Code::SourceTooLong);
    }
    let expr = super::ENV
        .parser()
        .max_recursion_depth(MAX_PARSER_DEPTH)
        .parse(source)
        .map_err(|errors| error::from_parse(&errors))?;
    if depth_exceeds(&expr, MAX_AST_DEPTH) {
        return Err(Code::DepthExceeded);
    }
    Ok(expr)
}

fn children(expr: &IdedExpr) -> Vec<&IdedExpr> {
    match &expr.expr {
        Expr::Call(call) => call
            .target
            .iter()
            .map(AsRef::as_ref)
            .chain(call.args.iter())
            .collect(),
        Expr::Comprehension(comprehension) => vec![
            &comprehension.iter_range,
            &comprehension.accu_init,
            &comprehension.loop_cond,
            &comprehension.loop_step,
            &comprehension.result,
        ],
        Expr::Select(select) => vec![select.operand.as_ref()],
        Expr::List(list) => list.elements.iter().collect(),
        // Map/struct literals are rejected by the allowlist; for depth they count as leaves.
        _ => Vec::new(),
    }
}

/// Explicit-stack depth walk that stops as soon as `limit` is exceeded.
pub(super) fn depth_exceeds(root: &IdedExpr, limit: usize) -> bool {
    let mut stack = vec![(root, 1_usize)];
    while let Some((expr, depth)) = stack.pop() {
        if depth > limit {
            return true;
        }
        stack.extend(children(expr).into_iter().map(|child| (child, depth + 1)));
    }
    false
}

/// Compiles one formula. `ty` is the declared derived type; `None` means a boolean constraint.
pub(super) fn compile(
    lookup: &Lookup,
    id: &str,
    source: &str,
    ty: Option<&ValueType>,
) -> Result<(Checked, u64), Rejection> {
    let expr = parse(source).map_err(|code| reject(code, id))?;
    let mut checker = Checker {
        lookup,
        scope: Vec::new(),
        in_body: false,
        multiplier: 1,
        cost: 0,
        deps: BTreeSet::new(),
        reads: BTreeSet::new(),
        item_reads: BTreeSet::new(),
    };
    let typed = checker.check(&expr).map_err(|code| reject(code, id))?;
    let expected = ty.map_or(Ty::Bool, from_value_type);
    if !assignable(&typed, &expected) {
        return Err(reject(Code::TypeMismatch, id));
    }
    enum_guard(&expected, &typed.ty).map_err(|code| reject(code, id))?;
    Ok((
        Checked {
            expr,
            deps: checker.deps,
            reads: checker.reads,
            item_reads: checker.item_reads,
        },
        checker.cost,
    ))
}

fn assignable(typed: &Typed, expected: &Ty) -> bool {
    matches!(
        (&typed.ty, expected),
        (Ty::Int, Ty::Int) | (Ty::Bool, Ty::Bool) | (Ty::Str { .. }, Ty::Str { .. })
    )
}

/// The macro a comprehension node was expanded from, with its user-written argument.
enum Shape<'a> {
    Filter(&'a IdedExpr),
    Map(&'a IdedExpr),
    Exists(&'a IdedExpr),
    All(&'a IdedExpr),
}

const ACCUMULATOR: &str = "@result";

fn is_ident(expr: &IdedExpr, name: &str) -> bool {
    matches!(&expr.expr, Expr::Ident(ident) if ident == name)
}

fn is_bool(expr: &IdedExpr, value: bool) -> bool {
    matches!(&expr.expr, Expr::Literal(LiteralValue::Boolean(literal)) if *literal.inner() == value)
}

fn is_empty_list(expr: &IdedExpr) -> bool {
    matches!(&expr.expr, Expr::List(list) if list.elements.is_empty())
}

fn call<'a>(expr: &'a IdedExpr, name: &str, arity: usize) -> Option<&'a CallExpr> {
    match &expr.expr {
        Expr::Call(call)
            if call.func_name == name && call.target.is_none() && call.args.len() == arity =>
        {
            Some(call)
        }
        _ => None,
    }
}

/// `@result + [element]`
fn append(expr: &IdedExpr) -> Option<&IdedExpr> {
    let add = call(expr, operators::ADD, 2)?;
    if !is_ident(&add.args[0], ACCUMULATOR) {
        return None;
    }
    match &add.args[1].expr {
        Expr::List(list) if list.elements.len() == 1 => Some(&list.elements[0]),
        _ => None,
    }
}

/// Recognizes exactly the expansions of `filter`, `map` (two-argument), `exists` and `all`
/// produced by the pinned `cel` macros. Any other comprehension is rejected.
fn shape(comprehension: &ComprehensionExpr) -> Option<Shape<'_>> {
    if comprehension.iter_var2.is_some()
        || comprehension.accu_var != ACCUMULATOR
        || !is_ident(&comprehension.result, ACCUMULATOR)
    {
        return None;
    }
    let (init, cond, step) = (
        &comprehension.accu_init,
        &comprehension.loop_cond,
        &comprehension.loop_step,
    );
    if is_empty_list(init) && is_bool(cond, true) {
        if let Some(conditional) = call(step, operators::CONDITIONAL, 3) {
            let element = append(&conditional.args[1])?;
            return (is_ident(element, &comprehension.iter_var)
                && is_ident(&conditional.args[2], ACCUMULATOR))
            .then_some(Shape::Filter(&conditional.args[0]));
        }
        return append(step).map(Shape::Map);
    }
    let strict = call(cond, operators::NOT_STRICTLY_FALSE, 1)?;
    if is_bool(init, false) {
        let not = call(&strict.args[0], operators::LOGICAL_NOT, 1)?;
        let or = call(step, operators::LOGICAL_OR, 2)?;
        return (is_ident(&not.args[0], ACCUMULATOR) && is_ident(&or.args[0], ACCUMULATOR))
            .then_some(Shape::Exists(&or.args[1]));
    }
    if is_bool(init, true) {
        let and = call(step, operators::LOGICAL_AND, 2)?;
        return (is_ident(&strict.args[0], ACCUMULATOR) && is_ident(&and.args[0], ACCUMULATOR))
            .then_some(Shape::All(&and.args[1]));
    }
    None
}

struct Checker<'a> {
    lookup: &'a Lookup,
    scope: Vec<(String, Ty)>,
    in_body: bool,
    multiplier: u64,
    cost: u64,
    deps: BTreeSet<String>,
    reads: BTreeSet<String>,
    item_reads: BTreeSet<(String, String)>,
}

fn list_units() -> u64 {
    u64::try_from(MAX_LIST_ITEMS).unwrap_or(u64::MAX)
}

fn chars(value: usize) -> u64 {
    u64::try_from(value).unwrap_or(u64::MAX)
}

impl Checker<'_> {
    fn charge(&mut self, units: u64) {
        self.cost = self
            .cost
            .saturating_add(self.multiplier.saturating_mul(units));
    }

    fn check(&mut self, expr: &IdedExpr) -> Result<Typed, Code> {
        self.charge(1);
        match &expr.expr {
            Expr::Literal(literal) => literal_type(literal).map(Typed::of),
            Expr::Ident(ident) => self.ident(ident),
            Expr::Select(select) => {
                let Expr::Ident(variable) = &select.operand.expr else {
                    return Err(Code::UnsupportedConstruct);
                };
                let Some(Ty::Record(list)) = self.bound(variable) else {
                    // `item.x` outside an aggregate names nothing; selecting from a declared
                    // scalar or list is outside the profile.
                    return Err(
                        if self.bound(variable).is_none() && !self.lookup.defines(variable) {
                            Code::UnknownField
                        } else {
                            Code::UnsupportedConstruct
                        },
                    );
                };
                let property = self.property(&list, &select.field)?;
                if !select.test {
                    self.item_reads.insert((list, select.field.clone()));
                }
                Ok(Typed::of(if select.test { Ty::Bool } else { property }))
            }
            Expr::Call(call) => self.call(call),
            Expr::Comprehension(comprehension) => self.comprehension(comprehension),
            _ => Err(Code::UnsupportedConstruct),
        }
    }

    fn bound(&self, name: &str) -> Option<Ty> {
        self.scope
            .iter()
            .rev()
            .find(|(bound, _)| bound == name)
            .map(|(_, ty)| ty.clone())
    }

    fn property(&self, list: &str, property: &str) -> Result<Ty, Code> {
        if property == "id" {
            return Ok(Ty::Str {
                max: ITEM_ID_CHARS,
                options: None,
                literal: None,
            });
        }
        let (properties, _) = self.lookup.list(list).ok_or(Code::UnknownField)?;
        properties
            .get(property)
            .map(from_value_type)
            .ok_or(Code::UnknownProperty)
    }

    fn ident(&mut self, ident: &str) -> Result<Typed, Code> {
        if let Some(ty) = self.bound(ident) {
            return Ok(Typed::of(ty));
        }
        if let Some((ty, nullable)) = self.lookup.scalar(ident) {
            self.reads.insert(ident.to_owned());
            return Ok(Typed {
                ty: from_value_type(ty),
                nullable: *nullable,
            });
        }
        if let Some(ty) = self.lookup.derived.get(ident) {
            self.deps.insert(ident.to_owned());
            return Ok(Typed::of(from_value_type(ty)));
        }
        if self.lookup.lists.contains_key(ident) {
            return Ok(Typed::of(Ty::List(Box::new(Ty::Record(ident.to_owned())))));
        }
        Err(Code::UnknownField)
    }

    fn args(&mut self, call: &CallExpr) -> Result<Vec<Typed>, Code> {
        call.args.iter().map(|arg| self.check(arg)).collect()
    }

    fn call(&mut self, call: &CallExpr) -> Result<Typed, Code> {
        if call.target.is_some() {
            return Err(Code::UnsupportedConstruct);
        }
        let name = call.func_name.as_str();
        let arity = expected_arity(name).ok_or(Code::UnsupportedConstruct)?;
        if !arity.contains(&call.args.len()) {
            return Err(Code::ArityMismatch);
        }
        if name == operators::IN {
            return self.membership(call);
        }
        let args = self.args(call)?;
        let all = |ty: &Ty| args.iter().all(|arg| &arg.ty == ty);
        let result = match name {
            operators::LOGICAL_AND | operators::LOGICAL_OR | operators::LOGICAL_NOT
                if all(&Ty::Bool) =>
            {
                Ty::Bool
            }
            operators::ADD
            | operators::SUBSTRACT
            | operators::MULTIPLY
            | operators::DIVIDE
            | operators::MODULO
            | operators::NEGATE
            | "clamp"
            | "max"
            | "min"
                if all(&Ty::Int) =>
            {
                Ty::Int
            }
            operators::LESS
            | operators::LESS_EQUALS
            | operators::GREATER
            | operators::GREATER_EQUALS
                if all(&Ty::Int) =>
            {
                Ty::Bool
            }
            operators::EQUALS | operators::NOT_EQUALS => {
                self.equality(&args[0], &args[1])?;
                Ty::Bool
            }
            operators::CONDITIONAL if args[0].ty == Ty::Bool => {
                return merge(&args[1], &args[2]);
            }
            // List functions walk the whole list: charge one unit per possible item.
            "size" if matches!(args[0].ty, Ty::List(_)) => {
                self.charge(list_units());
                Ty::Int
            }
            "sum" if args[0].ty == Ty::List(Box::new(Ty::Int)) => {
                self.charge(list_units());
                Ty::Int
            }
            "max_of" | "min_of"
                if args[0].ty == Ty::List(Box::new(Ty::Int)) && args[1].ty == Ty::Int =>
            {
                self.charge(list_units());
                Ty::Int
            }
            _ => return Err(Code::TypeMismatch),
        };
        Ok(Typed::of(result))
    }

    fn equality(&mut self, left: &Typed, right: &Typed) -> Result<(), Code> {
        match (&left.ty, &right.ty) {
            (Ty::Int, Ty::Int) | (Ty::Bool, Ty::Bool) => Ok(()),
            (Ty::Null, _) if right.nullable => Ok(()),
            (_, Ty::Null) if left.nullable => Ok(()),
            (Ty::Str { max: a, .. }, Ty::Str { max: b, .. }) => {
                enum_guard(&left.ty, &right.ty)?;
                enum_guard(&right.ty, &left.ty)?;
                self.charge(chars(*a.min(b)) / STRING_CHARS_PER_UNIT + 1);
                Ok(())
            }
            _ => Err(Code::TypeMismatch),
        }
    }

    /// `x in [literal, ...]` with a scalar left side; lists of records are rejected.
    fn membership(&mut self, call: &CallExpr) -> Result<Typed, Code> {
        let left = self.check(&call.args[0])?;
        let Expr::List(list) = &call.args[1].expr else {
            return Err(Code::UnsupportedConstruct);
        };
        if list.elements.is_empty() || !list.optional_indices.is_empty() {
            return Err(Code::UnsupportedConstruct);
        }
        for element in &list.elements {
            let Expr::Literal(literal) = &element.expr else {
                return Err(Code::UnsupportedConstruct);
            };
            let item = Typed::of(literal_type(literal)?);
            if !matches!(
                (&left.ty, &item.ty),
                (Ty::Int, Ty::Int) | (Ty::Str { .. }, Ty::Str { .. })
            ) {
                return Err(Code::TypeMismatch);
            }
            self.equality(&left, &item)?;
        }
        Ok(Typed::of(Ty::Bool))
    }

    fn comprehension(&mut self, comprehension: &ComprehensionExpr) -> Result<Typed, Code> {
        if self.in_body {
            return Err(Code::NestedAggregate);
        }
        let shape = shape(comprehension).ok_or(Code::UnsupportedConstruct)?;
        let range = self.check(&comprehension.iter_range)?;
        let Ty::List(element) = range.ty else {
            return Err(Code::TypeMismatch);
        };
        let variable = &comprehension.iter_var;
        // `item` is reserved as a field name precisely so it can serve as the iteration variable.
        if !identifier(variable)
            || (variable != "item" && RESERVED.contains(&variable.as_str()))
            || self.lookup.defines(variable)
            || self.bound(variable).is_some()
        {
            return Err(Code::InvalidIterationVariable);
        }
        self.scope.push((variable.clone(), (*element).clone()));
        self.in_body = true;
        let saved = self.multiplier;
        self.multiplier = saved.saturating_mul(list_units());
        self.charge(ACCUMULATE_UNITS);
        let body = match shape {
            Shape::Filter(expr) | Shape::Map(expr) | Shape::Exists(expr) | Shape::All(expr) => {
                self.check(expr)
            }
        };
        self.multiplier = saved;
        self.in_body = false;
        self.scope.pop();
        let body = body?;
        match shape {
            Shape::Filter(_) if body.ty == Ty::Bool => Ok(Typed::of(Ty::List(element))),
            Shape::Map(_) if matches!(body.ty, Ty::Int | Ty::Bool | Ty::Str { .. }) => {
                Ok(Typed::of(Ty::List(Box::new(plain(body.ty)))))
            }
            Shape::Exists(_) | Shape::All(_) if body.ty == Ty::Bool => Ok(Typed::of(Ty::Bool)),
            _ => Err(Code::TypeMismatch),
        }
    }
}

/// Drops literal identity so list element types compare structurally.
fn plain(ty: Ty) -> Ty {
    match ty {
        Ty::Str { max, options, .. } => Ty::Str {
            max,
            options,
            literal: None,
        },
        other => other,
    }
}

fn enum_guard(maybe_enum: &Ty, maybe_literal: &Ty) -> Result<(), Code> {
    if let (
        Ty::Str {
            options: Some(options),
            ..
        },
        Ty::Str {
            literal: Some(text),
            ..
        },
    ) = (maybe_enum, maybe_literal)
        && !options.contains(text)
    {
        return Err(Code::UnknownEnumOption);
    }
    Ok(())
}

fn merge(left: &Typed, right: &Typed) -> Result<Typed, Code> {
    let ty = match (&left.ty, &right.ty) {
        (Ty::Int, Ty::Int) => Ty::Int,
        (Ty::Bool, Ty::Bool) => Ty::Bool,
        (
            Ty::Str {
                max: left_max,
                options: left_options,
                literal: left_literal,
            },
            Ty::Str {
                max: right_max,
                options: right_options,
                literal: right_literal,
            },
        ) => Ty::Str {
            max: *left_max.max(right_max),
            options: match (left_options, right_options) {
                (Some(left), Some(right)) if left == right => Some(left.clone()),
                _ => None,
            },
            literal: if left_literal == right_literal {
                left_literal.clone()
            } else {
                None
            },
        },
        _ => return Err(Code::TypeMismatch),
    };
    Ok(Typed {
        ty,
        nullable: left.nullable || right.nullable,
    })
}

fn literal_type(literal: &LiteralValue) -> Result<Ty, Code> {
    match literal {
        LiteralValue::Int(_) => Ok(Ty::Int),
        LiteralValue::Boolean(_) => Ok(Ty::Bool),
        LiteralValue::String(text) => Ok(Ty::Str {
            max: text.inner().chars().count(),
            options: None,
            literal: Some(text.inner().to_owned()),
        }),
        LiteralValue::Null => Ok(Ty::Null),
        _ => Err(Code::UnsupportedConstruct),
    }
}

fn expected_arity(name: &str) -> Option<std::ops::RangeInclusive<usize>> {
    Some(match name {
        operators::LOGICAL_NOT | operators::NEGATE | "size" | "sum" => 1..=1,
        operators::LOGICAL_AND
        | operators::LOGICAL_OR
        | operators::ADD
        | operators::SUBSTRACT
        | operators::MULTIPLY
        | operators::DIVIDE
        | operators::MODULO
        | operators::LESS
        | operators::LESS_EQUALS
        | operators::GREATER
        | operators::GREATER_EQUALS
        | operators::EQUALS
        | operators::NOT_EQUALS
        | operators::IN
        | "max_of"
        | "min_of" => 2..=2,
        operators::CONDITIONAL | "clamp" => 3..=3,
        "max" | "min" => 2..=MAX_FIELDS,
        _ => return None,
    })
}

#[cfg(test)]
pub(super) fn children_for_tests(expr: &IdedExpr) -> Vec<&IdedExpr> {
    children(expr)
}

#[cfg(test)]
pub(super) fn shape_recognized(comprehension: &ComprehensionExpr) -> bool {
    shape(comprehension).is_some()
}
