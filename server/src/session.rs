use jsonwebtoken::{decode, Algorithm, DecodingKey, Validation};
use serde::Deserialize;

#[derive(Deserialize)]
struct SessionClaims {
    session_id: String,
}

/// Verify an HS256 JWT and extract the `session_id` claim.
/// Returns `None` if the token is invalid, expired, or missing the claim.
pub fn verify_session_jwt(token: &str, secret: &str) -> Option<String> {
    let dk = DecodingKey::from_secret(secret.as_bytes());
    let mut validation = Validation::new(Algorithm::HS256);
    validation.required_spec_claims = ["exp".to_string()].into();
    decode::<SessionClaims>(token, &dk, &validation)
        .ok()
        .map(|d| d.claims.session_id)
}
