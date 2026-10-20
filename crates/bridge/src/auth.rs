//! App-layer bearer auth, enforced even behind the tunnel. See docs/wiki/auth-and-security.md

use axum::http::header::AUTHORIZATION;
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use sha2::{Digest, Sha256};
use subtle::ConstantTimeEq;

#[derive(Clone)]
pub struct Auth {
    /// SHA-256 of the configured key; `None` ⇒ auth disabled.
    expected: Option<[u8; 32]>,
}

impl Auth {
    pub fn new(api_key: Option<&str>) -> Self {
        Self {
            expected: api_key.map(|k| sha256(k.as_bytes())),
        }
    }

    pub const fn enabled(&self) -> bool {
        self.expected.is_some()
    }

    /// Returns `None` if authorized (or auth disabled); `Some(401 response)` to reject.
    pub fn check(&self, headers: &HeaderMap) -> Option<Response> {
        let expected = self.expected.as_ref()?;
        let presented = headers
            .get(AUTHORIZATION)
            .and_then(|v| v.to_str().ok())
            .map_or("", strip_bearer);
        let got = sha256(presented.as_bytes());
        // Constant-time: a byte-wise compare would leak how much of the key is correct.
        if !presented.is_empty() && bool::from(got.as_slice().ct_eq(expected.as_slice())) {
            None
        } else {
            Some((StatusCode::UNAUTHORIZED, "unauthorized").into_response())
        }
    }
}

fn strip_bearer(s: &str) -> &str {
    s.strip_prefix("Bearer ")
        .or_else(|| s.strip_prefix("bearer "))
        .unwrap_or(s)
        .trim()
}

fn sha256(bytes: &[u8]) -> [u8; 32] {
    let mut hasher = Sha256::new();
    hasher.update(bytes);
    let digest = hasher.finalize();
    let mut out = [0u8; 32];
    out.copy_from_slice(&digest);
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::http::HeaderValue;

    fn hdr(authorization: Option<&'static str>) -> HeaderMap {
        let mut h = HeaderMap::new();
        if let Some(v) = authorization {
            h.insert(AUTHORIZATION, HeaderValue::from_static(v));
        }
        h
    }

    #[test]
    fn disabled_auth_authorizes_everything() {
        let a = Auth::new(None);
        assert!(!a.enabled());
        assert!(a.check(&hdr(None)).is_none());
        assert!(a.check(&hdr(Some("Bearer anything"))).is_none());
    }

    #[test]
    fn correct_key_authorizes() {
        let a = Auth::new(Some("s3cret-key"));
        assert!(a.enabled());
        assert!(a.check(&hdr(Some("Bearer s3cret-key"))).is_none());
        assert!(a.check(&hdr(Some("bearer s3cret-key"))).is_none());
        assert!(a.check(&hdr(Some("s3cret-key"))).is_none());
    }

    #[test]
    fn wrong_or_missing_key_rejected() {
        let a = Auth::new(Some("s3cret-key"));
        assert!(a.check(&hdr(None)).is_some());
        assert!(a.check(&hdr(Some("Bearer wrong"))).is_some());
        assert!(a.check(&hdr(Some("Bearer "))).is_some());
        assert!(a.check(&hdr(Some("Bearer s3cret-ke"))).is_some());
    }

    #[test]
    fn empty_configured_digest_cannot_authorize_a_missing_or_empty_header() {
        let auth = Auth::new(Some(""));
        for value in [None, Some(""), Some("Bearer "), Some("bearer   ")] {
            assert!(auth.check(&hdr(value)).is_some());
        }
    }

    #[test]
    fn strip_bearer_variants() {
        assert_eq!(strip_bearer("Bearer abc"), "abc");
        assert_eq!(strip_bearer("bearer abc"), "abc");
        assert_eq!(strip_bearer("abc"), "abc");
        assert_eq!(strip_bearer("Bearer  abc "), "abc");
    }
}
