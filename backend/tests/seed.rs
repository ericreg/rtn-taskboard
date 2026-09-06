use argon2::{Argon2, PasswordHash, PasswordVerifier};
use std::{
    io::Write,
    path::Path,
    process::{Command, Output, Stdio},
};
use taskboard::db::{self, Database};

fn command(database: &Path, args: &[&str], password: &str) -> Output {
    let mut child = Command::new(env!("CARGO_BIN_EXE_taskboard"))
        .env_clear()
        .env("TASKBOARD_DATABASE", database)
        .env("RUST_LOG", "off")
        .args(args)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    child
        .stdin
        .take()
        .unwrap()
        .write_all(password.as_bytes())
        .unwrap();
    child.wait_with_output().unwrap()
}

async fn snapshot(path: &Path) -> (Vec<u8>, Vec<u8>, i64) {
    let database = Database::open(path).await.unwrap();
    let conn = database.connect().await.unwrap();
    let row = db::one(
        &conn,
        "SELECT private_key,host_state,revision FROM rtn_identity WHERE id=1",
        (),
    )
    .await
    .unwrap();
    (
        row.get("private_key"),
        row.get("host_state"),
        row.get("revision"),
    )
}

#[tokio::test]
async fn seed_creates_editor_and_private_blobs_offline_and_never_overwrites_them() {
    let directory = tempfile::tempdir().unwrap();
    let database = directory.path().join("data/taskboard.db");
    let password = "a long test-only passphrase";
    let output = command(
        &database,
        &["seed", " ADMIN@example.test ", " Admin "],
        &format!("{password}\n"),
    );
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    {
        let db = Database::open(&database).await.unwrap();
        let conn = db.connect().await.unwrap();
        let row = db::one(&conn, "SELECT email,name,role,password_hash FROM users", ())
            .await
            .unwrap();
        assert_eq!(row.get::<String>("email"), "admin@example.test");
        assert_eq!(row.get::<String>("name"), "Admin");
        assert_eq!(row.get::<String>("role"), "editor");
        Argon2::default()
            .verify_password(
                password.as_bytes(),
                &PasswordHash::new(&row.get::<String>("password_hash")).unwrap(),
            )
            .unwrap();
    }
    let original = snapshot(&database).await;
    assert_eq!(original.0.len(), 32);
    assert!(!original.1.is_empty());
    let output = command(
        &database,
        &["seed", "someone@example.test"],
        &format!("{password}\n"),
    );
    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("already initialized"));
    assert_eq!(snapshot(&database).await, original);
    {
        let db = Database::open(&database).await.unwrap();
        assert_eq!(
            db::scalar::<i64>(
                &db.connect().await.unwrap(),
                "SELECT COUNT(*) FROM users",
                ()
            )
            .await
            .unwrap(),
            1
        );
    }
    assert!(!database.parent().unwrap().join("rtn").exists());
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        for entry in std::fs::read_dir(database.parent().unwrap()).unwrap() {
            assert_eq!(
                entry.unwrap().metadata().unwrap().permissions().mode() & 0o077,
                0
            );
        }
    }
}

async fn assert_empty(database: &Path) {
    let db = Database::open(database).await.unwrap();
    let count: i64 = db::scalar(
        &db.connect().await.unwrap(),
        "SELECT (SELECT COUNT(*) FROM users) + (SELECT COUNT(*) FROM rtn_identity)",
        (),
    )
    .await
    .unwrap();
    assert_eq!(count, 0);
}

#[tokio::test]
async fn seed_rolls_back_editor_if_transport_storage_fails_and_can_be_retried() {
    let directory = tempfile::tempdir().unwrap();
    let database = directory.path().join("taskboard.db");
    let args = ["seed", "admin@example.test"];
    assert!(!command(&database, &args, "short\n").status.success());
    assert_empty(&database).await;
    {
        let db = Database::open(&database).await.unwrap();
        db.connect().await.unwrap().execute_batch("CREATE TRIGGER fail_seed BEFORE INSERT ON rtn_identity BEGIN SELECT RAISE(FAIL, 'test storage failure'); END;").await.unwrap();
    }
    let failed = command(&database, &args, "a long test-only passphrase\n");
    assert!(!failed.status.success());
    assert_empty(&database).await;
    {
        let db = Database::open(&database).await.unwrap();
        db.connect()
            .await
            .unwrap()
            .execute_batch("DROP TRIGGER fail_seed;")
            .await
            .unwrap();
    }
    let output = command(&database, &args, "a long test-only passphrase\n");
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn normal_start_and_join_code_require_seed_and_do_not_generate_files() {
    let directory = tempfile::tempdir().unwrap();
    let database = directory.path().join("taskboard.db");
    for args in [vec![], vec!["issue-gateway-code"]] {
        let output = command(&database, &args, "");
        assert!(!output.status.success());
        assert!(String::from_utf8_lossy(&output.stderr).contains("taskboard seed"));
        assert!(!database.exists());
    }
}

#[tokio::test]
async fn rejects_previous_database_without_changing_its_records() {
    let directory = tempfile::tempdir().unwrap();
    let database = directory.path().join("taskboard.db");
    // An unmarked database represents the pre-Turso schema. No legacy driver is used.
    {
        let db = turso::Builder::new_local(database.to_str().unwrap())
            .experimental_without_rowid(true)
            .build()
            .await
            .unwrap();
        db.connect().unwrap().execute_batch("CREATE TABLE users(id INTEGER PRIMARY KEY, email TEXT); INSERT INTO users VALUES(1,'existing@example.test'); CREATE TABLE attachment_chunks(attachment_id TEXT, sequence INTEGER, data BLOB, PRIMARY KEY(attachment_id,sequence)) WITHOUT ROWID;").await.unwrap();
    }
    for args in [
        vec!["seed", "new@example.test"],
        vec!["status"],
        vec!["issue-gateway-code"],
        vec![],
    ] {
        let output = command(&database, &args, "a long test-only passphrase\n");
        assert!(!output.status.success());
        assert!(
            String::from_utf8_lossy(&output.stderr).contains("requires a fresh database"),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
    }
    let db = turso::Builder::new_local(database.to_str().unwrap())
        .experimental_without_rowid(true)
        .build()
        .await
        .unwrap();
    let conn = db.connect().unwrap();
    let email: String = db::scalar(&conn, "SELECT email FROM users", ())
        .await
        .unwrap();
    assert_eq!(email, "existing@example.test");
    let tables: i64 = db::scalar(
        &conn,
        "SELECT COUNT(*) FROM sqlite_schema WHERE type='table'",
        (),
    )
    .await
    .unwrap();
    assert_eq!(tables, 2);
}

#[tokio::test]
async fn maintenance_requires_exclusive_access_and_invalidates_all_token_tables() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("taskboard.db");
    assert!(
        command(
            &path,
            &["seed", "admin@example.test"],
            "a long test-only passphrase\n"
        )
        .status
        .success()
    );
    let original = snapshot(&path).await;
    {
        let database = Database::open(&path).await.unwrap();
        let conn = database.connect().await.unwrap();
        conn.execute_batch("INSERT INTO sessions VALUES('session',1,'csrf',9999999999); INSERT INTO account_tokens(token_hash,email,purpose,expires_at) VALUES('invite','invite@example.test','invite',9999999999); INSERT INTO discord_link_tokens(token_hash,user_id,expires_at) VALUES('link',1,9999999999);").await.unwrap();
        assert!(
            !command(&path, &["status"], "").status.success(),
            "A second process must not open the live database"
        );
    }
    let status = command(&path, &["status"], "");
    assert!(
        status.status.success(),
        "{}",
        String::from_utf8_lossy(&status.stderr)
    );
    let output = command(&path, &["invalidate-sessions"], "");
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(snapshot(&path).await, original);
    let database = Database::open(&path).await.unwrap();
    let count: i64 = db::scalar(&database.connect().await.unwrap(), "SELECT (SELECT COUNT(*) FROM sessions) + (SELECT COUNT(*) FROM account_tokens) + (SELECT COUNT(*) FROM discord_link_tokens)", ()).await.unwrap();
    assert_eq!(count, 0);
}
