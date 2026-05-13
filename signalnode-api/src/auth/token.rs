use chrono::{Duration, Utc};
use jsonwebtoken::{decode, encode, DecodingKey, EncodingKey, Header, Validation};
use serde::{Deserialize, Serialize};

const ACCESS_TOKEN_MINUTES: i64 = 15;
const REFRESH_TOKEN_DAYS: i64 = 7;

#[derive(Debug, Serialize, Deserialize)]
pub struct Claims {
    pub sub: String,
    pub exp: i64,
}

pub fn encode_access_token(user_id: &str, secret: &str) -> Result<String, jsonwebtoken::errors::Error> {
    let exp = (Utc::now() + Duration::minutes(ACCESS_TOKEN_MINUTES)).timestamp();
    let claims = Claims { sub: user_id.to_string(), exp };
    encode(&Header::default(), &claims, &EncodingKey::from_secret(secret.as_bytes()))
}

pub fn encode_refresh_token(user_id: &str, secret: &str) -> Result<String, jsonwebtoken::errors::Error> {
    let exp = (Utc::now() + Duration::days(REFRESH_TOKEN_DAYS)).timestamp();
    let claims = Claims { sub: user_id.to_string(), exp };
    encode(&Header::default(), &claims, &EncodingKey::from_secret(secret.as_bytes()))
}

pub fn decode_access_token(token: &str, secret: &str) -> Result<Claims, jsonwebtoken::errors::Error> {
    let data = decode::<Claims>(token, &DecodingKey::from_secret(secret.as_bytes()), &Validation::default())?;
    Ok(data.claims)
}

#[cfg(test)]
mod tests {
    use super::*;

    const SECRET: &str = "test-secret-at-least-32-chars-long!";

    #[test]
    fn encode_decode_roundtrip() {
        let token = encode_access_token("user-id-123", SECRET).unwrap();
        let claims = decode_access_token(&token, SECRET).unwrap();
        assert_eq!(claims.sub, "user-id-123");
    }

    #[test]
    fn wrong_secret_rejected() {
        let token = encode_access_token("user-id-123", SECRET).unwrap();
        assert!(decode_access_token(&token, "wrong-secret-padding-padding-ppp").is_err());
    }
}
