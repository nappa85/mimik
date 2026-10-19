//! Lenient number entry on runtime sheets (assisted sheet plan §3a, slice 1.1).
//!
//! The player types into an integer cell what they would write on paper: a literal
//! ("-15", "+3", "0007") or an absolute sum ("84 -15", "84-15+3"). A sign-led input is
//! always the literal value (decision D-Q). Anything else is [`Unreadable`]. This parser
//! is for user input only: protocol integers such as submitted revisions keep the strict
//! [`super::values::integer`].

/// The most characters an entry may have, whatever the form's `maxlength`.
pub(crate) const MAX_CHARS: usize = 200;
/// The most numbers one sum may have.
pub(crate) const MAX_SCALARS: usize = 64;

/// Why an entry could not be read as a number.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Unreadable {
    /// Empty, or spaces and tabs only (an unknown value from slice 1.4 on).
    Blank,
    /// More than [`MAX_CHARS`] characters.
    TooLong,
    /// NUL or another control character other than tab.
    Control,
    /// Neither a literal nor a sum.
    Syntax,
    /// More than [`MAX_SCALARS`] numbers.
    TooManyScalars,
    /// A term or the result is outside the signed 64-bit range.
    Range,
}

impl Unreadable {
    /// The refusal reason shown with the kept draft.
    pub(crate) fn message(self) -> &'static str {
        match self {
            Self::Blank => "A number is empty; enter a whole number or a sum such as 84 -15.",
            Self::TooLong => "A number entry has more than 200 characters.",
            Self::Control => "A number entry contains a control character.",
            Self::Syntax => "A number is unreadable; enter a whole number or a sum such as 84 -15.",
            Self::TooManyScalars => "A sum has more than 64 numbers.",
            Self::Range => "Integer outside signed i64 range.",
        }
    }
}

/// Reads a player's number entry: a literal or an absolute sum (plan §3a grammar).
///
/// ```text
/// int     = DIGIT+                     ; ASCII only; leading zeros allowed
/// sign    = "+" | "-" | "−"            ; U+2212 accepted
/// literal = [sign] int                 ; a sign must touch its digits
/// sum     = int 1*( ws* sign ws* int ) ; unsigned first term; ws = space or tab
/// ```
///
/// Leading and trailing spaces and tabs are trimmed. Terms are added left to right in
/// `i128`, each checked as it is parsed and added; the result must fit `i64`.
pub(crate) fn number(raw: &str) -> Result<i64, Unreadable> {
    if raw.chars().nth(MAX_CHARS).is_some() {
        return Err(Unreadable::TooLong);
    }
    if raw.chars().any(|c| c.is_control() && c != '\t') {
        return Err(Unreadable::Control);
    }
    let text = raw.trim_matches(is_space);
    if text.is_empty() {
        return Err(Unreadable::Blank);
    }
    let mut total: i128 = 0;
    for (negative, run) in terms(text)? {
        let term = magnitude(run)?;
        total = if negative {
            total.checked_sub(term)
        } else {
            total.checked_add(term)
        }
        .ok_or(Unreadable::Range)?;
    }
    i64::try_from(total).map_err(|_| Unreadable::Range)
}

/// Splits trimmed, non-empty text into signed digit runs: one literal, or a sum.
fn terms(text: &str) -> Result<Vec<(bool, &str)>, Unreadable> {
    // A sign-led input is a literal: the sign must touch the digits and nothing may follow.
    if let Some((negative, after)) = strip_sign(text) {
        let (run, tail) = split_digits(after);
        if run.is_empty() || !tail.is_empty() {
            return Err(Unreadable::Syntax);
        }
        return Ok(vec![(negative, run)]);
    }
    let (run, mut rest) = split_digits(text);
    if run.is_empty() {
        return Err(Unreadable::Syntax);
    }
    let mut terms = vec![(false, run)];
    loop {
        rest = rest.trim_start_matches(is_space);
        if rest.is_empty() {
            return Ok(terms);
        }
        let (negative, after) = strip_sign(rest).ok_or(Unreadable::Syntax)?;
        let (run, tail) = split_digits(after.trim_start_matches(is_space));
        if run.is_empty() {
            return Err(Unreadable::Syntax);
        }
        if terms.len() == MAX_SCALARS {
            return Err(Unreadable::TooManyScalars);
        }
        terms.push((negative, run));
        rest = tail;
    }
}

fn is_space(c: char) -> bool {
    matches!(c, ' ' | '\t')
}

/// A leading sign and the text after it; `true` is minus.
fn strip_sign(text: &str) -> Option<(bool, &str)> {
    let mut chars = text.chars();
    let negative = match chars.next()? {
        '+' => false,
        '-' | '\u{2212}' => true,
        _ => return None,
    };
    Some((negative, chars.as_str()))
}

/// The leading run of ASCII digits and the rest.
fn split_digits(text: &str) -> (&str, &str) {
    text.split_at(
        text.find(|c: char| !c.is_ascii_digit())
            .unwrap_or(text.len()),
    )
}

/// The value of an ASCII digit run, accumulated with checked `i128` arithmetic.
fn magnitude(run: &str) -> Result<i128, Unreadable> {
    run.bytes().try_fold(0_i128, |value, digit| {
        value
            .checked_mul(10)
            .and_then(|value| value.checked_add(i128::from(digit - b'0')))
            .ok_or(Unreadable::Range)
    })
}

#[cfg(test)]
#[path = "entry_tests.rs"]
mod tests;
