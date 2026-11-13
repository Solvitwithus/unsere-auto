//! User JWTs and station keys.

use crate::error::AppError;
use crate::state::AppState;
use axum::extract::FromRequestParts;
use axum::http::request::Parts;
use jsonwebtoken::{decode, encode, DecodingKey, EncodingKey, Header, Validation};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use uuid::Uuid;

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Claims {
    pub sub: Uuid,
    pub username: String,
    pub name: String,
    pub role: String,
    pub site_id: Option<Uuid>,
    pub exp: usize,
}

pub fn issue(secret: &str, c: &Claims) -> anyhow::Result<String> {
    Ok(encode(&Header::default(), c, &EncodingKey::from_secret(secret.as_bytes()))?)
}

/// A signed-in dashboard user.
pub struct AuthUser(pub Claims);

impl AuthUser {
    /// Admins can do everything; others need one of `roles`.
    pub fn require(&self, roles: &[&str]) -> Result<(), AppError> {
        if self.0.role == "admin" || roles.contains(&self.0.role.as_str()) {
            Ok(())
        } else {
            Err(AppError::Forbidden)
        }
    }
    pub fn actor(&self) -> String {
        self.0.username.clone()
    }
}

impl FromRequestParts<AppState> for AuthUser {
    type Rejection = AppError;

    async fn from_request_parts(parts: &mut Parts, state: &AppState) -> Result<Self, Self::Rejection> {
        let token = parts
            .headers
            .get("authorization")
            .and_then(|v| v.to_str().ok())
            .and_then(|v| v.strip_prefix("Bearer "))
            .ok_or(AppError::Unauthorized)?;
        let data = decode::<Claims>(token, &DecodingKey::from_secret(state.jwt_secret.as_bytes()), &Validation::default())
            .map_err(|_| AppError::Unauthorized)?;
        // Deactivated users lose access immediately, not when the token expires.
        let active: Option<bool> = sqlx::query_scalar("SELECT active FROM users WHERE id = $1")
            .bind(data.claims.sub)
            .fetch_optional(&state.db)
            .await?;
        if active != Some(true) {
            return Err(AppError::Unauthorized);
        }
        Ok(AuthUser(data.claims))
    }
}

/// Station keys look like `wbk_<station uuid, simple>_<secret>`; only a SHA-256 of the secret is stored.
pub fn new_station_key(station_id: Uuid) -> (String, String) {
    use rand::RngCore;
    let mut b = [0u8; 24];
    rand::thread_rng().fill_bytes(&mut b);
    let secret = hex::encode(b);
    (format!("wbk_{}_{}", station_id.simple(), secret), sha256_hex(&secret))
}

pub fn sha256_hex(s: &str) -> String {
    hex::encode(Sha256::digest(s.as_bytes()))
}

/// An authenticated station (header `X-Station-Key`).
pub struct StationAuth {
    pub station_id: Uuid,
    pub site_id: Uuid,
    pub code: String,
}

impl FromRequestParts<AppState> for StationAuth {
    type Rejection = AppError;

    async fn from_request_parts(parts: &mut Parts, state: &AppState) -> Result<Self, Self::Rejection> {
        let key = parts.headers.get("x-station-key").and_then(|v| v.to_str().ok()).ok_or(AppError::Unauthorized)?;
        let mut it = key.splitn(3, '_');
        let (Some("wbk"), Some(id), Some(secret)) = (it.next(), it.next(), it.next()) else {
            return Err(AppError::Unauthorized);
        };
        let id = Uuid::parse_str(id).map_err(|_| AppError::Unauthorized)?;
        let row: Option<(String, Uuid, String, bool)> =
            sqlx::query_as("SELECT key_hash, site_id, code, active FROM stations WHERE id = $1").bind(id).fetch_optional(&state.db).await?;
        let Some((hash, site_id, code, active)) = row else { return Err(AppError::Unauthorized) };
        let given = sha256_hex(secret);
        // constant-time compare
        let same = hash.len() == given.len() && hash.bytes().zip(given.bytes()).fold(0u8, |a, (x, y)| a | (x ^ y)) == 0;
        if !same || !active {
            return Err(AppError::Unauthorized);
        }
        Ok(StationAuth { station_id: id, site_id, code })
    }
}
