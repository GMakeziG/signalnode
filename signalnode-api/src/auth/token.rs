use chrono::{DateTime, Duration, Utc};
use jsonwebtoken::{
    decode, encode, errors::ErrorKind, DecodingKey, EncodingKey, Header, Validation,
};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

const ACCESS_TOKEN_MINUTES: i64 = 15;
pub const REFRESH_TOKEN_DAYS: i64 = 7;
const KIND_ACCESS: &str = "access";
const KIND_REFRESH: &str = "refresh";

#[derive(Debug, Serialize, Deserialize)]
pub struct Claims {
    pub sub: String,
    pub exp: i64,
    pub kind: String,
    #[serde(default)]
    pub jti: Option<String>,
}

pub fn encode_access_token(
    user_id: &str,
    secret: &str,
) -> Result<String, jsonwebtoken::errors::Error> {
    let exp = (Utc::now() + Duration::minutes(ACCESS_TOKEN_MINUTES)).timestamp();
    let claims = Claims {
        sub: user_id.to_string(),
        exp,
        kind: KIND_ACCESS.to_string(),
        jti: None,
    };
    encode(
        &Header::default(),
        &claims,
        &EncodingKey::from_secret(secret.as_bytes()),
    )
}

/// Returns (jwt_string, jti, expires_at). Caller must persist jti in refresh_tokens.
pub fn encode_refresh_token(
    user_id: &str,
    secret: &str,
) -> Result<(String, Uuid, DateTime<Utc>), jsonwebtoken::errors::Error> {
    let jti = Uuid::new_v4();
    let expires_at = Utc::now() + Duration::days(REFRESH_TOKEN_DAYS);
    let claims = Claims {
        sub: user_id.to_string(),
        exp: expires_at.timestamp(),
        kind: KIND_REFRESH.to_string(),
        jti: Some(jti.to_string()),
    };
    let token = encode(
        &Header::default(),
        &claims,
        &EncodingKey::from_secret(secret.as_bytes()),
    )?;
    Ok((token, jti, expires_at))
}

pub fn decode_access_token(
    token: &str,
    secret: &str,
) -> Result<Claims, jsonwebtoken::errors::Error> {
    let data = decode::<Claims>(
        token,
        &DecodingKey::from_secret(secret.as_bytes()),
        &Validation::default(),
    )?;
    if data.claims.kind != KIND_ACCESS {
        return Err(ErrorKind::InvalidToken.into());
    }
    Ok(data.claims)
}

pub fn decode_refresh_token(
    token: &str,
    secret: &str,
) -> Result<Claims, jsonwebtoken::errors::Error> {
    let data = decode::<Claims>(
        token,
        &DecodingKey::from_secret(secret.as_bytes()),
        &Validation::default(),
    )?;
    if data.claims.kind != KIND_REFRESH {
        return Err(ErrorKind::InvalidToken.into());
    }
    if data.claims.jti.is_none() {
        return Err(ErrorKind::InvalidToken.into());
    }
    Ok(data.claims)
}

#[cfg(test)]
mod tests {
    use super::*;

    const SECRET: &str = "test-secret-at-least-32-chars-long!";

    #[test]
    fn encode_decode_access_roundtrip() {
        let token = encode_access_token("user-id-123", SECRET).unwrap();
        let claims = decode_access_token(&token, SECRET).unwrap();
        assert_eq!(claims.sub, "user-id-123");
        assert_eq!(claims.kind, "access");
    }

    #[test]
    fn encode_decode_refresh_roundtrip() {
        let (token, _, _) = encode_refresh_token("user-id-456", SECRET).unwrap();
        let claims = decode_refresh_token(&token, SECRET).unwrap();
        assert_eq!(claims.sub, "user-id-456");
        assert_eq!(claims.kind, "refresh");
    }

    #[test]
    fn wrong_secret_rejected() {
        let token = encode_access_token("user-id-123", SECRET).unwrap();
        assert!(decode_access_token(&token, "wrong-secret-padding-padding-ppp").is_err());
    }

    #[test]
    fn access_token_rejected_as_refresh() {
        let token = encode_access_token("uid", SECRET).unwrap();
        assert!(decode_refresh_token(&token, SECRET).is_err());
    }

    #[test]
    fn refresh_token_rejected_as_access() {
        let (token, _, _) = encode_refresh_token("uid", SECRET).unwrap();
        assert!(decode_access_token(&token, SECRET).is_err());
    }

    #[test]
    fn refresh_token_has_jti() {
        let (token, _jti, _exp) = encode_refresh_token("user-123", SECRET).unwrap();
        let claims = decode_refresh_token(&token, SECRET).unwrap();
        assert!(claims.jti.is_some());
        let jti_str = claims.jti.unwrap();
        assert!(!jti_str.is_empty());
        assert!(jti_str.parse::<uuid::Uuid>().is_ok(), "jti must be a valid UUID");
    }

    #[test]
    fn access_token_has_no_jti() {
        let token = encode_access_token("user-123", SECRET).unwrap();
        let claims = decode_access_token(&token, SECRET).unwrap();
        assert!(claims.jti.is_none());
    }

    #[test]
    fn refresh_token_jtis_are_unique() {
        let (_, jti1, _) = encode_refresh_token("user-123", SECRET).unwrap();
        let (_, jti2, _) = encode_refresh_token("user-123", SECRET).unwrap();
        assert_ne!(jti1, jti2);
    }
}
