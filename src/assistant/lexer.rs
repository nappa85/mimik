//! The phrase-number lexer and the fraction words of literal provenance (assisted sheet plan
//! §1.4, "Output contract"; frozen for the 2b screen, ASSIST-38).
//!
//! Separate from number entry (`campaigns::entry::read` reads whole entries and is not
//! changed): this finds the numbers a player's phrase spells, so a model's literal can be
//! traced to one of them. Digits err towards finding nothing: a false rejection only drops an
//! option. Number words (ASSIST-53) err the other way: a homograph ("sei" is also "you are",
//! "no one" is not 1) admits a number the player did not mean, so the option is shown and the
//! owner confirms it (`docs/limitations.md`).
use unicode_general_category::{GeneralCategory, get_general_category};

/// The most digits of a number the lexer finds.
const MAX_DIGITS: usize = 18;

/// The decimal separators: a run preceded by one, or followed by one that touches a digit,
/// is part of a decimal or grouped number and gives nothing.
fn is_separator(c: char) -> bool {
    matches!(
        c,
        '.' | ',' | '\u{066b}' | '\u{066c}' | '\u{ff0c}' | '\u{ff0e}'
    )
}

/// Group joiners: two runs joined by one of these ("1 500", "1'500") both give nothing.
pub(crate) fn is_joiner(c: char) -> bool {
    matches!(c, '\u{a0}' | '\u{202f}' | '\'' | '\u{2019}')
}

fn is_letter(c: char) -> bool {
    use GeneralCategory as G;
    matches!(
        get_general_category(c),
        G::UppercaseLetter
            | G::LowercaseLetter
            | G::TitlecaseLetter
            | G::ModifierLetter
            | G::OtherLetter
    )
}

/// Any Unicode number character (`N*`: decimal digits, letter numbers such as Roman
/// numerals, other numbers such as `²` or `①`).
fn is_number(c: char) -> bool {
    use GeneralCategory as G;
    matches!(
        get_general_category(c),
        G::DecimalNumber | G::LetterNumber | G::OtherNumber
    )
}

/// The value of a decimal digit (`Nd`), ASCII or not; `None` for anything else. Unicode
/// encodes every `Nd` digit in contiguous runs of ten, 0 to 9, so the value is the offset
/// from the start of the contiguous `Nd` range, modulo 10.
fn decimal(c: char) -> Option<u8> {
    if c.is_ascii_digit() {
        return Some(c as u8 - b'0');
    }
    let decimal = |code: u32| {
        char::from_u32(code)
            .is_some_and(|c| get_general_category(c) == GeneralCategory::DecimalNumber)
    };
    let code = u32::from(c);
    if !decimal(code) {
        return None;
    }
    let mut start = code;
    while start > 0 && decimal(start - 1) {
        start -= 1;
    }
    u8::try_from((code - start) % 10).ok()
}

/// The numbers a phrase spells, in order, duplicates kept (plan §1.4): maximal runs of
/// decimal digits (folded to ASCII) whose neighbours are neither letters nor other number
/// characters, not preceded by a separator, not followed by a separator that touches a
/// digit, not joined to another run by a single joiner, without a leading zero and at most
/// 18 digits long. Signs are not part of a number.
pub(crate) fn phrase_numbers(phrase: &str) -> Vec<u64> {
    phrase_number_spans(phrase)
        .into_iter()
        .map(|(value, _, _)| value)
        .collect()
}

/// [`phrase_numbers`] with where each number sits: `(value, start, end)`, the character
/// offsets (not bytes) of its digits in `phrase`, in order.
pub(crate) fn phrase_number_spans(phrase: &str) -> Vec<(u64, usize, usize)> {
    let chars: Vec<char> = phrase.chars().collect();
    let mut runs = Vec::new();
    let mut i = 0;
    while i < chars.len() {
        if decimal(chars[i]).is_none() {
            i += 1;
            continue;
        }
        let start = i;
        while i < chars.len() && decimal(chars[i]).is_some() {
            i += 1;
        }
        runs.push((start, i));
    }
    let mut rejected = vec![false; runs.len()];
    for (index, &(start, end)) in runs.iter().enumerate() {
        let before = start.checked_sub(1).map(|at| chars[at]);
        let after = chars.get(end).copied();
        let after2 = chars.get(end + 1).copied();
        if before.is_some_and(|c| is_letter(c) || is_number(c) || is_separator(c)) {
            rejected[index] = true;
        }
        if after.is_some_and(|c| is_letter(c) || is_number(c)) {
            rejected[index] = true;
        }
        if after.is_some_and(is_separator) && after2.is_some_and(|c| decimal(c).is_some()) {
            rejected[index] = true;
        }
        if after.is_some_and(is_joiner) && runs.get(index + 1).is_some_and(|next| next.0 == end + 1)
        {
            rejected[index] = true;
            rejected[index + 1] = true;
        }
    }
    runs.iter()
        .zip(rejected)
        .filter(|(_, rejected)| !rejected)
        .filter_map(|(&(start, end), _)| {
            let digits: Vec<u8> = chars[start..end]
                .iter()
                .filter_map(|&c| decimal(c))
                .collect();
            if digits.len() > MAX_DIGITS || (digits.len() > 1 && digits[0] == 0) {
                return None;
            }
            Some((
                digits
                    .iter()
                    .fold(0_u64, |value, &digit| value * 10 + u64::from(digit)),
                start,
                end,
            ))
        })
        .collect()
}

/// The fraction and percent words (EN, IT), after case and diacritic folding ("metà" is
/// "meta"); "per cento" is matched as two words.
const FRACTION_WORDS: [&str; 12] = [
    "half", "third", "quarter", "percent", "meta", "mezzo", "mezza", "terzo", "terza", "quarto",
    "quarta", "percento",
];

/// The only literals besides the phrase's numbers and the target's value that a fraction
/// or percent word admits (ASSIST-38).
pub(crate) const FRACTION_LITERALS: [u64; 4] = [2, 3, 4, 100];

/// Lower case, decomposed, combining marks removed.
fn fold(text: &str) -> String {
    crate::text_fold::strip_marks(
        &text
            .chars()
            .flat_map(char::to_lowercase)
            .collect::<String>(),
    )
}

/// Whether the phrase contains a fraction or percent word as a whole word (letters and
/// digits form words), or `%`.
pub(crate) fn has_fraction_word(phrase: &str) -> bool {
    if phrase.contains('%') {
        return true;
    }
    let folded = fold(phrase);
    let words: Vec<&str> = folded
        .split(|c: char| !(is_letter(c) || decimal(c).is_some()))
        .filter(|word| !word.is_empty())
        .collect();
    words.iter().any(|word| FRACTION_WORDS.contains(word))
        || words.windows(2).any(|pair| pair == ["per", "cento"])
}

/// The number words (EN, IT) of ASSIST-53, after case and diacritic folding: zero to twenty,
/// the tens to ninety and one hundred; `uno`, `una` and `un` (so also an elided `un'`) are 1.
/// "a" and "an" are articles and are not here. Compound words ("ventuno") are not words of
/// this list and give nothing; a hyphenated "twenty-one" gives its two parts, 20 and 1.
const NUMBER_WORDS: [(&str, u64); 59] = [
    ("zero", 0),
    ("one", 1),
    ("two", 2),
    ("three", 3),
    ("four", 4),
    ("five", 5),
    ("six", 6),
    ("seven", 7),
    ("eight", 8),
    ("nine", 9),
    ("ten", 10),
    ("eleven", 11),
    ("twelve", 12),
    ("thirteen", 13),
    ("fourteen", 14),
    ("fifteen", 15),
    ("sixteen", 16),
    ("seventeen", 17),
    ("eighteen", 18),
    ("nineteen", 19),
    ("twenty", 20),
    ("thirty", 30),
    ("forty", 40),
    ("fifty", 50),
    ("sixty", 60),
    ("seventy", 70),
    ("eighty", 80),
    ("ninety", 90),
    ("hundred", 100),
    ("uno", 1),
    ("una", 1),
    ("un", 1),
    ("due", 2),
    ("tre", 3),
    ("quattro", 4),
    ("cinque", 5),
    ("sei", 6),
    ("sette", 7),
    ("otto", 8),
    ("nove", 9),
    ("dieci", 10),
    ("undici", 11),
    ("dodici", 12),
    ("tredici", 13),
    ("quattordici", 14),
    ("quindici", 15),
    ("sedici", 16),
    ("diciassette", 17),
    ("diciotto", 18),
    ("diciannove", 19),
    ("venti", 20),
    ("trenta", 30),
    ("quaranta", 40),
    ("cinquanta", 50),
    ("sessanta", 60),
    ("settanta", 70),
    ("ottanta", 80),
    ("novanta", 90),
    ("cento", 100),
];

/// The numbers a phrase spells as words (ASSIST-53), in order, duplicates kept: whole words
/// only (letters and digits form words, so "someone" and "one5" give nothing), any case.
pub(crate) fn number_words(phrase: &str) -> Vec<u64> {
    fold(phrase)
        .split(|c: char| !(is_letter(c) || decimal(c).is_some()))
        .filter_map(|word| {
            NUMBER_WORDS
                .iter()
                .find(|(name, _)| *name == word)
                .map(|&(_, value)| value)
        })
        .collect()
}

#[cfg(test)]
#[path = "lexer_tests.rs"]
mod tests;
