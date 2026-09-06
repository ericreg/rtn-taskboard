use argon2::{Argon2, PasswordHash, PasswordVerifier};
use rusqlite::Connection;
use std::{
    io::Write,
    path::Path,
    process::{Command, Output, Stdio},
};

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

#[test]
fn seed_creates_editor_and_private_blobs_offline_and_never_overwrites_them() {
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
    let connection = Connection::open(&database).unwrap();
    let (email, name, role, hash): (String, String, String, String) = connection
        .query_row(
            "SELECT email,name,role,password_hash FROM users",
            [],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
        )
        .unwrap();
    assert_eq!(
        (email.as_str(), name.as_str(), role.as_str()),
        ("admin@example.test", "Admin", "editor")
    );
    Argon2::default()
        .verify_password(password.as_bytes(), &PasswordHash::new(&hash).unwrap())
        .unwrap();
    let snapshot = || {
        connection
            .query_row(
                "SELECT private_key,host_state FROM rtn_identity WHERE id=1",
                [],
                |row| Ok((row.get::<_, Vec<u8>>(0)?, row.get::<_, Vec<u8>>(1)?)),
            )
            .unwrap()
    };
    let original = snapshot();
    assert_eq!(original.0.len(), 32);
    assert!(!original.1.is_empty());
    let output = command(
        &database,
        &["seed", "someone@example.test"],
        &format!("{password}\n"),
    );
    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("already initialized"));
    assert!(snapshot() == original);
    assert_eq!(
        connection
            .query_row("SELECT COUNT(*) FROM users", [], |row| row.get::<_, i64>(0))
            .unwrap(),
        1
    );
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

#[test]
fn seed_rolls_back_editor_if_transport_storage_fails_and_can_be_retried() {
    let directory = tempfile::tempdir().unwrap();
    let database = directory.path().join("taskboard.db");
    let args = ["seed", "admin@example.test"];
    let invalid = command(&database, &args, "short\n");
    assert!(!invalid.status.success());
    let connection = Connection::open(&database).unwrap();
    let empty = || {
        assert_eq!(
            connection
                .query_row(
                    "SELECT (SELECT COUNT(*) FROM users) + (SELECT COUNT(*) FROM rtn_identity)",
                    [],
                    |row| row.get::<_, i64>(0),
                )
                .unwrap(),
            0
        );
    };
    empty();
    connection.execute_batch("CREATE TRIGGER fail_seed BEFORE INSERT ON rtn_identity BEGIN SELECT RAISE(FAIL, 'test storage failure'); END;").unwrap();
    let failed = command(&database, &args, "a long test-only passphrase\n");
    assert!(!failed.status.success());
    empty();
    connection.execute_batch("DROP TRIGGER fail_seed;").unwrap();
    assert!(
        command(&database, &args, "a long test-only passphrase\n")
            .status
            .success()
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
