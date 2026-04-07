use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use base64::Engine;
use rand::rngs::OsRng;
use rand::TryRngCore;
use serde::Serialize;
use sqlx::SqlitePool;
use std::time::Duration;

use crate::error::DbResult;
use crate::timestamp;

// --- Types ---

#[derive(Debug, Clone, Serialize, sqlx::FromRow)]
pub struct Session {
    pub id: String,
    pub user_id: String,
    pub created_at: String,
    pub expires_at: String,
}

// --- Token generation ---

/// Generate a cryptographically random 32-byte session token, base64url-encoded (no padding).
fn generate_token() -> String {
    let mut bytes = [0u8; 32];
    OsRng.try_fill_bytes(&mut bytes).expect("OS RNG failed");
    URL_SAFE_NO_PAD.encode(bytes)
}

// --- Query functions ---

/// Create a new session for a user. Generates a random token and sets
/// expiry to `now + session_lifetime`. Returns the session with its token.
pub async fn create(
    pool: &SqlitePool,
    user_id: &str,
    session_lifetime: Duration,
) -> DbResult<Session> {
    let id = generate_token();
    let created_at = timestamp::now_utc();
    let expires_at = (chrono::Utc::now()
        + chrono::Duration::from_std(session_lifetime)
            .unwrap_or(chrono::Duration::hours(24)))
    .format("%Y-%m-%dT%H:%M:%S%.3fZ")
    .to_string();

    sqlx::query(
        "INSERT INTO sessions (id, user_id, created_at, expires_at) VALUES (?, ?, ?, ?)",
    )
    .bind(&id)
    .bind(user_id)
    .bind(&created_at)
    .bind(&expires_at)
    .execute(pool)
    .await?;

    Ok(Session {
        id,
        user_id: user_id.to_owned(),
        created_at,
        expires_at,
    })
}

/// Find a valid (non-expired) session by its token. Returns None if the
/// token does not exist or the session has expired.
pub async fn find_by_token(pool: &SqlitePool, token: &str) -> DbResult<Option<Session>> {
    let now = timestamp::now_utc();
    let row = sqlx::query_as::<_, Session>(
        "SELECT id, user_id, created_at, expires_at \
         FROM sessions WHERE id = ? AND expires_at > ?",
    )
    .bind(token)
    .bind(&now)
    .fetch_optional(pool)
    .await?;
    Ok(row)
}

/// Delete a session by its token. Used for logout.
pub async fn delete(pool: &SqlitePool, token: &str) -> DbResult<()> {
    sqlx::query("DELETE FROM sessions WHERE id = ?")
        .bind(token)
        .execute(pool)
        .await?;
    Ok(())
}

/// Delete all sessions for a user. Used when deleting a user account
/// or implementing "log out everywhere".
pub async fn delete_all_for_user(pool: &SqlitePool, user_id: &str) -> DbResult<u64> {
    let result = sqlx::query("DELETE FROM sessions WHERE user_id = ?")
        .bind(user_id)
        .execute(pool)
        .await?;
    Ok(result.rows_affected())
}

/// Delete all expired sessions. Returns the number of sessions deleted.
pub async fn delete_expired(pool: &SqlitePool) -> DbResult<u64> {
    let now = timestamp::now_utc();
    let result = sqlx::query("DELETE FROM sessions WHERE expires_at <= ?")
        .bind(&now)
        .execute(pool)
        .await?;
    Ok(result.rows_affected())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;

    #[test]
    fn generate_token_produces_43_char_base64url_string() {
        let token = generate_token();
        assert_eq!(token.len(), 43);
        // base64url alphabet: A-Z, a-z, 0-9, -, _
        assert!(token
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_'));
    }

    #[test]
    fn generate_token_produces_unique_tokens() {
        let tokens: HashSet<String> = (0..100).map(|_| generate_token()).collect();
        assert_eq!(tokens.len(), 100);
    }
}
