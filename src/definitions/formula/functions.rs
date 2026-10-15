//! Mimik's custom CEL function catalogue. Every function validates its own arguments.
use super::error::{Code, function_error};
use cel::{Context, DeclarationError, ExecutionError, Value, extractors::Arguments};

pub(super) const CATALOGUE: &[&str] = &["sum", "max_of", "min_of", "clamp", "max", "min"];

pub(super) fn is_catalogue(name: &str) -> bool {
    CATALOGUE.contains(&name)
}

fn ints(function: &str, list: &Value) -> Result<Vec<i64>, ExecutionError> {
    let Value::List(items) = list else {
        return Err(function_error(function, Code::TypeMismatch));
    };
    items
        .iter()
        .map(|item| match item {
            Value::Int(value) => Ok(*value),
            Value::Null => Err(function_error(function, Code::NullValue)),
            _ => Err(function_error(function, Code::TypeMismatch)),
        })
        .collect()
}

fn int(function: &str, value: &Value) -> Result<i64, ExecutionError> {
    match value {
        Value::Int(value) => Ok(*value),
        Value::Null => Err(function_error(function, Code::NullValue)),
        _ => Err(function_error(function, Code::TypeMismatch)),
    }
}

fn sum(list: &Value) -> Result<Value, ExecutionError> {
    let mut total: i64 = 0;
    for value in ints("sum", list)? {
        total = total
            .checked_add(value)
            .ok_or_else(|| function_error("sum", Code::Overflow))?;
    }
    Ok(Value::Int(total))
}

fn extreme_of(
    function: &str,
    list: &Value,
    default: &Value,
    max: bool,
) -> Result<Value, ExecutionError> {
    let values = ints(function, list)?;
    let default = int(function, default)?;
    let found = if max {
        values.into_iter().max()
    } else {
        values.into_iter().min()
    };
    Ok(Value::Int(found.unwrap_or(default)))
}

fn variadic(function: &str, arguments: &[Value], max: bool) -> Result<Value, ExecutionError> {
    if arguments.len() < 2 {
        return Err(function_error(function, Code::ArityMismatch));
    }
    let values = arguments
        .iter()
        .map(|value| int(function, value))
        .collect::<Result<Vec<_>, _>>()?;
    let found = if max {
        values.into_iter().max()
    } else {
        values.into_iter().min()
    };
    found
        .map(Value::Int)
        .ok_or_else(|| function_error(function, Code::ArityMismatch))
}

fn clamp(Arguments(arguments): Arguments) -> Result<Value, ExecutionError> {
    let [x, low, high] = arguments.as_slice() else {
        return Err(function_error("clamp", Code::ArityMismatch));
    };
    let (x, low, high) = (int("clamp", x)?, int("clamp", low)?, int("clamp", high)?);
    if low > high {
        return Err(function_error("clamp", Code::InvalidArgument));
    }
    Ok(Value::Int(x.clamp(low, high)))
}

fn pair(function: &'static str, arguments: &[Value], max: bool) -> Result<Value, ExecutionError> {
    let [list, default] = arguments else {
        return Err(function_error(function, Code::ArityMismatch));
    };
    extreme_of(function, list, default, max)
}

/// Registers the catalogue on a root context. Names never shadow `cel` stdlib overloads:
/// `add_function` refuses such collisions.
pub(super) fn register(context: &mut Context<'_, '_>) -> Result<(), DeclarationError> {
    context.add_function("sum", |Arguments(arguments): Arguments| {
        match arguments.as_slice() {
            [list] => sum(list),
            _ => Err(function_error("sum", Code::ArityMismatch)),
        }
    })?;
    context.add_function("max_of", |Arguments(arguments): Arguments| {
        pair("max_of", &arguments, true)
    })?;
    context.add_function("min_of", |Arguments(arguments): Arguments| {
        pair("min_of", &arguments, false)
    })?;
    context.add_function("clamp", clamp)?;
    context.add_function("max", |Arguments(arguments): Arguments| {
        variadic("max", &arguments, true)
    })?;
    context.add_function("min", |Arguments(arguments): Arguments| {
        variadic("min", &arguments, false)
    })?;
    Ok(())
}
