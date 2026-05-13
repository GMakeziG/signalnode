use chrono::{Duration, Utc};
use jsonwebtoken::{decode, encode, errors::ErrorKind, DecodingKey, EncodingKey, Header, Validation};
use serde::{Deserialize, Serialize};

const ACCESS_TOKEN_MINUTES: i64 = 15;
const REFRESH_TOKEN_DAYS: i64 = 7;
const KIND_ACCESS: &str = "access";
const KIND_REFRESH: &str = "refresh";

#[derive(Debug, Serialize, Deserialize)]
pub struct Claims {
    pub sub: String,
    pub exp: i64,
    pub kind: String,
}

pub fn encode_access_token(user_id: &str, secret: &str) -> Result<String, jsonwebtoken::errors::Error> {
    let exp = (Utc::now() + Duration::minutes(ACCESS_TOKEN_MINUTES)).timestamp();
    let claims = Claims { sub: user_id.to_string(), exp, kind: KIND_ACCESS.to_string() };
    encode(&Header::default(), &claims, &EncodingKey::from_secret(secret.as_bytes()))
}

pub fn encode_refresh_token(user_id: &str, secret: &str) -> Result<String, jsonwebtoken::errors::Error> {
    let exp = (Utc::now() + Duration::days(REFRESH_TOKEN_DAYS)).timestamp();
    let claims = Claims { sub: user_id.to_string(), exp, kind: KIND_REFRESH.to_string() };
    encode(&Header::default(), &claims, &EncodingKey::from_secret(secret.as_bytes()))
}

pub fn decode_access_token(token: &str, secret: &str) -> Result<Claims, jsonwebtoken::errors::Error> {
    let data = decode::<Claims>(token, &DecodingKey::from_secret(secret.as_bytes()), &Validation::default())?;
    if data.claims.kind != KIND_ACCESS {
        return Err(ErrorKind::InvalidToken.into());
    }
    Ok(data.claims)
}

pub fn decode_refresh_token(token: &str, secret: &str) -> Result<Claims, jsonwebtoken::errors::Error> {
    let data = decode::<Claims>(token, &DecodingKey::from_secret(secret.as_bytes()), &Validation::default())?;
    if data.claims.kind != KIND_REFRESH {
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
        let token = encode_refresh_token("user-id-456", SECRET).unwrap();
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
        let token = encode_refresh_token("uid", SECRET).unwrap();
        assert!(decode_access_token(&token, SECRET).is_err());
    }
}
