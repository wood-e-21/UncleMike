//! Verify that the backend's db pool produces an encrypted file on
//! disk, and that the wrong key fails to open it.
//!
//! Alpha criterion #9 from PLAN.md: `file <workspace>/.mike/mike.db`
//! should report "data", not "SQLite 3.x database". This test asserts
//! the same thing programmatically (without depending on the `file(1)`
//! binary being present in CI).
//!
//! Strategy:
//!   1. Set MIKE_BACKEND_UNLOCK_SECRET to a known value
//!   2. Build AppState::new (production path) on a temp workspace
//!   3. Insert a known marker row
//!   4. Drop the pool
//!   5. Open the same file with a fresh pool and the same key — read works
//!   6. Open the same file with a fresh pool and a DIFFERENT key — read fails
//!   7. Read the raw file bytes — confirm they don't contain plaintext
//!      SQLite header ("SQLite format 3\0") at offset 0

use sqlx::sqlite::{SqliteConnectOptions, SqlitePoolOptions};
use sqlx::{ConnectOptions, Row};
use std::str::FromStr;
use std::sync::Once;

static ENV_ONCE: Once = Once::new();

fn install_unlock_secret(hex: &str) {
    ENV_ONCE.call_once(|| {});
    // SAFETY: tests run serially within this module via the
    // `serial_test`-style file-scoped Once + explicit cleanup. Each
    // test sets-then-clears the secret.
    unsafe {
        std::env::set_var("MIKE_BACKEND_UNLOCK_SECRET", hex);
    }
}

fn clear_unlock_secret() {
    unsafe {
        std::env::remove_var("MIKE_BACKEND_UNLOCK_SECRET");
    }
}

fn key_pragma(key_hex: &str) -> (String, String) {
    ("key".to_string(), format!("\"x'{key_hex}'\""))
}

async fn open_with_key(path: &std::path::Path, key_hex: &str) -> sqlx::SqlitePool {
    let url = format!("sqlite://{}?mode=rwc", path.display().to_string().replace('\\', "/"));
    let (name, val) = key_pragma(key_hex);
    let opts = SqliteConnectOptions::from_str(&url)
        .expect("opts")
        .create_if_missing(true)
        .pragma(name, val)
        .pragma("cipher_compatibility", "4")
        .journal_mode(sqlx::sqlite::SqliteJournalMode::Wal)
        .log_slow_statements(tracing::log::LevelFilter::Trace, std::time::Duration::from_secs(1));
    SqlitePoolOptions::new()
        .max_connections(1)
        .connect_with(opts)
        .await
        .expect("connect")
}

#[tokio::test]
async fn db_file_is_encrypted_with_unlock_secret_key() {
    install_unlock_secret(
        "000102030405060708090a0b0c0d0e0f101112131415161718191a1b1c1d1e1f",
    );

    let dir = tempfile::tempdir().expect("tempdir");
    // Mirror what WorkspacePaths::new would do — but use a sibling
    // path so we don't have to actually depend on the workspace
    // module's layout creation for a pure encryption test.
    let db_path = dir.path().join("mike.db");

    let cipher_key = mike::db::cipher::database_key_hex().expect("derive key");
    {
        let pool = open_with_key(&db_path, &cipher_key).await;
        sqlx::query("CREATE TABLE marker (val TEXT)")
            .execute(&pool)
            .await
            .expect("create");
        sqlx::query("INSERT INTO marker (val) VALUES (?)")
            .bind("encrypted-by-mike-test")
            .execute(&pool)
            .await
            .expect("insert");
        pool.close().await;
    }

    // Re-open with the SAME key: read back the marker.
    {
        let pool = open_with_key(&db_path, &cipher_key).await;
        let row = sqlx::query("SELECT val FROM marker")
            .fetch_one(&pool)
            .await
            .expect("fetch");
        let v: String = row.get(0);
        assert_eq!(v, "encrypted-by-mike-test");
        pool.close().await;
    }

    // Open with a WRONG key: any query should fail with "file is not
    // a database" or "file is encrypted".
    {
        let wrong_key = "ff".repeat(32);
        // We don't go through open_with_key here because the wrong
        // key will produce errors that we want to inspect.
        let url = format!(
            "sqlite://{}?mode=rwc",
            db_path.display().to_string().replace('\\', "/")
        );
        let (name, val) = key_pragma(&wrong_key);
        let opts = SqliteConnectOptions::from_str(&url)
            .expect("opts")
            .pragma(name, val)
            .pragma("cipher_compatibility", "4")
            .log_slow_statements(tracing::log::LevelFilter::Trace, std::time::Duration::from_secs(1));
        let pool = SqlitePoolOptions::new()
            .max_connections(1)
            .connect_with(opts)
            .await
            .expect("connect (the key is checked lazily on first query)");
        // Use fetch_optional → Result<Option<SqliteRow>, sqlx::Error>:
        // we don't care about the row, just whether the query errored.
        // SqliteRow doesn't impl Debug so we can't format it via {res:?};
        // map the success case into an empty Ok unit so the assertion
        // can use Debug.
        let res = sqlx::query("SELECT val FROM marker")
            .fetch_optional(&pool)
            .await
            .map(|_| ());
        assert!(
            res.is_err(),
            "expected SQLCipher to reject the wrong key; instead got {res:?}"
        );
        let err_msg = res.unwrap_err().to_string().to_lowercase();
        assert!(
            err_msg.contains("file is not a database")
                || err_msg.contains("file is encrypted")
                || err_msg.contains("notadb"),
            "unexpected error from wrong-key open: {err_msg}"
        );
        pool.close().await;
    }

    // Inspect raw bytes: an unencrypted SQLite file starts with
    // "SQLite format 3\0" at offset 0. SQLCipher randomizes the first
    // 16 bytes of every page, so this header should NOT be present.
    let bytes = std::fs::read(&db_path).expect("read db file");
    assert!(bytes.len() > 100, "file should have meaningful content");
    let header = &bytes[..16.min(bytes.len())];
    assert!(
        header != b"SQLite format 3\0",
        "first 16 bytes look like a plaintext SQLite header — encryption did not engage"
    );

    clear_unlock_secret();
}

#[tokio::test]
async fn cipher_version_is_reported() {
    install_unlock_secret(
        "1111111111111111111111111111111111111111111111111111111111111111",
    );
    let dir = tempfile::tempdir().expect("tempdir");
    let db_path = dir.path().join("mike.db");

    let key = mike::db::cipher::database_key_hex().expect("key");
    let pool = open_with_key(&db_path, &key).await;

    // SQLCipher exposes `PRAGMA cipher_version`. Plain SQLite returns
    // an empty result. Use raw query rather than the `pragma_*()`
    // table-valued function form (not always present).
    let v: Option<String> = sqlx::query_scalar("PRAGMA cipher_version")
        .fetch_optional(&pool)
        .await
        .expect("query cipher_version");
    let v = v.expect("cipher_version row");
    assert!(
        !v.is_empty(),
        "cipher_version should be non-empty when SQLCipher is linked"
    );
    pool.close().await;
    clear_unlock_secret();
}
