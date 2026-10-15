//! Stable Mimik codes for formula admission and evaluation, mapped from pinned `cel` errors.
use cel::{ExecutionError, ParseErrors, Value};
use serde::{Serialize, Serializer};
use std::fmt;

macro_rules! codes {
    ($($variant:ident => $text:literal),+ $(,)?) => {
        /// A stable `snake_case` code. Wording from `cel` is never shown or persisted.
        #[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
        pub enum Code { $($variant),+ }

        impl Code {
            pub const ALL: &'static [Code] = &[$(Code::$variant),+];

            #[must_use]
            pub fn as_str(self) -> &'static str {
                match self { $(Code::$variant => $text),+ }
            }

            #[must_use]
            pub fn parse(text: &str) -> Option<Self> {
                match text { $($text => Some(Code::$variant),)+ _ => None }
            }
        }
    };
}

codes! {
    ParseError => "parse_error",
    SourceTooLong => "source_too_long",
    DepthExceeded => "depth_exceeded",
    NestedAggregate => "nested_aggregate",
    UnsupportedConstruct => "unsupported_construct",
    ArityMismatch => "arity_mismatch",
    UnknownField => "unknown_field",
    UnknownProperty => "unknown_property",
    UnknownEnumOption => "unknown_enum_option",
    InvalidIdentifier => "invalid_identifier",
    ReservedIdentifier => "reserved_identifier",
    DuplicateIdentifier => "duplicate_identifier",
    InvalidIterationVariable => "invalid_iteration_variable",
    InvalidDeclaration => "invalid_declaration",
    FormulaCycle => "formula_cycle",
    CostLimit => "cost_limit",
    CollectionTooLarge => "collection_too_large",
    InvalidInput => "invalid_input",
    InputTooLarge => "input_too_large",
    MissingProperty => "missing_property",
    NullValue => "null_value",
    TypeMismatch => "type_mismatch",
    Overflow => "overflow",
    DivisionByZero => "division_by_zero",
    InvalidArgument => "invalid_argument",
    InvalidResult => "invalid_result",
    DependencyFailed => "dependency_failed",
    DeadlineExceeded => "deadline_exceeded",
    InternalError => "internal_error",
}

impl fmt::Display for Code {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}

impl Serialize for Code {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(self.as_str())
    }
}

/// Custom functions report Mimik codes through `function_error(name, code)`.
pub(super) fn function_error(function: &str, code: Code) -> ExecutionError {
    ExecutionError::function_error(function, code.as_str())
}

/// The recursion guard is the only parse failure distinguished from a syntax error. The
/// pinned `cel` reports it only in message text, so the match is gated by a test per version.
pub(super) fn from_parse(errors: &ParseErrors) -> Code {
    if errors.to_string().contains("Recursion limit of") {
        Code::DepthExceeded
    } else {
        Code::ParseError
    }
}

fn null_involved(values: &[&Value]) -> bool {
    values.iter().any(|value| matches!(value, Value::Null))
}

/// Maps runtime errors of the pinned `cel` version. Unknown or new variants fail closed.
pub(super) fn from_execution(error: &ExecutionError) -> Code {
    match error {
        ExecutionError::Overflow(..) => Code::Overflow,
        ExecutionError::DivisionByZero(_) | ExecutionError::RemainderByZero(_) => {
            Code::DivisionByZero
        }
        ExecutionError::NoSuchKey(_) => Code::MissingProperty,
        ExecutionError::UndeclaredReference(_) => Code::UnknownField,
        ExecutionError::InvalidArgumentCount { .. } => Code::ArityMismatch,
        ExecutionError::FunctionError { function, message } => {
            match (
                super::functions::is_catalogue(function),
                Code::parse(message),
            ) {
                (true, Some(code)) => code,
                _ => Code::InternalError,
            }
        }
        ExecutionError::UnsupportedBinaryOperator(_, left, right)
        | ExecutionError::ValuesNotComparable(left, right) => {
            if null_involved(&[left, right]) {
                Code::NullValue
            } else {
                Code::TypeMismatch
            }
        }
        ExecutionError::NoSuchOverload(overload) => {
            if overload
                .argument_types()
                .iter()
                .any(|name| name == "null_type")
            {
                Code::NullValue
            } else {
                Code::TypeMismatch
            }
        }
        ExecutionError::UnexpectedType { .. } | ExecutionError::UnsupportedTargetType { .. } => {
            Code::TypeMismatch
        }
        _ => Code::InternalError,
    }
}
