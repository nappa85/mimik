//! Submitted text canonicalisation shared by every form decoder.
//!
//! A native form submission serialises textarea line breaks as CR LF, while the textarea API
//! value and canonical storage use LF. Normalising before validation keeps stored text LF-only
//! and makes character limits count what the user typed (`maxlength` counts LF as one).
use serde::{Deserialize, Deserializer};

/// CR LF and lone CR become LF; text without CR is returned unchanged.
pub(crate) fn newlines_to_lf(value: String) -> String {
    if value.contains('\r') {
        value.replace("\r\n", "\n").replace('\r', "\n")
    } else {
        value
    }
}

/// `deserialize_with` for a submitted text field.
pub(crate) fn lf<'de, D: Deserializer<'de>>(decoder: D) -> Result<String, D::Error> {
    String::deserialize(decoder).map(newlines_to_lf)
}

/// `deserialize_with` for an optional submitted text field.
pub(crate) fn lf_option<'de, D: Deserializer<'de>>(decoder: D) -> Result<Option<String>, D::Error> {
    Option::<String>::deserialize(decoder).map(|value| value.map(newlines_to_lf))
}

#[cfg(test)]
mod tests {
    use super::newlines_to_lf;

    #[test]
    fn crlf_and_cr_become_lf() {
        assert_eq!(newlines_to_lf("a\r\nb\rc\nd".into()), "a\nb\nc\nd");
        assert_eq!(newlines_to_lf("\r\n\r\n".into()), "\n\n");
        assert_eq!(newlines_to_lf("plain".into()), "plain");
    }
}
