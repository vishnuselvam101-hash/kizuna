//! The language of server-rendered text (letters, reports, generated titles) for one request.
//!
//! Order: an explicit `?lang=ja|en` (handled by the endpoint), then the `Accept-Language` header (the web app sends
//! its UI language), then the endpoint's previous default (Japanese for letters, English for reports), so API
//! clients that send neither see no change.

use axum::extract::FromRequestParts;
use axum::http::request::Parts;
pub use kizuna_messages::{Lang, Msg};

use crate::app::AppState;
use crate::error::ApiError;

/// Request language from `Accept-Language` (`None` when absent or unsupported).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ReqLang(pub Option<Lang>);

impl ReqLang {
    /// `explicit` (a `?lang=` value) wins over the header, the header over `default`; an unknown explicit value
    /// is `invalid_lang`.
    pub fn resolve(self, explicit: Option<&str>, default: Lang) -> Result<Lang, ApiError> {
        match explicit {
            None => Ok(self.0.unwrap_or(default)),
            Some(s) => Lang::parse(s).ok_or(ApiError::BadRequest("invalid_lang")),
        }
    }
}

pub fn from_headers(h: &axum::http::HeaderMap) -> Option<Lang> {
    h.get(axum::http::header::ACCEPT_LANGUAGE)
        .and_then(|v| v.to_str().ok())
        .and_then(Lang::from_accept_language)
}

impl FromRequestParts<AppState> for ReqLang {
    type Rejection = ApiError;
    async fn from_request_parts(parts: &mut Parts, _st: &AppState) -> Result<Self, ApiError> {
        Ok(ReqLang(from_headers(&parts.headers)))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::http::{HeaderMap, HeaderValue, header::ACCEPT_LANGUAGE};

    #[test]
    fn explicit_then_header_then_default() {
        let mut h = HeaderMap::new();
        assert_eq!(from_headers(&h), None);
        assert_eq!(ReqLang(None).resolve(None, Lang::En).ok(), Some(Lang::En));
        h.insert(ACCEPT_LANGUAGE, HeaderValue::from_static("ja-JP,en;q=0.9"));
        let r = ReqLang(from_headers(&h));
        assert_eq!(r.resolve(None, Lang::En).ok(), Some(Lang::Ja));
        assert_eq!(r.resolve(Some("en"), Lang::Ja).ok(), Some(Lang::En));
        assert!(r.resolve(Some("fr"), Lang::Ja).is_err());
    }
}
