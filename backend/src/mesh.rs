use crate::state::AppState;
use anyhow::Context;
use rtn_mq::{Config, JoinOptions, MAX_JOIN_LIFETIME, MessagingEndpoint, Permission, ShutdownMode};
use std::{sync::Arc, time::Duration};
use taskboard_wire::{REQUEST_TOPIC, RESPONSE_TOPIC};

mod storage;

pub async fn backend(state: &AppState) -> anyhow::Result<MessagingEndpoint> {
    backend_with_transport(state, transport_config(state.config.rtn_relay_only), false).await
}

async fn backend_with_transport(
    state: &AppState,
    transport: Config,
    replace_existing: bool,
) -> anyhow::Result<MessagingEndpoint> {
    let (identity, storage) = if replace_existing {
        storage::DatabaseStorage::replacement(&state.db).await?
    } else {
        storage::DatabaseStorage::open(&state.db).await?
    };
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

pub async fn issue_gateway_code(
    state: &AppState,
    replace_existing: bool,
) -> anyhow::Result<String> {
    let endpoint = backend_with_transport(
        state,
        transport_config(state.config.rtn_relay_only),
        replace_existing,
    )
    .await?;
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
    async fn replacing_gateway_is_atomic_and_preserves_backend_identity_and_application_data() {
        let directory = tempfile::tempdir().unwrap();
        let state = AppState::new(TaskboardConfig {
            database: directory.path().join("taskboard.db"),
            base_url: "http://localhost:8080".into(),
            max_db_bytes: 0,
            max_image_bytes: 10 * 1024 * 1024,
            secure_cookies: false,
            discord_token: None,
            discord_guild: None,
            rtn_relay_only: false,
        })
        .await
        .unwrap();
        auth::seed(
            &state,
            "admin@example.test",
            "Admin",
            "a long test-only passphrase",
        )
        .await
        .unwrap();
        let connection = state.db.connect().await.unwrap();
        let user_before: String =
            crate::db::scalar(&connection, "SELECT password_hash FROM users", ())
                .await
                .unwrap();
        let snapshot = || async {
            let (identity, storage) = storage::DatabaseStorage::open(&state.db).await.unwrap();
            (identity.to_bytes(), storage.load().unwrap().unwrap())
        };
        let socket = std::net::UdpSocket::bind("127.0.0.1:0").unwrap();
        let mut transport = local_transport();
        transport.bind_addr = Some(socket.local_addr().unwrap());
        drop(socket);
        let host = backend_with_transport(&state, transport.clone(), false)
            .await
            .unwrap();
        let host_id = host.endpoint_id();
        let mut options = JoinOptions::new(vec![Permission::publish(REQUEST_TOPIC).unwrap()]);
        options.max_uses = 1;
        let old_code = host.issue_join_code(options.clone()).await.unwrap();
        let old_identity = Identity::generate();
        let gateway = MessagingEndpoint::join(local_transport(), old_identity.clone(), &old_code)
            .await
            .unwrap();
        gateway.shutdown(ShutdownMode::Immediate).await.unwrap();
        drop(gateway);
        host.shutdown(ShutdownMode::Immediate).await.unwrap();
        drop(host);
        let original = snapshot().await;

        // Binding/going online without successfully issuing a code must not
        // invalidate the existing enrollment authority.
        let replacement = backend_with_transport(&state, transport.clone(), true)
            .await
            .unwrap();
        assert_eq!(replacement.endpoint_id(), host_id);
        assert_eq!(snapshot().await, original);
        connection.execute_batch("CREATE TRIGGER fail_replacement BEFORE UPDATE ON rtn_identity BEGIN SELECT RAISE(FAIL, 'test replacement failure'); END;").await.unwrap();
        assert!(replacement.issue_join_code(options.clone()).await.is_err());
        assert_eq!(snapshot().await, original);
        replacement.shutdown(ShutdownMode::Immediate).await.unwrap();
        drop(replacement);
        connection
            .execute_batch("DROP TRIGGER fail_replacement;")
            .await
            .unwrap();

        // The old key still works after a failed replacement.
        let host = backend_with_transport(&state, transport.clone(), false)
            .await
            .unwrap();
        let gateway = MessagingEndpoint::join(local_transport(), old_identity.clone(), &old_code)
            .await
            .unwrap();
        gateway.shutdown(ShutdownMode::Immediate).await.unwrap();
        drop(gateway);
        host.shutdown(ShutdownMode::Immediate).await.unwrap();
        drop(host);

        let replacement = backend_with_transport(&state, transport.clone(), true)
            .await
            .unwrap();
        let new_code = replacement.issue_join_code(options).await.unwrap();
        replacement.shutdown(ShutdownMode::Immediate).await.unwrap();
        drop(replacement);
        state.db.checkpoint().await.unwrap();
        let updated = snapshot().await;
        assert_eq!(updated.0, original.0);
        assert_ne!(updated.1, original.1);
        assert_eq!(
            crate::db::scalar::<String>(&connection, "SELECT password_hash FROM users", ())
                .await
                .unwrap(),
            user_before
        );
        assert_eq!(
            crate::db::scalar::<i64>(&connection, "SELECT COUNT(*) FROM users", ())
                .await
                .unwrap(),
            1
        );

        // The replacement persists across a normal host restart and frees the
        // single member slot without accepting the old code again.
        let host = backend_with_transport(&state, transport.clone(), false)
            .await
            .unwrap();
        assert_eq!(host.endpoint_id(), host_id);
        assert!(matches!(
            MessagingEndpoint::join(local_transport(), old_identity, &old_code).await,
            Err(rtn_mq::Error::Unauthorized)
        ));
        let new_identity = Identity::generate();
        let gateway = MessagingEndpoint::join(local_transport(), new_identity.clone(), &new_code)
            .await
            .unwrap();
        gateway.shutdown(ShutdownMode::Immediate).await.unwrap();
        drop(gateway);
        host.shutdown(ShutdownMode::Immediate).await.unwrap();
        drop(host);
        let host = backend_with_transport(&state, transport, false)
            .await
            .unwrap();
        let gateway = MessagingEndpoint::join(local_transport(), new_identity, &new_code)
            .await
            .unwrap();
        gateway.shutdown(ShutdownMode::Immediate).await.unwrap();
        drop(gateway);
        host.shutdown(ShutdownMode::Immediate).await.unwrap();
        drop(host);
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
            backend_with_transport(&state, local_transport(), false)
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
        let host = backend_with_transport(&state, transport.clone(), false)
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
        let host = backend_with_transport(&state, transport.clone(), false)
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
        let host = backend_with_transport(&state, transport, false)
            .await
            .unwrap();
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
