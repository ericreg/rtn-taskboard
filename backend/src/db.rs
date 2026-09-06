//! Embedded Turso storage and the application's row decoding helpers.
use std::{path::Path, sync::Arc, time::Duration};
use turso::{Connection, IntoParams, Value};

pub use turso::Error;
pub type Result<T> = std::result::Result<T, Error>;

const APPLICATION_ID: i64 = 0x52544e54;
const SCHEMA_VERSION: i64 = 1;

#[derive(Clone)]
pub struct Database(turso::Database);

impl Database {
    pub async fn open(path: &Path) -> anyhow::Result<Self> {
        let filename = path
            .to_str()
            .ok_or_else(|| anyhow::anyhow!("Database path must be UTF-8"))?;
        prepare_files(path)?;
        let db = Self(turso::Builder::new_local(filename).build().await?);
        let mut conn = db.connect().await?;
        let tx = conn
            .transaction_with_behavior(turso::transaction::TransactionBehavior::Immediate)
            .await?;
        let application: i64 = scalar(&tx, "PRAGMA application_id", ()).await?;
        let version: i64 = scalar(&tx, "PRAGMA user_version", ()).await?;
        let tables: i64 = scalar(
            &tx,
            "SELECT COUNT(*) FROM sqlite_schema WHERE name NOT LIKE 'sqlite_%'",
            (),
        )
        .await?;
        if application == 0 && version == 0 && tables == 0 {
            tx.execute_batch(include_str!("../schema.sql")).await?;
            tx.execute(&format!("PRAGMA application_id={APPLICATION_ID}"), ())
                .await?;
            tx.execute(&format!("PRAGMA user_version={SCHEMA_VERSION}"), ())
                .await?;
        } else {
            anyhow::ensure!(
                application == APPLICATION_ID && version == SCHEMA_VERSION,
                "Unsupported database. This Turso release requires a fresh database; stop the backend, move the old data directory aside, and run just seed. Existing databases are not migrated."
            );
        }
        tx.commit().await?;
        Ok(db)
    }

    pub async fn connect(&self) -> Result<Connection> {
        let conn = self.0.connect()?;
        conn.busy_timeout(Duration::from_secs(15))?;
        conn.execute_batch("PRAGMA foreign_keys=ON; PRAGMA synchronous=FULL;")
            .await?;
        Ok(conn)
    }

    pub async fn checkpoint(&self) -> Result<()> {
        let conn = self.connect().await?;
        let row = one(&conn, "PRAGMA wal_checkpoint(TRUNCATE)", ()).await?;
        if i64::decode(row.values[0].clone())? != 0 {
            return Err(Error::Busy("Database checkpoint is busy".into()));
        }
        Ok(())
    }
}

pub struct WriteResult {
    affected: u64,
    id: i64,
}
impl WriteResult {
    pub fn rows_affected(&self) -> u64 {
        self.affected
    }
    pub fn last_insert_rowid(&self) -> i64 {
        self.id
    }
}
pub async fn execute(conn: &Connection, sql: &str, params: impl IntoParams) -> Result<WriteResult> {
    let affected = conn.execute(sql, params).await?;
    Ok(WriteResult {
        affected,
        id: conn.last_insert_rowid(),
    })
}

pub struct Row {
    names: Arc<Vec<String>>,
    values: Vec<Value>,
}
impl Row {
    fn read(row: turso::Row, names: Arc<Vec<String>>) -> Result<Self> {
        let values = (0..names.len())
            .map(|i| row.get_value(i))
            .collect::<Result<_>>()?;
        Ok(Self { names, values })
    }
    pub fn try_get<T: Decode>(&self, name: &str) -> Result<T> {
        let index = self
            .names
            .iter()
            .position(|n| n == name)
            .ok_or_else(|| Error::ConversionFailure(format!("Missing column: {name}")))?;
        T::decode(self.values[index].clone())
    }
    pub fn get<T: Decode>(&self, name: &str) -> T {
        self.try_get(name)
            .expect("query columns must match the application schema")
    }
    pub fn get_or_default<T: Decode + Default>(&self, name: &str) -> Result<T> {
        if self.names.iter().any(|n| n == name) {
            self.try_get(name)
        } else {
            Ok(T::default())
        }
    }
}

pub trait Decode: Sized {
    fn decode(value: Value) -> Result<Self>;
}
macro_rules! decode {
    ($ty:ty, $variant:ident) => {
        impl Decode for $ty {
            fn decode(value: Value) -> Result<Self> {
                match value {
                    Value::$variant(v) => Ok(v),
                    _ => Err(Error::ConversionFailure(
                        concat!("Expected ", stringify!($variant)).into(),
                    )),
                }
            }
        }
    };
}
decode!(i64, Integer);
decode!(String, Text);
decode!(Vec<u8>, Blob);
impl Decode for bool {
    fn decode(value: Value) -> Result<Self> {
        Ok(i64::decode(value)? != 0)
    }
}
impl<T: Decode> Decode for Option<T> {
    fn decode(value: Value) -> Result<Self> {
        match value {
            Value::Null => Ok(None),
            other => T::decode(other).map(Some),
        }
    }
}
pub trait FromRow: Sized {
    fn from_row(row: Row) -> Result<Self>;
}
impl FromRow for (i64, i64) {
    fn from_row(row: Row) -> Result<Self> {
        Ok((
            i64::decode(row.values[0].clone())?,
            i64::decode(row.values[1].clone())?,
        ))
    }
}

pub async fn all(conn: &Connection, sql: &str, params: impl IntoParams) -> Result<Vec<Row>> {
    let mut rows = conn.query(sql, params).await?;
    let names = Arc::new(rows.column_names());
    let mut result = Vec::new();
    while let Some(row) = rows.next().await? {
        result.push(Row::read(row, names.clone())?);
    }
    Ok(result)
}
pub async fn optional(
    conn: &Connection,
    sql: &str,
    params: impl IntoParams,
) -> Result<Option<Row>> {
    let mut rows = conn.query(sql, params).await?;
    let names = Arc::new(rows.column_names());
    let row = rows
        .next()
        .await?
        .map(|row| Row::read(row, names))
        .transpose()?;
    // Finish the statement so locks are released before the next write.
    while rows.next().await?.is_some() {}
    Ok(row)
}
pub async fn one(conn: &Connection, sql: &str, params: impl IntoParams) -> Result<Row> {
    optional(conn, sql, params)
        .await?
        .ok_or(Error::QueryReturnedNoRows)
}
pub async fn all_as<T: FromRow>(
    conn: &Connection,
    sql: &str,
    params: impl IntoParams,
) -> Result<Vec<T>> {
    all(conn, sql, params)
        .await?
        .into_iter()
        .map(T::from_row)
        .collect()
}
pub async fn optional_as<T: FromRow>(
    conn: &Connection,
    sql: &str,
    params: impl IntoParams,
) -> Result<Option<T>> {
    optional(conn, sql, params)
        .await?
        .map(T::from_row)
        .transpose()
}
pub async fn one_as<T: FromRow>(
    conn: &Connection,
    sql: &str,
    params: impl IntoParams,
) -> Result<T> {
    T::from_row(one(conn, sql, params).await?)
}
pub async fn scalar<T: Decode>(conn: &Connection, sql: &str, params: impl IntoParams) -> Result<T> {
    T::decode(one(conn, sql, params).await?.values.remove(0))
}
pub async fn optional_scalar<T: Decode>(
    conn: &Connection,
    sql: &str,
    params: impl IntoParams,
) -> Result<Option<T>> {
    optional(conn, sql, params)
        .await?
        .map(|mut r| T::decode(r.values.remove(0)))
        .transpose()
}
pub async fn scalars<T: Decode>(
    conn: &Connection,
    sql: &str,
    params: impl IntoParams,
) -> Result<Vec<T>> {
    all(conn, sql, params)
        .await?
        .into_iter()
        .map(|mut r| T::decode(r.values.remove(0)))
        .collect()
}

fn prepare_files(path: &std::path::Path) -> anyhow::Result<()> {
    let mut directory = std::fs::DirBuilder::new();
    directory.recursive(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::DirBuilderExt;
        directory.mode(0o700);
    }
    if let Some(parent) = path.parent().filter(|p| !p.as_os_str().is_empty()) {
        directory.create(parent)?;
    }
    // The database contains transport private keys as well as user credentials.
    // Restrict existing sidecars too, before any key material can reach the WAL.
    for suffix in ["", "-wal"] {
        let mut file_path = path.as_os_str().to_os_string();
        file_path.push(suffix);
        match std::fs::symlink_metadata(&file_path) {
            Ok(metadata) => {
                anyhow::ensure!(
                    metadata.is_file() && !metadata.file_type().is_symlink(),
                    "Database and sidecars must be regular files"
                );
                #[cfg(unix)]
                {
                    use std::os::unix::fs::PermissionsExt;
                    std::fs::set_permissions(&file_path, std::fs::Permissions::from_mode(0o600))?;
                }
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(error.into()),
        }
    }
    let mut options = std::fs::OpenOptions::new();
    options.create(true).append(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    options.open(path)?;
    let mut wal = path.as_os_str().to_os_string();
    wal.push("-wal");
    options.open(wal)?;
    Ok(())
}
