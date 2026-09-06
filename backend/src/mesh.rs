use crate::state::AppState;
use anyhow::Context;
use rtn_mq::{Config, JoinOptions, MAX_JOIN_LIFETIME, MessagingEndpoint, Permission, ShutdownMode};
use std::{sync::Arc, time::Duration};
use taskboard_wire::{REQUEST_TOPIC, RESPONSE_TOPIC};

mod storage;

pub async fn backend(state: &AppState) -> anyhow::Result<MessagingEndpoint> {
    backend_with_transport(state, transport_config(state.config.rtn_relay_only)).await
}

async fn backend_with_transport(
    state: &AppState,
    transport: Config,
) -> anyhow::Result<MessagingEndpoint> {
    let (identity, storage) = storage::DatabaseStorage::open(&state.db).await?;
    MessagingEndpoint::host_with_storage(
        transport,
        identity,
        vec![
            Permission::subscribe(REQUEST_TOPIC)?,
            Permission::publish(RESPONSE_TOPIC)?,
        ],
        Arc::new(storage),
    )
    .await
    .context("start persistent rtn-mq backend")
}

pub async fn issue_gateway_code(state: &AppState) -> anyhow::Result<String> {
    let endpoint = backend(state).await?;
    let result: anyhow::Result<String> = async {
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
        Ok(endpoint.issue_join_code(options).await?.encode()?)
    }
    .await;
    let shutdown = endpoint.shutdown(ShutdownMode::Immediate).await;
    let encoded = result?;
    shutdown?;
    Ok(encoded)
}

pub fn transport_config(relay_only: bool) -> Config {
    let mut config = Config::new();
    config.relay_only = relay_only;
    config.max_peers = 1;
    config.max_topics = 2;
    config
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{auth, config::Config as TaskboardConfig};
    use rtn_mq::{HostStorage, Identity, RelayMode};

    fn local_transport() -> Config {
        let mut config = transport_config(false);
        config.relay_mode = RelayMode::Disabled;
        config.bind_addr = Some("127.0.0.1:0".parse().unwrap());
        config
    }

    #[tokio::test]
    async fn turso_preserves_identity_grants_and_redeemed_membership_across_restarts() {
        let directory = tempfile::tempdir().unwrap();
        let config = TaskboardConfig {
            database: directory.path().join("taskboard.db"),
            base_url: "http://localhost:8080".into(),
            max_db_bytes: 0,
            max_image_bytes: 10 * 1024 * 1024,
            secure_cookies: false,
            discord_token: None,
            discord_guild: None,
            rtn_relay_only: false,
        };
        let state = AppState::new(config.clone()).await.unwrap();
        assert!(
            backend_with_transport(&state, local_transport())
                .await
                .is_err()
        );
        auth::seed(
            &state,
            "admin@example.test",
            "Admin",
            "a long test-only passphrase",
        )
        .await
        .unwrap();
        let socket = std::net::UdpSocket::bind("127.0.0.1:0").unwrap();
        let address = socket.local_addr().unwrap();
        drop(socket);
        let mut transport = local_transport();
        transport.bind_addr = Some(address);
        let host = backend_with_transport(&state, transport.clone())
            .await
            .unwrap();
        let host_id = host.endpoint_id();
        let mut options = JoinOptions::new(vec![Permission::publish(REQUEST_TOPIC).unwrap()]);
        options.max_uses = 1;
        let code = host.issue_join_code(options).await.unwrap();
        host.shutdown(ShutdownMode::Immediate).await.unwrap();
        drop(host);
        state.db.checkpoint().await.unwrap();
        drop(state);
        let state = AppState::new(config.clone()).await.unwrap();

        // The issued, unredeemed grant survives a complete database/host restart.
        let host = backend_with_transport(&state, transport.clone())
            .await
            .unwrap();
        assert_eq!(host.endpoint_id(), host_id);
        let gateway_identity = Identity::generate();
        let gateway = MessagingEndpoint::join(local_transport(), gateway_identity.clone(), &code)
            .await
            .unwrap();
        gateway.shutdown(ShutdownMode::Immediate).await.unwrap();
        host.shutdown(ShutdownMode::Immediate).await.unwrap();
        drop(gateway);
        drop(host);

        state.db.checkpoint().await.unwrap();
        drop(state);
        let state = AppState::new(config.clone()).await.unwrap();

        // The original gateway can resume; a different key cannot consume the code again.
        let host = backend_with_transport(&state, transport).await.unwrap();
        assert_eq!(host.endpoint_id(), host_id);
        let gateway = MessagingEndpoint::join(local_transport(), gateway_identity, &code)
            .await
            .unwrap();
        assert!(matches!(
            MessagingEndpoint::join(local_transport(), Identity::generate(), &code).await,
            Err(rtn_mq::Error::QueueFull)
        ));
        gateway.shutdown(ShutdownMode::Immediate).await.unwrap();
        host.shutdown(ShutdownMode::Immediate).await.unwrap();
        drop(gateway);
        drop(host);

        // A stale writer must never undo another process's recorded enrollment.
        let (_, first) = storage::DatabaseStorage::open(&state.db).await.unwrap();
        let (_, stale) = storage::DatabaseStorage::open(&state.db).await.unwrap();
        let snapshot = first.load().unwrap().unwrap();
        first.save(&snapshot).unwrap();
        assert!(stale.save(&snapshot).is_err());
        assert!(!directory.path().join("rtn").exists());
    }
}
