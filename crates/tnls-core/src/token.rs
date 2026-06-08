use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use base64::Engine;
use hmac::{Hmac, Mac};
use serde::{Deserialize, Serialize};
use sha2::Sha256;
use subtle::ConstantTimeEq;

type HmacSha256 = Hmac<Sha256>;

/// Capability claims signed into the token.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Claims {
    pub tunnel_id: String,
    pub scope: Vec<String>,
    pub exp: u64, // unix seconds
}

#[derive(Debug, PartialEq, Eq, thiserror::Error)]
pub enum TokenError {
    #[error("malformed token")]
    Malformed,
    #[error("bad signature")]
    BadSignature,
    #[error("expired")]
    Expired,
}

/// `b64url(payload_json) + "." + b64url(HMAC_SHA256(secret, payload_json))`.
pub fn mint(secret: &[u8], claims: &Claims) -> String {
    let payload = serde_json::to_vec(claims).expect("claims serialize");
    let mut mac = HmacSha256::new_from_slice(secret).expect("hmac accepts any key length");
    mac.update(&payload);
    let sig = mac.finalize().into_bytes();
    format!(
        "{}.{}",
        URL_SAFE_NO_PAD.encode(&payload),
        URL_SAFE_NO_PAD.encode(sig)
    )
}

/// Verify signature (constant-time) then check `exp > now`. Signature is checked
/// before parsing claims so a forged token never reaches `serde_json`.
pub fn verify(secret: &[u8], token: &str, now: u64) -> Result<Claims, TokenError> {
    let (p_b64, s_b64) = token.split_once('.').ok_or(TokenError::Malformed)?;
    let payload = URL_SAFE_NO_PAD
        .decode(p_b64)
        .map_err(|_| TokenError::Malformed)?;
    let sig = URL_SAFE_NO_PAD
        .decode(s_b64)
        .map_err(|_| TokenError::Malformed)?;

    let mut mac = HmacSha256::new_from_slice(secret).expect("hmac accepts any key length");
    mac.update(&payload);
    let expected = mac.finalize().into_bytes();
    if expected.as_slice().ct_eq(&sig).unwrap_u8() != 1 {
        return Err(TokenError::BadSignature);
    }

    let claims: Claims = serde_json::from_slice(&payload).map_err(|_| TokenError::Malformed)?;
    if claims.exp <= now {
        return Err(TokenError::Expired);
    }
    Ok(claims)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn claims(exp: u64) -> Claims {
        Claims {
            tunnel_id: "abc123".into(),
            scope: vec!["read".into()],
            exp,
        }
    }

    #[test]
    fn roundtrip_returns_same_claims() {
        let secret = b"super-secret-key";
        let c = claims(10_000);
        let token = mint(secret, &c);
        assert_eq!(verify(secret, &token, 9_000).unwrap(), c);
    }

    #[test]
    fn expired_token_rejected() {
        let secret = b"super-secret-key";
        let token = mint(secret, &claims(10_000));
        assert_eq!(verify(secret, &token, 10_000), Err(TokenError::Expired));
        assert_eq!(verify(secret, &token, 10_001), Err(TokenError::Expired));
    }

    #[test]
    fn tampered_payload_rejected() {
        let secret = b"super-secret-key";
        let token = mint(secret, &claims(10_000));
        let (_p, sig) = token.split_once('.').unwrap();
        // Swap in a different payload, keep the old signature.
        let forged_payload = URL_SAFE_NO_PAD.encode(
            serde_json::to_vec(&Claims {
                tunnel_id: "abc123".into(),
                scope: vec!["read".into(), "shell".into()], // privilege escalation attempt
                exp: 10_000,
            })
            .unwrap(),
        );
        let forged = format!("{forged_payload}.{sig}");
        assert_eq!(
            verify(secret, &forged, 9_000),
            Err(TokenError::BadSignature)
        );
    }

    #[test]
    fn wrong_secret_rejected() {
        let token = mint(b"key-a", &claims(10_000));
        assert_eq!(
            verify(b"key-b", &token, 9_000),
            Err(TokenError::BadSignature)
        );
    }

    #[test]
    fn garbage_is_malformed() {
        assert_eq!(verify(b"k", "not-a-token", 0), Err(TokenError::Malformed));
    }
}
