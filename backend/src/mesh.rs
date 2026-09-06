use crate::config::Config as TaskboardConfig;
use anyhow::{Context, ensure};
use rtn_mq::{
    Config, Identity, JoinOptions, MAX_JOIN_LIFETIME, MessagingEndpoint, Permission, ShutdownMode,
};
use std::{path::Path, time::Duration};
use taskboard_wire::{REQUEST_TOPIC, RESPONSE_TOPIC};

pub async fn backend(config: &TaskboardConfig) -> anyhow::Result<MessagingEndpoint> {
    let identity = load_or_create_identity(&config.rtn_identity)?;
    prepare_private_parent(&config.rtn_state)?;
    MessagingEndpoint::host_persistent(
        transport_config(config.rtn_relay_only),
        identity,
        vec![
            Permission::subscribe(REQUEST_TOPIC)?,
            Permission::publish(RESPONSE_TOPIC)?,
        ],
        &config.rtn_state,
    )
    .await
    .context("start persistent rtn-mq backend")
}

pub async fn issue_gateway_code(config: &TaskboardConfig) -> anyhow::Result<String> {
    let endpoint = backend(config).await?;
    endpoint
        .online(Duration::from_secs(30))
        .await
        .context("connect backend to an Iroh relay")?;
    let mut options = JoinOptions::new(vec![
        Permission::publish(REQUEST_TOPIC)?,
        Permission::subscribe(RESPONSE_TOPIC)?,
    ]);
    options.max_uses = 1;
    options.lifetime = MAX_JOIN_LIFETIME;
    options.certificate_lifetime = MAX_JOIN_LIFETIME;
    let encoded = endpoint.issue_join_code(options).await?.encode()?;
    endpoint.shutdown(ShutdownMode::Immediate).await?;
    Ok(encoded)
}

pub fn transport_config(relay_only: bool) -> Config {
    let mut config = Config::new();
    config.relay_only = relay_only;
    config.max_peers = 1;
    config.max_topics = 2;
    config
}

pub fn load_or_create_identity(path: &Path) -> anyhow::Result<Identity> {
    if path.exists() {
        return Identity::load(path).context("load rtn-mq identity");
    }
    prepare_private_parent(path)?;
    let identity = Identity::generate();
    identity.save(path).context("save rtn-mq identity")?;
    Ok(identity)
}

fn prepare_private_parent(path: &Path) -> anyhow::Result<()> {
    let parent = path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."));
    let existed = parent.exists();
    std::fs::create_dir_all(parent).context("create rtn-mq private directory")?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        if !existed {
            std::fs::set_permissions(parent, std::fs::Permissions::from_mode(0o700))?;
        }
        let metadata = std::fs::symlink_metadata(parent)?;
        ensure!(
            metadata.is_dir()
                && !metadata.file_type().is_symlink()
                && metadata.permissions().mode() & 0o077 == 0,
            "rtn-mq state directory must be a private (0700) real directory"
        );
    }
    Ok(())
}
