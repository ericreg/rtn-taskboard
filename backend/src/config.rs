use anyhow::{Context, ensure};
use std::{env, path::PathBuf};

fn gib_to_bytes(value: &str) -> anyhow::Result<u64> {
    value.parse::<u64>()
        .context("TASKBOARD_MAX_DB_GIB must be a nonnegative whole number (0 means unlimited)")?
        .checked_mul(1024 * 1024 * 1024)
        .context("TASKBOARD_MAX_DB_GIB is too large")
}

#[derive(Clone)]
pub struct Config {
    pub database: PathBuf,
    pub bind: String,
    pub base_url: String,
    pub frontend: PathBuf,
    pub max_db_bytes: u64,
    pub secure_cookies: bool,
    pub discord_token: Option<String>,
    pub discord_guild: Option<u64>,
}
impl Config {
    pub fn from_env() -> anyhow::Result<Self> {
        let get = |k: &str, default: &str| env::var(k).unwrap_or_else(|_| default.into());
        let base_url = get("TASKBOARD_BASE_URL", "http://localhost:8080").trim_end_matches('/').to_string();
        let parsed = url::Url::parse(&base_url).context("Invalid TASKBOARD_BASE_URL")?;
        ensure!(matches!(parsed.scheme(), "http" | "https") && parsed.path() == "/" && parsed.query().is_none() && parsed.fragment().is_none(), "TASKBOARD_BASE_URL must be an http(s) origin without a path");
        let token = env::var("TASKBOARD_DISCORD_TOKEN").ok().filter(|s| !s.is_empty());
        let guild = env::var("TASKBOARD_DISCORD_GUILD_ID").ok().filter(|s| !s.is_empty()).map(|s| s.parse()).transpose().context("Invalid Discord server ID")?;
        ensure!(token.is_none() || guild.is_some(), "TASKBOARD_DISCORD_GUILD_ID is required with a bot token");
        let max_db_bytes = gib_to_bytes(&get("TASKBOARD_MAX_DB_GIB", "0"))?;
        Ok(Self {
            database: get("TASKBOARD_DATABASE", "data/taskboard.db").into(),
            bind: get("TASKBOARD_BIND", "0.0.0.0:8080"),
            frontend: get("TASKBOARD_FRONTEND", "frontend/dist").into(),
            max_db_bytes,
            secure_cookies: get("TASKBOARD_SECURE_COOKIES", if parsed.scheme() == "https" { "true" } else { "false" }).parse().context("Invalid TASKBOARD_SECURE_COOKIES")?,
            base_url,
            discord_token: token,
            discord_guild: guild,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::gib_to_bytes;

    #[test]
    fn converts_gibibytes_and_rejects_invalid_values() {
        assert_eq!(gib_to_bytes("0").unwrap(), 0);
        assert_eq!(gib_to_bytes("2").unwrap(), 2_147_483_648);
        assert!(gib_to_bytes("1.5").is_err());
        assert!(gib_to_bytes("18446744073709551615").is_err());
    }
}
