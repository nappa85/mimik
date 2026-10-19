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
    /// A term, or the running sum after a term, overflows `i128`, or the final result is
    /// outside `i64`. A single term or the running sum may exceed `i64` when the final
    /// result fits.
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

/// A readable entry: its value and whether it was a sum of two or more numbers.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Reading {
    pub value: i64,
    /// Two or more terms ("84 -15"); a literal ("84", "-15", "+3") is not a sum.
    pub sum: bool,
}

impl Unreadable {
    /// Whether a known saved value is kept, with the input recorded as a scribble (plan
    /// §3a, slice 1.2). Blank (an unknown value from slice 1.4), over-long and
    /// control-character input is refused instead: it cannot be kept as a faithful scribble.
    pub(crate) fn keeps_value(self) -> bool {
        matches!(self, Self::Syntax | Self::TooManyScalars | Self::Range)
    }
}

/// Reads a player's number entry; see [`number`].
pub(crate) fn read(raw: &str) -> Result<Reading, Unreadable> {
    let (value, terms) = evaluate(raw)?;
    Ok(Reading {
        value,
        sum: terms > 1,
    })
}

/// The sum an entry spelled, normalised for display ("84 -15" → "84 − 15"), or `None` when
/// the entry is not a readable sum. Magnitudes lose leading zeros; minus is U+2212.
pub(crate) fn sum_text(raw: &str) -> Option<String> {
    if !read(raw).is_ok_and(|reading| reading.sum) {
        return None;
    }
    let text = raw.trim_matches(is_space);
    let mut shown = String::new();
    for (index, (negative, run)) in terms(text).ok()?.into_iter().enumerate() {
        let magnitude = magnitude(run).ok()?;
        if index > 0 {
            shown.push_str(if negative { " \u{2212} " } else { " + " });
        }
        shown.push_str(&magnitude.to_string());
    }
    Some(shown)
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
    evaluate(raw).map(|(value, _)| value)
}

/// The value of an entry and its number of terms.
fn evaluate(raw: &str) -> Result<(i64, usize), Unreadable> {
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
    let terms = terms(text)?;
    let count = terms.len();
    for (negative, run) in terms {
        let term = magnitude(run)?;
        total = if negative {
            total.checked_sub(term)
        } else {
            total.checked_add(term)
        }
        .ok_or(Unreadable::Range)?;
    }
    i64::try_from(total)
        .map(|value| (value, count))
        .map_err(|_| Unreadable::Range)
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
