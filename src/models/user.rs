use argon2::password_hash::SaltString;
use argon2::{Argon2, PasswordHash, PasswordHasher, PasswordVerifier};
use rand_core::OsRng;
use serde::Serialize;
use sqlx::SqlitePool;

use crate::error::{DbError, DbResult};
use crate::id;
use crate::timestamp;

// --- Types ---

#[derive(Debug, Clone, Serialize, sqlx::FromRow)]
pub struct User {
    pub id: String,
    pub username: String,
    #[serde(skip)]
    pub password_hash: String,
    pub created_at: String,
}

// --- Password hashing ---

/// Hash a plaintext password with argon2id. Returns the PHC-formatted hash string.
pub fn hash_password(password: &str) -> DbResult<String> {
    let salt = SaltString::generate(&mut OsRng);
    let argon2 = Argon2::default();
    argon2
        .hash_password(password.as_bytes(), &salt)
        .map(|h| h.to_string())
        .map_err(|e| DbError::Validation(format!("password hashing failed: {e}")))
}

/// Verify a plaintext password against a stored argon2id hash.
pub fn verify_password(password: &str, hash: &str) -> bool {
    let Ok(parsed) = PasswordHash::new(hash) else {
        return false;
    };
    Argon2::default()
        .verify_password(password.as_bytes(), &parsed)
        .is_ok()
}

// --- Username validation ---

/// Validates a username: 3-32 characters, alphanumeric + hyphens, no leading/trailing hyphens.
pub fn validate_username(username: &str) -> Result<(), &'static str> {
    let len = username.len();
    if !(3..=32).contains(&len) {
        return Err("username must be 3-32 characters");
    }

    let bytes = username.as_bytes();
    if bytes[0] == b'-' || bytes[len - 1] == b'-' {
        return Err("username must not start or end with a hyphen");
    }

    for &b in bytes {
        if !matches!(b, b'a'..=b'z' | b'A'..=b'Z' | b'0'..=b'9' | b'-') {
            return Err("username must contain only alphanumeric characters and hyphens");
        }
    }

    Ok(())
}

// --- Query functions ---

/// Create a new user. Returns the created record.
pub async fn create(pool: &SqlitePool, username: &str, password: &str) -> DbResult<User> {
    validate_username(username)
        .map_err(|e| DbError::Validation(e.to_string()))?;

    let password_hash = hash_password(password)?;
    let id = id::new_id();
    let created_at = timestamp::now_utc();

    sqlx::query(
        "INSERT INTO users (id, username, password_hash, created_at) VALUES (?, ?, ?, ?)",
    )
    .bind(&id)
    .bind(username)
    .bind(&password_hash)
    .bind(&created_at)
    .execute(pool)
    .await
    .map_err(|e| match e {
        sqlx::Error::Database(ref db_err) if db_err.message().contains("UNIQUE") => {
            DbError::Conflict(format!("username '{username}' already exists"))
        }
        other => DbError::from(other),
    })?;

    get_by_id(pool, &id).await
}

/// Fetch a user by primary key.
pub async fn get_by_id(pool: &SqlitePool, id: &str) -> DbResult<User> {
    sqlx::query_as::<_, User>(
        "SELECT id, username, password_hash, created_at FROM users WHERE id = ?",
    )
    .bind(id)
    .fetch_optional(pool)
    .await?
    .ok_or_else(|| DbError::NotFound(format!("user {id}")))
}

/// Fetch a user by username.
pub async fn get_by_username(pool: &SqlitePool, username: &str) -> DbResult<User> {
    sqlx::query_as::<_, User>(
        "SELECT id, username, password_hash, created_at FROM users WHERE username = ?",
    )
    .bind(username)
    .fetch_optional(pool)
    .await?
    .ok_or_else(|| DbError::NotFound(format!("user '{username}'")))
}

/// List all users, ordered by created_at ASC.
pub async fn list(pool: &SqlitePool) -> DbResult<Vec<User>> {
    let rows = sqlx::query_as::<_, User>(
        "SELECT id, username, password_hash, created_at FROM users ORDER BY created_at ASC",
    )
    .fetch_all(pool)
    .await?;
    Ok(rows)
}

/// Delete a user by ID. Returns an error if the user does not exist.
pub async fn delete(pool: &SqlitePool, id: &str) -> DbResult<()> {
    let result = sqlx::query("DELETE FROM users WHERE id = ?")
        .bind(id)
        .execute(pool)
        .await?;

    if result.rows_affected() == 0 {
        return Err(DbError::NotFound(format!("user {id}")));
    }
    Ok(())
}

/// Update a user's password. Hashes the new password and stores it.
pub async fn update_password(pool: &SqlitePool, id: &str, new_password: &str) -> DbResult<()> {
    let password_hash = hash_password(new_password)?;

    let result = sqlx::query("UPDATE users SET password_hash = ? WHERE id = ?")
        .bind(&password_hash)
        .bind(id)
        .execute(pool)
        .await?;

    if result.rows_affected() == 0 {
        return Err(DbError::NotFound(format!("user {id}")));
    }
    Ok(())
}

/// Count total users.
pub async fn count(pool: &SqlitePool) -> DbResult<i64> {
    let row: (i64,) = sqlx::query_as("SELECT COUNT(*) FROM users")
        .fetch_one(pool)
        .await?;
    Ok(row.0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::Db;

    async fn test_pool() -> SqlitePool {
        let db = Db::new_in_memory().await.expect("in-memory db");
        db.migrate().await.expect("migration");
        db.pool().clone()
    }

    // --- Password hashing tests ---

    #[test]
    fn hash_and_verify_password_roundtrips() {
        let hash = hash_password("correct-horse-battery-staple").unwrap();
        assert!(verify_password("correct-horse-battery-staple", &hash));
        assert!(!verify_password("wrong-password", &hash));
    }

    #[test]
    fn hash_password_produces_unique_salts() {
        let h1 = hash_password("same-password").unwrap();
        let h2 = hash_password("same-password").unwrap();
        assert_ne!(h1, h2, "hashes should differ due to unique salts");
        // Both should still verify
        assert!(verify_password("same-password", &h1));
        assert!(verify_password("same-password", &h2));
    }

    #[test]
    fn verify_password_rejects_malformed_hash() {
        assert!(!verify_password("anything", "not-a-valid-hash"));
    }

    // --- Username validation tests ---

    #[test]
    fn validate_username_accepts_valid() {
        assert!(validate_username("admin").is_ok());
        assert!(validate_username("user-1").is_ok());
        assert!(validate_username("abc").is_ok());
        assert!(validate_username("a".repeat(32).as_str()).is_ok());
    }

    #[test]
    fn validate_username_rejects_too_short() {
        assert!(validate_username("ab").is_err());
        assert!(validate_username("a").is_err());
        assert!(validate_username("").is_err());
    }

    #[test]
    fn validate_username_rejects_too_long() {
        assert!(validate_username("a".repeat(33).as_str()).is_err());
    }

    #[test]
    fn validate_username_rejects_leading_trailing_hyphen() {
        assert!(validate_username("-admin").is_err());
        assert!(validate_username("admin-").is_err());
    }

    #[test]
    fn validate_username_rejects_invalid_chars() {
        assert!(validate_username("admin@home").is_err());
        assert!(validate_username("user name").is_err());
        assert!(validate_username("user_name").is_err());
    }

    // --- Database query tests ---

    #[tokio::test]
    async fn create_user_and_fetch_by_id() {
        let pool = test_pool().await;
        let user = create(&pool, "admin", "secret123").await.unwrap();

        assert_eq!(user.username, "admin");
        assert!(!user.id.is_empty());
        assert!(!user.created_at.is_empty());
        assert!(!user.password_hash.is_empty());

        let fetched = get_by_id(&pool, &user.id).await.unwrap();
        assert_eq!(fetched.id, user.id);
        assert_eq!(fetched.username, user.username);
    }

    #[tokio::test]
    async fn create_user_rejects_duplicate_username() {
        let pool = test_pool().await;
        create(&pool, "admin", "password1").await.unwrap();
        let result = create(&pool, "admin", "password2").await;
        assert!(matches!(result, Err(DbError::Conflict(_))));
    }

    #[tokio::test]
    async fn create_user_rejects_invalid_username() {
        let pool = test_pool().await;
        let result = create(&pool, "ab", "password").await;
        assert!(matches!(result, Err(DbError::Validation(_))));
    }

    #[tokio::test]
    async fn get_by_username_finds_existing() {
        let pool = test_pool().await;
        let created = create(&pool, "testuser", "password").await.unwrap();
        let found = get_by_username(&pool, "testuser").await.unwrap();
        assert_eq!(found.id, created.id);
    }

    #[tokio::test]
    async fn get_by_username_returns_not_found() {
        let pool = test_pool().await;
        let result = get_by_username(&pool, "nonexistent").await;
        assert!(matches!(result, Err(DbError::NotFound(_))));
    }

    #[tokio::test]
    async fn list_returns_all_users_ordered() {
        let pool = test_pool().await;
        create(&pool, "alice", "password").await.unwrap();
        create(&pool, "bob", "password").await.unwrap();

        let users = list(&pool).await.unwrap();
        assert_eq!(users.len(), 2);
        // Created in order, so alice first (earlier created_at)
        assert_eq!(users[0].username, "alice");
        assert_eq!(users[1].username, "bob");
    }

    #[tokio::test]
    async fn delete_removes_user() {
        let pool = test_pool().await;
        let user = create(&pool, "delete-me", "password").await.unwrap();

        delete(&pool, &user.id).await.unwrap();
        let result = get_by_id(&pool, &user.id).await;
        assert!(matches!(result, Err(DbError::NotFound(_))));
    }

    #[tokio::test]
    async fn delete_nonexistent_returns_not_found() {
        let pool = test_pool().await;
        let result = delete(&pool, "nonexistent").await;
        assert!(matches!(result, Err(DbError::NotFound(_))));
    }

    #[tokio::test]
    async fn update_password_changes_hash() {
        let pool = test_pool().await;
        let user = create(&pool, "admin", "old-password").await.unwrap();
        let old_hash = user.password_hash.clone();

        update_password(&pool, &user.id, "new-password").await.unwrap();
        let updated = get_by_id(&pool, &user.id).await.unwrap();

        assert_ne!(updated.password_hash, old_hash);
        assert!(verify_password("new-password", &updated.password_hash));
        assert!(!verify_password("old-password", &updated.password_hash));
    }

    #[tokio::test]
    async fn update_password_nonexistent_returns_not_found() {
        let pool = test_pool().await;
        let result = update_password(&pool, "nonexistent", "password").await;
        assert!(matches!(result, Err(DbError::NotFound(_))));
    }

    #[tokio::test]
    async fn count_returns_correct_number() {
        let pool = test_pool().await;
        assert_eq!(count(&pool).await.unwrap(), 0);

        create(&pool, "user1", "password").await.unwrap();
        assert_eq!(count(&pool).await.unwrap(), 1);

        create(&pool, "user2", "password").await.unwrap();
        assert_eq!(count(&pool).await.unwrap(), 2);
    }

    #[tokio::test]
    async fn created_user_password_verifies() {
        let pool = test_pool().await;
        let user = create(&pool, "admin", "my-secret-password").await.unwrap();
        assert!(verify_password("my-secret-password", &user.password_hash));
    }
}
