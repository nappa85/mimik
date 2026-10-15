//! URL decoding remains `serde_urlencoded`'s responsibility; retain pairs until closed-key admission.
use serde::{
    Deserialize,
    de::{SeqAccess, Visitor},
};
use std::{
    collections::{BTreeMap, BTreeSet},
    fmt,
};

pub(super) struct Pairs(Vec<(String, String)>);

impl<'de> Deserialize<'de> for Pairs {
    fn deserialize<D: serde::Deserializer<'de>>(decoder: D) -> Result<Self, D::Error> {
        struct PairVisitor;
        impl<'de> Visitor<'de> for PairVisitor {
            type Value = Pairs;
            fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str("at most 516 unique URL form pairs")
            }
            fn visit_seq<A: SeqAccess<'de>>(self, mut sequence: A) -> Result<Pairs, A::Error> {
                let mut pairs = Vec::new();
                let mut seen = BTreeSet::new();
                while let Some((key, value)) = sequence.next_element::<(String, String)>()? {
                    if pairs.len() >= 516 || key.len() > 70 || !seen.insert(key.clone()) {
                        return Err(serde::de::Error::custom(
                            "duplicate key or form expansion limit",
                        ));
                    }
                    pairs.push((key, value));
                }
                Ok(Pairs(pairs))
            }
        }
        decoder.deserialize_seq(PairVisitor)
    }
}

impl Pairs {
    pub(super) fn parse(bytes: &[u8]) -> Result<Self, axum::http::StatusCode> {
        serde_urlencoded::from_bytes(bytes).map_err(|_| axum::http::StatusCode::BAD_REQUEST)
    }
    pub(super) fn csrf(&self) -> Result<&str, axum::http::StatusCode> {
        self.0
            .iter()
            .find(|(key, _)| key == "csrf")
            .map(|(_, value)| value.as_str())
            .ok_or(axum::http::StatusCode::FORBIDDEN)
    }
    pub(super) fn admit(
        self,
        keys: &[&str],
    ) -> Result<BTreeMap<String, String>, axum::http::StatusCode> {
        if self.0.len() != keys.len() || self.0.iter().any(|(key, _)| !keys.contains(&key.as_str()))
        {
            return Err(axum::http::StatusCode::BAD_REQUEST);
        }
        Ok(self.0.into_iter().collect())
    }
}
