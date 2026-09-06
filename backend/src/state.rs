use crate::{config::Config, error::{Error, Result}};
use serde::Serialize;
use sqlx::{sqlite::{SqliteAutoVacuum, SqliteConnectOptions, SqliteJournalMode, SqlitePoolOptions}, SqliteConnection, SqlitePool};
use std::{collections::HashMap, sync::{Arc, atomic::{AtomicBool, AtomicU64, Ordering}}, time::{Duration, Instant}};
use tokio::sync::{Mutex, Semaphore};

#[derive(Clone)]
pub struct AppState {
    pub pool: SqlitePool, pub config: Arc<Config>, pub writes: Arc<Mutex<()>>,
    pub hashes: Arc<Semaphore>, pub max_db_bytes: Arc<AtomicU64>, pub discord_connected: Arc<AtomicBool>,
    pub rates: Arc<Mutex<HashMap<String, (Instant, u32)>>>,
}
#[derive(Serialize)]
pub struct StorageStatus {
    pub database_bytes: u64, pub database_file_bytes: u64, pub wal_bytes: u64,
    pub image_bytes: i64, pub max_image_bytes: u64, pub limit_bytes: u64, pub content_blocked: bool,
    pub discord_configured: bool, pub discord_connected: bool, pub failed_deliveries: i64,
}
impl AppState {
    pub async fn new(config: Config) -> anyhow::Result<Self> {
        prepare_database(&config.database)?;
        let options = SqliteConnectOptions::new().filename(&config.database).create_if_missing(true)
            .journal_mode(SqliteJournalMode::Wal).foreign_keys(true).auto_vacuum(SqliteAutoVacuum::Full)
            .busy_timeout(Duration::from_secs(15));
        let pool = SqlitePoolOptions::new().max_connections(5).connect_with(options).await?;
        sqlx::migrate!("./migrations").run(&pool).await?;
        sqlx::query("INSERT OR IGNORE INTO app_settings(key,value) VALUES('max_db_bytes',?)").bind(config.max_db_bytes.to_string()).execute(&pool).await?;
        let limit: String = sqlx::query_scalar("SELECT value FROM app_settings WHERE key='max_db_bytes'").fetch_one(&pool).await?;
        Ok(Self { pool, config: Arc::new(config), writes: Arc::new(Mutex::new(())), hashes: Arc::new(Semaphore::new(2)), max_db_bytes: Arc::new(AtomicU64::new(limit.parse()?)), discord_connected: Arc::new(AtomicBool::new(false)), rates: Arc::new(Mutex::new(HashMap::new())) })
    }
    pub async fn check_capacity(&self, conn: &mut SqliteConnection) -> Result<()> {
        let limit = self.max_db_bytes.load(Ordering::Relaxed);
        if limit > 0 && database_size(conn).await? >= limit { return Err(Error::full()); }
        Ok(())
    }
    pub async fn check_result_size(&self, conn: &mut SqliteConnection) -> Result<()> {
        let limit = self.max_db_bytes.load(Ordering::Relaxed);
        if limit > 0 && database_size(conn).await? > limit { return Err(Error::full()); }
        Ok(())
    }
    pub async fn storage(&self) -> Result<StorageStatus> {
        let mut conn = self.pool.acquire().await?;
        let size = database_size(&mut conn).await?;
        drop(conn);
        let file_size = |p: std::path::PathBuf| async move { tokio::fs::metadata(p).await.map(|m| m.len()).unwrap_or(0) };
        let limit = self.max_db_bytes.load(Ordering::Relaxed);
        Ok(StorageStatus {
            database_bytes: size, database_file_bytes: file_size(self.config.database.clone()).await,
            wal_bytes: file_size(format!("{}-wal", self.config.database.display()).into()).await,
            image_bytes: sqlx::query_scalar("SELECT COALESCE(SUM(size),0) FROM attachments").fetch_one(&self.pool).await?,
            max_image_bytes: self.config.max_image_bytes,
            limit_bytes: limit, content_blocked: limit > 0 && size >= limit,
            discord_configured: self.config.discord_token.is_some(), discord_connected: self.discord_connected.load(Ordering::Relaxed),
            failed_deliveries: sqlx::query_scalar("SELECT COUNT(*) FROM notification_deliveries WHERE state='failed'").fetch_one(&self.pool).await?,
        })
    }
    pub async fn rate_limit(&self, key: String, allowed: u32) -> Result<()> {
        let mut rates = self.rates.lock().await;
        rates.retain(|_, (at, _)| at.elapsed() < Duration::from_secs(900));
        // Bound memory even when attackers rotate identifiers.
        if rates.len() >= 10000 && !rates.contains_key(&key) { return Err(Error(axum::http::StatusCode::TOO_MANY_REQUESTS, "rate_limit", "Please try again later.".into())); }
        let (_, count) = rates.entry(key).or_insert((Instant::now(), 0)); *count += 1;
        if *count > allowed { return Err(Error(axum::http::StatusCode::TOO_MANY_REQUESTS, "rate_limit", "Too many attempts. Try again in 15 minutes.".into())); }
        Ok(())
    }
}
fn prepare_database(path: &std::path::Path) -> anyhow::Result<()> {
    let mut directory = std::fs::DirBuilder::new();
    directory.recursive(true);
    #[cfg(unix)] {
        use std::os::unix::fs::DirBuilderExt;
        directory.mode(0o700);
    }
    if let Some(parent) = path.parent().filter(|p| !p.as_os_str().is_empty()) {
        directory.create(parent)?;
    }
    // The database now contains transport private keys as well as user credentials.
    // Restrict existing sidecars too, before any key material can reach the WAL.
    for suffix in ["", "-wal", "-shm"] {
        let mut file_path = path.as_os_str().to_os_string();
        file_path.push(suffix);
        match std::fs::symlink_metadata(&file_path) {
            Ok(metadata) => {
                anyhow::ensure!(metadata.is_file() && !metadata.file_type().is_symlink(), "Database and sidecars must be regular files");
                #[cfg(unix)] {
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
    #[cfg(unix)] {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    options.open(path)?;
    Ok(())
}
pub async fn database_size(conn: &mut SqliteConnection) -> Result<u64> {
    let pages: i64 = sqlx::query_scalar("PRAGMA page_count").fetch_one(&mut *conn).await?;
    let page_size: i64 = sqlx::query_scalar("PRAGMA page_size").fetch_one(&mut *conn).await?;
    Ok((pages * page_size) as u64)
}
