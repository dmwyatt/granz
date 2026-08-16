//! Minimal JWT payload decoding for account identity.
//!
//! The Granola access token is a JWT whose payload carries a stable WorkOS
//! user id in the `sub` claim. Decoding is a local base64 + JSON parse used
//! as a label for account binding, not a security decision, so the signature
//! is deliberately not verified.

use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use log::debug;

/// Extract the `sub` claim from a JWT without verifying its signature.
///
/// Returns None when the token is not decodable as a JWT (wrong segment
/// count, invalid base64url, invalid JSON) or when the payload has no string
/// `sub` claim. Arbitrary `--token` values are expected to land here; the
/// API rejects them on its own, so None means "no identity available", not
/// an error.
pub fn decode_sub(token: &str) -> Option<String> {
    let mut segments = token.split('.');
    let (Some(_header), Some(payload), Some(_signature), None) = (
        segments.next(),
        segments.next(),
        segments.next(),
        segments.next(),
    ) else {
        debug!("token is not a three-segment JWT");
        return None;
    };

    let bytes = match URL_SAFE_NO_PAD.decode(payload) {
        Ok(bytes) => bytes,
        Err(e) => {
            debug!("JWT payload is not valid base64url: {}", e);
            return None;
        }
    };

    let claims: serde_json::Value = match serde_json::from_slice(&bytes) {
        Ok(claims) => claims,
        Err(e) => {
            debug!("JWT payload is not valid JSON: {}", e);
            return None;
        }
    };

    match claims.get("sub").and_then(|sub| sub.as_str()) {
        Some(sub) => Some(sub.to_string()),
        None => {
            debug!("JWT payload has no string `sub` claim");
            None
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Build an unsigned JWT with the given payload JSON.
    fn make_jwt(payload: &serde_json::Value) -> String {
        let header = URL_SAFE_NO_PAD.encode(br#"{"alg":"RS256","typ":"JWT"}"#);
        let payload = URL_SAFE_NO_PAD.encode(payload.to_string());
        format!("{}.{}.fake-signature", header, payload)
    }

    #[test]
    fn decodes_sub_from_valid_jwt() {
        let token = make_jwt(&serde_json::json!({
            "sub": "user_01K5EXAMPLE",
            "workos_id": "user_01K5EXAMPLE",
            "iss": "https://api.workos.com",
        }));
        assert_eq!(decode_sub(&token), Some("user_01K5EXAMPLE".to_string()));
    }

    #[test]
    fn returns_none_for_non_jwt_garbage() {
        assert_eq!(decode_sub("not-a-jwt"), None);
        assert_eq!(decode_sub(""), None);
        assert_eq!(decode_sub("only.two"), None);
        assert_eq!(decode_sub("a.b.c.d"), None);
        // Three segments but not base64/JSON
        assert_eq!(decode_sub("!!.@@.##"), None);
    }

    #[test]
    fn returns_none_for_jwt_without_sub() {
        let token = make_jwt(&serde_json::json!({"iss": "https://api.workos.com"}));
        assert_eq!(decode_sub(&token), None);
    }

    #[test]
    fn returns_none_for_non_string_sub() {
        let token = make_jwt(&serde_json::json!({"sub": 42}));
        assert_eq!(decode_sub(&token), None);
    }
}
