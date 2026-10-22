//! Datetimes are presentation; canonical storage remains integer time.
pub(crate) async fn script_asset() -> impl axum::response::IntoResponse {
    (
        [(
            axum::http::header::CONTENT_TYPE,
            "text/javascript; charset=utf-8",
        )],
        include_str!("../assets/account-sharing.js"),
    )
}
pub(crate) struct DisplayTime {
    pub iso: String,
    pub readable: String,
}
pub(crate) fn datetime(timestamp: i64) -> DisplayTime {
    let fallback = || DisplayTime {
        iso: String::new(),
        readable: "Time unavailable".into(),
    };
    let Ok(time) = time::OffsetDateTime::from_unix_timestamp(timestamp) else {
        return fallback();
    };
    let Ok(iso) = time.format(&time::format_description::well_known::Rfc3339) else {
        return fallback();
    };
    let format = time::format_description::parse_borrowed::<2>(
        "[day] [month repr:long] [year], [hour]:[minute] UTC",
    )
    .expect("static datetime format");
    let Ok(readable) = time.format(&format) else {
        return fallback();
    };
    DisplayTime { iso, readable }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn datetime_is_readable_and_handles_invalid_range() {
        let date = datetime(100);
        assert_eq!(date.iso, "1970-01-01T00:01:40Z");
        assert_eq!(date.readable, "01 January 1970, 00:01 UTC");
        assert_eq!(datetime(i64::MAX).iso, "");
        assert_eq!(datetime(i64::MAX).readable, "Time unavailable");
    }
}
