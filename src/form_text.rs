//! Submitted text canonicalisation shared by every form decoder.
//!
//! A native form submission serialises textarea line breaks as CR LF, while the textarea API
//! value and canonical storage use LF. Normalising before validation keeps stored text LF-only
//! and makes character limits count what the user typed (`maxlength` counts LF as one).
use serde::{Deserialize, Deserializer};

/// CR LF and lone CR become LF; text without CR is returned unchanged (no allocation).
/// One pass and one allocation otherwise.
pub(crate) fn newlines_to_lf(value: String) -> String {
    if !value.contains('\r') {
        return value;
    }
    let mut out = String::with_capacity(value.len());
    let mut chars = value.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '\r' {
            chars.next_if_eq(&'\n');
            out.push('\n');
        } else {
            out.push(c);
        }
    }
    out
}

/// `deserialize_with` for a submitted text field.
///
/// These wrappers apply wherever the annotated struct is deserialised, not only from forms:
/// a JSON or other serde source of `CharacterInput`, `CreateRequest`, `SheetForm`, `Context`,
/// `UnitReview`, `PageReview` or the material forms would also have CR normalised. No such
/// non-form use exists today; stored records are read through their own row types.
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
        assert_eq!(newlines_to_lf("\r\r\n\n\r".into()), "\n\n\n\n");
        assert_eq!(newlines_to_lf("é\r\nü".into()), "é\nü");
    }
}
