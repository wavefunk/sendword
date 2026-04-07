//! Per-hook webhook authentication.
//!
//! Verifies incoming trigger requests against the hook's auth config.
//! Supports three modes: none (public), bearer token, and HMAC signature.

use axum::http::HeaderMap;
use ring::hmac;

use crate::config::{HmacAlgorithm, HookAuthConfig};

/// Result of a webhook auth check.
#[derive(Debug)]
pub enum AuthResult {
    /// Auth succeeded or was not required.
    Ok,
    /// Auth failed with a reason for logging.
    Denied(String),
}

/// Resolve a config value that may be an env var reference.
///
/// If the value is `${VAR_NAME}`, looks up `VAR_NAME` in the environment.
/// Returns `None` if the env var is not set.
/// If the value is a plain string (no `${}` wrapper), returns it as-is.
fn resolve_env_ref(value: &str) -> Option<String> {
    if let Some(var_name) = value.strip_prefix("${").and_then(|s| s.strip_suffix('}')) {
        std::env::var(var_name).ok()
    } else {
        Some(value.to_owned())
    }
}

/// Constant-time byte slice equality comparison.
///
/// Returns true only if both slices have equal length and content.
/// Uses XOR accumulation to avoid early-exit timing differences.
fn constant_time_eq(a: &[u8], b: &[u8]) -> bool {
    if a.len() != b.len() {
        return false;
    }
    let mut acc: u8 = 0;
    for (x, y) in a.iter().zip(b.iter()) {
        acc |= x ^ y;
    }
    acc == 0
}

/// Encode bytes as lowercase hex string.
#[cfg(test)]
fn hex_encode(bytes: &[u8]) -> String {
    let mut s = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        use std::fmt::Write;
        let _ = write!(s, "{byte:02x}");
    }
    s
}

/// Decode a hex string into bytes. Returns `None` on invalid hex.
fn hex_decode(hex: &str) -> Option<Vec<u8>> {
    if hex.len() % 2 != 0 {
        return None;
    }
    let mut bytes = Vec::with_capacity(hex.len() / 2);
    for chunk in hex.as_bytes().chunks(2) {
        let hi = hex_digit(chunk[0])?;
        let lo = hex_digit(chunk[1])?;
        bytes.push((hi << 4) | lo);
    }
    Some(bytes)
}

fn hex_digit(b: u8) -> Option<u8> {
    match b {
        b'0'..=b'9' => Some(b - b'0'),
        b'a'..=b'f' => Some(b - b'a' + 10),
        b'A'..=b'F' => Some(b - b'A' + 10),
        _ => None,
    }
}

/// Verify a webhook request against the hook's auth config.
///
/// Returns `AuthResult::Ok` if auth succeeds or is not configured.
/// Returns `AuthResult::Denied` with a reason string on failure.
pub fn verify(
    auth: Option<&HookAuthConfig>,
    headers: &HeaderMap,
    body: &[u8],
) -> AuthResult {
    let auth = match auth {
        None => return AuthResult::Ok,
        Some(auth) => auth,
    };

    match auth {
        HookAuthConfig::None => AuthResult::Ok,
        HookAuthConfig::Bearer { token } => verify_bearer(token, headers),
        HookAuthConfig::Hmac { header, algorithm, secret } => {
            verify_hmac(header, *algorithm, secret, headers, body)
        }
    }
}

fn verify_bearer(token_config: &str, headers: &HeaderMap) -> AuthResult {
    let expected = match resolve_env_ref(token_config) {
        Some(t) => t,
        None => {
            tracing::warn!(
                "webhook auth: bearer token env var not set: {token_config}"
            );
            return AuthResult::Denied("server misconfiguration: token env var not set".into());
        }
    };

    let header_value = match headers.get(axum::http::header::AUTHORIZATION) {
        Some(v) => v,
        None => return AuthResult::Denied("missing Authorization header".into()),
    };

    let header_str = match header_value.to_str() {
        Ok(s) => s,
        Err(_) => return AuthResult::Denied("invalid Authorization header encoding".into()),
    };

    let provided = match header_str.strip_prefix("Bearer ") {
        Some(t) => t,
        None => return AuthResult::Denied("Authorization header is not Bearer scheme".into()),
    };

    // Constant-time comparison to prevent timing attacks
    if constant_time_eq(expected.as_bytes(), provided.as_bytes()) {
        AuthResult::Ok
    } else {
        AuthResult::Denied("bearer token mismatch".into())
    }
}

fn verify_hmac(
    header_name: &str,
    algorithm: HmacAlgorithm,
    secret_config: &str,
    headers: &HeaderMap,
    body: &[u8],
) -> AuthResult {
    let secret = match resolve_env_ref(secret_config) {
        Some(s) => s,
        None => {
            tracing::warn!(
                "webhook auth: HMAC secret env var not set: {secret_config}"
            );
            return AuthResult::Denied(
                "server misconfiguration: HMAC secret env var not set".into(),
            );
        }
    };

    let header_value = match headers.get(header_name) {
        Some(v) => v,
        None => {
            return AuthResult::Denied(
                format!("missing signature header: {header_name}"),
            );
        }
    };

    let header_str = match header_value.to_str() {
        Ok(s) => s,
        Err(_) => return AuthResult::Denied("invalid signature header encoding".into()),
    };

    // Strip algorithm prefix if present (e.g., "sha256=<hex>")
    let hex_sig = match algorithm {
        HmacAlgorithm::Sha256 => header_str
            .strip_prefix("sha256=")
            .unwrap_or(header_str),
    };

    let sig_bytes = match hex_decode(hex_sig) {
        Some(b) => b,
        None => return AuthResult::Denied("invalid hex in signature header".into()),
    };

    let ring_algorithm = match algorithm {
        HmacAlgorithm::Sha256 => hmac::HMAC_SHA256,
    };

    let key = hmac::Key::new(ring_algorithm, secret.as_bytes());
    match hmac::verify(&key, body, &sig_bytes) {
        Ok(()) => AuthResult::Ok,
        Err(_) => AuthResult::Denied("HMAC signature mismatch".into()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::http::{HeaderMap, HeaderName, HeaderValue};

    #[test]
    fn no_auth_config_allows_request() {
        let result = verify(None, &HeaderMap::new(), &[]);
        assert!(matches!(result, AuthResult::Ok));
    }

    #[test]
    fn none_mode_allows_request() {
        let auth = HookAuthConfig::None;
        let result = verify(Some(&auth), &HeaderMap::new(), &[]);
        assert!(matches!(result, AuthResult::Ok));
    }

    #[test]
    fn bearer_valid_token_allows_request() {
        let auth = HookAuthConfig::Bearer {
            token: "my-secret-token".to_owned(),
        };
        let mut headers = HeaderMap::new();
        headers.insert(
            "authorization",
            HeaderValue::from_static("Bearer my-secret-token"),
        );
        let result = verify(Some(&auth), &headers, &[]);
        assert!(matches!(result, AuthResult::Ok));
    }

    #[test]
    fn bearer_wrong_token_denies() {
        let auth = HookAuthConfig::Bearer {
            token: "correct-token".to_owned(),
        };
        let mut headers = HeaderMap::new();
        headers.insert(
            "authorization",
            HeaderValue::from_static("Bearer wrong-token"),
        );
        let result = verify(Some(&auth), &headers, &[]);
        assert!(matches!(result, AuthResult::Denied(_)));
    }

    #[test]
    fn bearer_missing_header_denies() {
        let auth = HookAuthConfig::Bearer {
            token: "my-token".to_owned(),
        };
        let result = verify(Some(&auth), &HeaderMap::new(), &[]);
        assert!(matches!(result, AuthResult::Denied(_)));
    }

    #[test]
    fn bearer_non_bearer_scheme_denies() {
        let auth = HookAuthConfig::Bearer {
            token: "my-token".to_owned(),
        };
        let mut headers = HeaderMap::new();
        headers.insert(
            "authorization",
            HeaderValue::from_static("Basic dXNlcjpwYXNz"),
        );
        let result = verify(Some(&auth), &headers, &[]);
        assert!(matches!(result, AuthResult::Denied(_)));
    }

    #[test]
    fn bearer_env_var_resolution() {
        // SAFETY: test-only, single-threaded unit test context
        unsafe { std::env::set_var("TEST_WEBHOOK_TOKEN_1", "resolved-token") };
        let auth = HookAuthConfig::Bearer {
            token: "${TEST_WEBHOOK_TOKEN_1}".to_owned(),
        };
        let mut headers = HeaderMap::new();
        headers.insert(
            "authorization",
            HeaderValue::from_static("Bearer resolved-token"),
        );
        let result = verify(Some(&auth), &headers, &[]);
        // SAFETY: test-only cleanup
        unsafe { std::env::remove_var("TEST_WEBHOOK_TOKEN_1") };
        assert!(matches!(result, AuthResult::Ok));
    }

    #[test]
    fn bearer_missing_env_var_denies() {
        // SAFETY: test-only, ensuring var is absent
        unsafe { std::env::remove_var("NONEXISTENT_TOKEN_VAR_XYZ") };
        let auth = HookAuthConfig::Bearer {
            token: "${NONEXISTENT_TOKEN_VAR_XYZ}".to_owned(),
        };
        let mut headers = HeaderMap::new();
        headers.insert(
            "authorization",
            HeaderValue::from_static("Bearer anything"),
        );
        let result = verify(Some(&auth), &headers, &[]);
        assert!(matches!(result, AuthResult::Denied(_)));
    }

    #[test]
    fn hmac_valid_signature_allows() {
        let secret = "my-hmac-secret";
        let body = b"hello world";

        // Compute expected signature
        let key = hmac::Key::new(hmac::HMAC_SHA256, secret.as_bytes());
        let tag = hmac::sign(&key, body);
        let hex_sig = hex_encode(tag.as_ref());

        let auth = HookAuthConfig::Hmac {
            header: "X-Hub-Signature-256".to_owned(),
            algorithm: HmacAlgorithm::Sha256,
            secret: secret.to_owned(),
        };
        let mut headers = HeaderMap::new();
        headers.insert(
            HeaderName::from_static("x-hub-signature-256"),
            HeaderValue::from_str(&format!("sha256={hex_sig}")).unwrap(),
        );
        let result = verify(Some(&auth), &headers, body);
        assert!(matches!(result, AuthResult::Ok));
    }

    #[test]
    fn hmac_valid_signature_without_prefix_allows() {
        let secret = "my-hmac-secret";
        let body = b"test payload";

        let key = hmac::Key::new(hmac::HMAC_SHA256, secret.as_bytes());
        let tag = hmac::sign(&key, body);
        let hex_sig = hex_encode(tag.as_ref());

        let auth = HookAuthConfig::Hmac {
            header: "X-Signature".to_owned(),
            algorithm: HmacAlgorithm::Sha256,
            secret: secret.to_owned(),
        };
        let mut headers = HeaderMap::new();
        headers.insert(
            HeaderName::from_static("x-signature"),
            // No "sha256=" prefix -- just raw hex
            HeaderValue::from_str(&hex_sig).unwrap(),
        );
        let result = verify(Some(&auth), &headers, body);
        assert!(matches!(result, AuthResult::Ok));
    }

    #[test]
    fn hmac_wrong_signature_denies() {
        let auth = HookAuthConfig::Hmac {
            header: "X-Hub-Signature-256".to_owned(),
            algorithm: HmacAlgorithm::Sha256,
            secret: "correct-secret".to_owned(),
        };
        let mut headers = HeaderMap::new();
        headers.insert(
            HeaderName::from_static("x-hub-signature-256"),
            HeaderValue::from_static("sha256=0000000000000000000000000000000000000000000000000000000000000000"),
        );
        let result = verify(Some(&auth), &headers, b"body");
        assert!(matches!(result, AuthResult::Denied(_)));
    }

    #[test]
    fn hmac_missing_header_denies() {
        let auth = HookAuthConfig::Hmac {
            header: "X-Hub-Signature-256".to_owned(),
            algorithm: HmacAlgorithm::Sha256,
            secret: "secret".to_owned(),
        };
        let result = verify(Some(&auth), &HeaderMap::new(), b"body");
        assert!(matches!(result, AuthResult::Denied(_)));
    }

    #[test]
    fn hmac_invalid_hex_denies() {
        let auth = HookAuthConfig::Hmac {
            header: "X-Sig".to_owned(),
            algorithm: HmacAlgorithm::Sha256,
            secret: "secret".to_owned(),
        };
        let mut headers = HeaderMap::new();
        headers.insert(
            HeaderName::from_static("x-sig"),
            HeaderValue::from_static("sha256=not-valid-hex!!"),
        );
        let result = verify(Some(&auth), &headers, b"body");
        assert!(matches!(result, AuthResult::Denied(_)));
    }

    #[test]
    fn hmac_env_var_secret_resolution() {
        let secret = "env-resolved-secret";
        // SAFETY: test-only, single-threaded unit test context
        unsafe { std::env::set_var("TEST_HMAC_SECRET_1", secret) };

        let body = b"payload";
        let key = hmac::Key::new(hmac::HMAC_SHA256, secret.as_bytes());
        let tag = hmac::sign(&key, body);
        let hex_sig = hex_encode(tag.as_ref());

        let auth = HookAuthConfig::Hmac {
            header: "X-Sig".to_owned(),
            algorithm: HmacAlgorithm::Sha256,
            secret: "${TEST_HMAC_SECRET_1}".to_owned(),
        };
        let mut headers = HeaderMap::new();
        headers.insert(
            HeaderName::from_static("x-sig"),
            HeaderValue::from_str(&format!("sha256={hex_sig}")).unwrap(),
        );
        let result = verify(Some(&auth), &headers, body);
        // SAFETY: test-only cleanup
        unsafe { std::env::remove_var("TEST_HMAC_SECRET_1") };
        assert!(matches!(result, AuthResult::Ok));
    }

    #[test]
    fn hmac_missing_env_var_secret_denies() {
        // SAFETY: test-only, ensuring var is absent
        unsafe { std::env::remove_var("NONEXISTENT_HMAC_VAR_XYZ") };
        let auth = HookAuthConfig::Hmac {
            header: "X-Sig".to_owned(),
            algorithm: HmacAlgorithm::Sha256,
            secret: "${NONEXISTENT_HMAC_VAR_XYZ}".to_owned(),
        };
        let mut headers = HeaderMap::new();
        headers.insert(
            HeaderName::from_static("x-sig"),
            HeaderValue::from_static("sha256=aabbccdd"),
        );
        let result = verify(Some(&auth), &headers, b"body");
        assert!(matches!(result, AuthResult::Denied(_)));
    }

    #[test]
    fn resolve_env_ref_with_plain_string() {
        let result = resolve_env_ref("plain-value");
        assert_eq!(result, Some("plain-value".to_owned()));
    }

    #[test]
    fn resolve_env_ref_with_env_var() {
        // SAFETY: test-only, single-threaded unit test context
        unsafe { std::env::set_var("TEST_RESOLVE_VAR_1", "found-it") };
        let result = resolve_env_ref("${TEST_RESOLVE_VAR_1}");
        // SAFETY: test-only cleanup
        unsafe { std::env::remove_var("TEST_RESOLVE_VAR_1") };
        assert_eq!(result, Some("found-it".to_owned()));
    }

    #[test]
    fn resolve_env_ref_with_missing_env_var() {
        // SAFETY: test-only, ensuring var is absent
        unsafe { std::env::remove_var("MISSING_VAR_RESOLVE_XYZ") };
        let result = resolve_env_ref("${MISSING_VAR_RESOLVE_XYZ}");
        assert_eq!(result, None);
    }

    #[test]
    fn constant_time_eq_equal_slices() {
        assert!(constant_time_eq(b"hello", b"hello"));
    }

    #[test]
    fn constant_time_eq_different_slices() {
        assert!(!constant_time_eq(b"hello", b"world"));
    }

    #[test]
    fn constant_time_eq_different_lengths() {
        assert!(!constant_time_eq(b"short", b"longer"));
    }

    #[test]
    fn constant_time_eq_empty_slices() {
        assert!(constant_time_eq(b"", b""));
    }

    #[test]
    fn hex_decode_valid_hex() {
        assert_eq!(hex_decode("aabb"), Some(vec![0xaa, 0xbb]));
    }

    #[test]
    fn hex_decode_odd_length_returns_none() {
        assert_eq!(hex_decode("abc"), None);
    }

    #[test]
    fn hex_decode_invalid_chars_returns_none() {
        assert_eq!(hex_decode("zzzz"), None);
    }

    #[test]
    fn hex_decode_empty_string_returns_empty_vec() {
        assert_eq!(hex_decode(""), Some(vec![]));
    }

    #[test]
    fn hex_decode_uppercase_is_accepted() {
        assert_eq!(hex_decode("AABB"), Some(vec![0xaa, 0xbb]));
    }

    #[test]
    fn hmac_empty_body_verifies_correctly() {
        let secret = "empty-body-secret";
        let body = b"";

        let key = hmac::Key::new(hmac::HMAC_SHA256, secret.as_bytes());
        let tag = hmac::sign(&key, body);
        let hex_sig = hex_encode(tag.as_ref());

        let auth = HookAuthConfig::Hmac {
            header: "X-Sig".to_owned(),
            algorithm: HmacAlgorithm::Sha256,
            secret: secret.to_owned(),
        };
        let mut headers = HeaderMap::new();
        headers.insert(
            HeaderName::from_static("x-sig"),
            HeaderValue::from_str(&format!("sha256={hex_sig}")).unwrap(),
        );
        let result = verify(Some(&auth), &headers, body);
        assert!(matches!(result, AuthResult::Ok));
    }
}
