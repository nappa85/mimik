//! Stat line: lines of repeated `KEY value` pairs (`Str 12 Dex +1 HP 2d6`) or `Key: value`
//! sequences separated by `,` or `;`.
use std::sync::LazyLock;

use regex::Regex;

use super::{Draft, Problem, Shape, caption_candidate, excerpt, is_value};

/// A line must carry at least this many pairs.
const MIN_PAIRS: usize = 2;
const STAT_CAP: usize = 95;
const MAX_VALUE_CHARS: usize = 24;

type Pairs = Vec<(String, String)>;

fn key_spaced(token: &str) -> bool {
    static KEY: LazyLock<Regex> =
        LazyLock::new(|| Regex::new(r"^[A-Z][A-Za-z.]{0,11}$").expect("static pattern"));
    KEY.is_match(token)
}

fn key_colon(token: &str) -> bool {
    static KEY: LazyLock<Regex> =
        LazyLock::new(|| Regex::new(r"^[A-Za-z][A-Za-z.]{0,11}$").expect("static pattern"));
    KEY.is_match(token)
}

fn unique(pairs: &Pairs) -> bool {
    let mut keys: Vec<&str> = pairs.iter().map(|(k, _)| k.as_str()).collect();
    keys.sort_unstable();
    keys.windows(2).all(|w| w[0] != w[1])
}

/// `KEY value KEY value`: every token belongs to a pair.
fn spaced_pairs(line: &str) -> Option<Pairs> {
    let tokens: Vec<&str> = line.split_whitespace().collect();
    if tokens.len() < 2 * MIN_PAIRS || !tokens.len().is_multiple_of(2) {
        return None;
    }
    let pairs: Pairs = tokens
        .chunks(2)
        .map(|pair| {
            let value = pair[1].trim_end_matches([',', ';']);
            (key_spaced(pair[0]) && is_value(value)).then(|| (pair[0].to_owned(), value.to_owned()))
        })
        .collect::<Option<_>>()?;
    unique(&pairs).then_some(pairs)
}

/// `Key: value, Key: value`.
fn colon_pairs(line: &str) -> Option<Pairs> {
    let parts: Vec<&str> = line.split([',', ';']).map(str::trim).collect();
    if parts.len() < MIN_PAIRS {
        return None;
    }
    let pairs: Pairs = parts
        .iter()
        .map(|part| {
            let (key, value) = part.split_once(':')?;
            let (key, value) = (key.trim(), value.trim());
            (key_colon(key)
                && !value.is_empty()
                && value.chars().count() <= MAX_VALUE_CHARS
                && !value.contains(':'))
            .then(|| (key.to_owned(), value.to_owned()))
        })
        .collect::<Option<_>>()?;
    unique(&pairs).then_some(pairs)
}

pub(super) fn read(lines: &[String]) -> Draft {
    let mut stat_lines: Vec<Pairs> = Vec::new();
    let mut caption = None;
    let mut unplaced = Vec::new();
    for (index, line) in lines.iter().enumerate() {
        if let Some(pairs) = spaced_pairs(line).or_else(|| colon_pairs(line)) {
            stat_lines.push(pairs);
        } else if index == 0 && caption_candidate(line) {
            caption = Some(line.clone());
        } else {
            unplaced.push(excerpt(line));
        }
    }
    let body = lines.len() - usize::from(caption.is_some());
    if stat_lines.is_empty() || stat_lines.len() * 2 < body {
        return Draft::none(Shape::Stat, Vec::new());
    }
    let same_keys = stat_lines.len() >= 2
        && stat_lines.iter().all(|line| {
            line.iter()
                .map(|(k, _)| k)
                .eq(stat_lines[0].iter().map(|(k, _)| k))
        });
    let (rows, header): (Vec<Vec<String>>, bool) = if same_keys {
        let mut rows = vec![stat_lines[0].iter().map(|(k, _)| k.clone()).collect()];
        rows.extend(
            stat_lines
                .iter()
                .map(|line| line.iter().map(|(_, v)| v.clone()).collect()),
        );
        (rows, true)
    } else {
        let rows = stat_lines
            .iter()
            .flatten()
            .map(|(k, v)| vec![k.clone(), v.clone()])
            .collect();
        (rows, false)
    };
    let consumed = stat_lines.len() + usize::from(caption.is_some());
    let score = u32::try_from(STAT_CAP * consumed / lines.len()).unwrap_or(0);
    Draft::build(
        Shape::Stat,
        rows,
        header,
        caption,
        score,
        unplaced.into_iter().map(Problem::Unplaced).collect(),
    )
}
