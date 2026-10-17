//! URL decoding remains `serde_urlencoded`'s responsibility; retain pairs until closed-key admission.
use serde::de::{DeserializeSeed, SeqAccess, Visitor};
use std::{
    borrow::Borrow,
    collections::{BTreeMap, BTreeSet, HashSet},
    fmt,
    hash::Hash,
};

/// The pair cap of every form except the private sheet editor.
const FORM_PAIRS: usize = 516;
/// The pair cap of private sheet create/edit POSTs (list rows, plan D2 and D7).
pub(super) const SHEET_FORM_PAIRS: usize = 65_536;

pub(super) struct Pairs(Vec<(String, String)>);

/// Deserializes at most `limit` unique pairs with keys of at most 70 bytes.
struct PairsSeed {
    limit: usize,
}

impl<'de> DeserializeSeed<'de> for PairsSeed {
    type Value = Pairs;
    fn deserialize<D: serde::Deserializer<'de>>(self, decoder: D) -> Result<Pairs, D::Error> {
        decoder.deserialize_seq(self)
    }
}

impl<'de> Visitor<'de> for PairsSeed {
    type Value = Pairs;
    fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "at most {} unique URL form pairs", self.limit)
    }
    fn visit_seq<A: SeqAccess<'de>>(self, mut sequence: A) -> Result<Pairs, A::Error> {
        let mut pairs = Vec::new();
        let mut seen = BTreeSet::new();
        while let Some((key, value)) = sequence.next_element::<(String, String)>()? {
            if pairs.len() >= self.limit || key.len() > 70 || !seen.insert(key.clone()) {
                return Err(serde::de::Error::custom(
                    "duplicate key or form expansion limit",
                ));
            }
            pairs.push((key, newlines_to_lf(value)));
        }
        Ok(Pairs(pairs))
    }
}

/// Native form submission serialises textarea line breaks as CR LF; the textarea API value
/// (and canonical storage) uses LF. Every submitted value therefore has CR LF and lone CR
/// normalised to LF before any validation, so length limits count the normalised text.
fn newlines_to_lf(value: String) -> String {
    if value.contains('\r') {
        value.replace("\r\n", "\n").replace('\r', "\n")
    } else {
        value
    }
}

impl Pairs {
    pub(super) fn parse(bytes: &[u8]) -> Result<Self, axum::http::StatusCode> {
        Self::parse_limited(bytes, FORM_PAIRS)
    }
    pub(super) fn parse_limited(
        bytes: &[u8],
        limit: usize,
    ) -> Result<Self, axum::http::StatusCode> {
        PairsSeed { limit }
            .deserialize(serde_urlencoded::Deserializer::new(form_urlencoded::parse(
                bytes,
            )))
            .map_err(|_| axum::http::StatusCode::BAD_REQUEST)
    }
    /// A form built without the URL parser (admission tests of large key sets).
    #[cfg(test)]
    pub(super) fn from_pairs(pairs: Vec<(String, String)>) -> Self {
        Self(pairs)
    }
    pub(super) fn csrf(&self) -> Result<&str, axum::http::StatusCode> {
        self.0
            .iter()
            .find(|(key, _)| key == "csrf")
            .map(|(_, value)| value.as_str())
            .ok_or(axum::http::StatusCode::FORBIDDEN)
    }
    pub(super) fn keys(&self) -> impl Iterator<Item = &str> {
        self.0.iter().map(|(key, _)| key.as_str())
    }
    /// Closed-key admission: exactly the expected key set, in O(pairs) set lookups.
    pub(super) fn admit<S: Borrow<str> + Hash + Eq>(
        self,
        keys: &HashSet<S>,
    ) -> Result<BTreeMap<String, String>, axum::http::StatusCode> {
        if self.0.len() != keys.len() || self.0.iter().any(|(key, _)| !keys.contains(key.as_str()))
        {
            return Err(axum::http::StatusCode::BAD_REQUEST);
        }
        Ok(self.0.into_iter().collect())
    }
}

#[cfg(test)]
mod tests {
    use super::{FORM_PAIRS, Pairs, SHEET_FORM_PAIRS};
    use std::collections::HashSet;

    fn encoded(count: usize) -> String {
        serde_urlencoded::to_string(
            (0..count)
                .map(|n| (format!("k{n}"), "0"))
                .collect::<Vec<_>>(),
        )
        .unwrap()
    }

    #[test]
    fn parse_limited_takes_its_limit_from_the_seed() {
        for limit in [3, FORM_PAIRS] {
            assert_eq!(
                Pairs::parse_limited(encoded(limit).as_bytes(), limit)
                    .unwrap()
                    .0
                    .len(),
                limit
            );
            assert!(Pairs::parse_limited(encoded(limit + 1).as_bytes(), limit).is_err());
        }
        assert!(Pairs::parse(encoded(516).as_bytes()).is_ok());
        assert!(Pairs::parse(encoded(517).as_bytes()).is_err());
        assert_eq!(SHEET_FORM_PAIRS, 65_536);
        assert!(Pairs::parse_limited(encoded(517).as_bytes(), SHEET_FORM_PAIRS).is_ok());
        // Duplicates after decoding and long keys stay refused whatever the limit.
        assert!(Pairs::parse_limited(b"a=1&%61=2", SHEET_FORM_PAIRS).is_err());
        let long = format!("{}=0", "k".repeat(71));
        assert!(Pairs::parse_limited(long.as_bytes(), SHEET_FORM_PAIRS).is_err());
        let edge = format!("{}=0", "k".repeat(70));
        assert!(Pairs::parse_limited(edge.as_bytes(), SHEET_FORM_PAIRS).is_ok());
    }

    #[test]
    fn submitted_values_have_crlf_and_cr_normalised_to_lf() {
        let pairs = Pairs::parse(b"a=%0D%0Aone%0D%0Atwo&b=%0Dthree&c=plain").unwrap();
        assert_eq!(
            pairs.0,
            [
                ("a".to_owned(), "\none\ntwo".to_owned()),
                ("b".to_owned(), "\nthree".to_owned()),
                ("c".to_owned(), "plain".to_owned()),
            ]
        );
    }

    /// Debug build, 65,536 pairs: set admission is linear; the old per-pair `contains` over
    /// a key slice was quadratic.
    #[test]
    fn admission_of_65536_pairs_is_linear() {
        let keys: Vec<String> = (0..SHEET_FORM_PAIRS).map(|n| format!("row-{n}")).collect();
        let pairs = Pairs::from_pairs(keys.iter().map(|k| (k.clone(), String::new())).collect());
        let expected: HashSet<String> = keys.iter().cloned().collect();
        let started = std::time::Instant::now();
        assert_eq!(pairs.admit(&expected).unwrap().len(), SHEET_FORM_PAIRS);
        let elapsed = started.elapsed();
        assert!(elapsed < std::time::Duration::from_secs(5), "{elapsed:?}");
    }
}
