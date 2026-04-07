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
    use crate::db::Db;
    use std::collections::HashSet;

    async fn test_pool() -> SqlitePool {
        let db = Db::new_in_memory().await.expect("in-memory db");
        db.migrate().await.expect("migration");
        db.pool().clone()
    }

    /// Insert a test user directly (bypasses the user model to keep tests self-contained).
    async fn insert_test_user(pool: &SqlitePool, id: &str) {
        sqlx::query(
            "INSERT INTO users (id, username, password_hash, created_at) VALUES (?, ?, ?, ?)",
        )
        .bind(id)
        .bind(format!("user-{id}"))
        .bind("$argon2id$v=19$m=19456,t=2,p=1$fakesalt$fakehash")
        .bind("2026-01-01T00:00:00.000Z")
        .execute(pool)
        .await
        .expect("insert test user");
    }

    // --- Token generation tests ---

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

    // --- CRUD integration tests ---

    #[tokio::test]
    async fn create_returns_session_with_valid_token() {
        let pool = test_pool().await;
        insert_test_user(&pool, "u1").await;

        let session = create(&pool, "u1", Duration::from_secs(3600))
            .await
            .unwrap();

        assert_eq!(session.id.len(), 43);
        assert_eq!(session.user_id, "u1");
        assert!(!session.created_at.is_empty());
        assert!(!session.expires_at.is_empty());
        assert!(session.expires_at > session.created_at);
    }

    #[tokio::test]
    async fn create_generates_unique_tokens() {
        let pool = test_pool().await;
        insert_test_user(&pool, "u1").await;

        let tokens: HashSet<String> = {
            let mut set = HashSet::new();
            for _ in 0..10 {
                let s = create(&pool, "u1", Duration::from_secs(3600))
                    .await
                    .unwrap();
                set.insert(s.id);
            }
            set
        };
        assert_eq!(tokens.len(), 10);
    }

    #[tokio::test]
    async fn find_by_token_returns_valid_session() {
        let pool = test_pool().await;
        insert_test_user(&pool, "u1").await;

        let session = create(&pool, "u1", Duration::from_secs(3600))
            .await
            .unwrap();
        let found = find_by_token(&pool, &session.id).await.unwrap();

        assert!(found.is_some());
        let found = found.unwrap();
        assert_eq!(found.id, session.id);
        assert_eq!(found.user_id, "u1");
    }

    #[tokio::test]
    async fn find_by_token_returns_none_for_unknown_token() {
        let pool = test_pool().await;
        let found = find_by_token(&pool, "nonexistent-token").await.unwrap();
        assert!(found.is_none());
    }

    #[tokio::test]
    async fn find_by_token_returns_none_for_expired_session() {
        let pool = test_pool().await;
        insert_test_user(&pool, "u1").await;

        // Insert a session with expires_at in the past
        sqlx::query(
            "INSERT INTO sessions (id, user_id, created_at, expires_at) VALUES (?, ?, ?, ?)",
        )
        .bind("expired-token")
        .bind("u1")
        .bind("2020-01-01T00:00:00.000Z")
        .bind("2020-01-02T00:00:00.000Z")
        .execute(&pool)
        .await
        .unwrap();

        let found = find_by_token(&pool, "expired-token").await.unwrap();
        assert!(found.is_none());
    }

    #[tokio::test]
    async fn delete_removes_session() {
        let pool = test_pool().await;
        insert_test_user(&pool, "u1").await;

        let session = create(&pool, "u1", Duration::from_secs(3600))
            .await
            .unwrap();
        delete(&pool, &session.id).await.unwrap();

        let found = find_by_token(&pool, &session.id).await.unwrap();
        assert!(found.is_none());
    }

    #[tokio::test]
    async fn delete_is_idempotent_for_missing_token() {
        let pool = test_pool().await;
        // Should not error when deleting a non-existent token
        delete(&pool, "nonexistent-token").await.unwrap();
    }

    #[tokio::test]
    async fn delete_all_for_user_removes_all_sessions() {
        let pool = test_pool().await;
        insert_test_user(&pool, "u1").await;

        let mut tokens = Vec::new();
        for _ in 0..3 {
            let s = create(&pool, "u1", Duration::from_secs(3600))
                .await
                .unwrap();
            tokens.push(s.id);
        }

        let count = delete_all_for_user(&pool, "u1").await.unwrap();
        assert_eq!(count, 3);

        for token in &tokens {
            let found = find_by_token(&pool, token).await.unwrap();
            assert!(found.is_none());
        }
    }

    #[tokio::test]
    async fn delete_all_for_user_does_not_affect_other_users() {
        let pool = test_pool().await;
        insert_test_user(&pool, "u1").await;
        insert_test_user(&pool, "u2").await;

        create(&pool, "u1", Duration::from_secs(3600))
            .await
            .unwrap();
        let s2 = create(&pool, "u2", Duration::from_secs(3600))
            .await
            .unwrap();

        delete_all_for_user(&pool, "u1").await.unwrap();

        // User 2's session should still be valid
        let found = find_by_token(&pool, &s2.id).await.unwrap();
        assert!(found.is_some());
    }

    #[tokio::test]
    async fn delete_expired_removes_only_expired_sessions() {
        let pool = test_pool().await;
        insert_test_user(&pool, "u1").await;

        // Insert an expired session directly
        sqlx::query(
            "INSERT INTO sessions (id, user_id, created_at, expires_at) VALUES (?, ?, ?, ?)",
        )
        .bind("old-token")
        .bind("u1")
        .bind("2020-01-01T00:00:00.000Z")
        .bind("2020-01-02T00:00:00.000Z")
        .execute(&pool)
        .await
        .unwrap();

        // Create a valid session
        let valid = create(&pool, "u1", Duration::from_secs(3600))
            .await
            .unwrap();

        let count = delete_expired(&pool).await.unwrap();
        assert_eq!(count, 1);

        // Valid session still exists
        let found = find_by_token(&pool, &valid.id).await.unwrap();
        assert!(found.is_some());

        // Expired session is gone (verify via raw query since find_by_token filters by expiry)
        let row: Option<(String,)> =
            sqlx::query_as("SELECT id FROM sessions WHERE id = ?")
                .bind("old-token")
                .fetch_optional(&pool)
                .await
                .unwrap();
        assert!(row.is_none());
    }

    #[tokio::test]
    async fn delete_expired_returns_zero_when_no_expired_sessions() {
        let pool = test_pool().await;
        insert_test_user(&pool, "u1").await;

        create(&pool, "u1", Duration::from_secs(3600))
            .await
            .unwrap();

        let count = delete_expired(&pool).await.unwrap();
        assert_eq!(count, 0);
    }
}
