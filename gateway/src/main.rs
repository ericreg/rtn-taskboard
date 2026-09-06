use std::io::{Read, Write};

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "taskboard_gateway=info,tower_http=info".into()),
        )
        .init();
    let config = taskboard_gateway::Config::from_env()?;
    if std::env::args().nth(1).as_deref() == Some("healthcheck") {
        return healthcheck(&config.bind);
    }
    let (state, responses, code) = taskboard_gateway::connect(&config).await?;
    let mut response_task =
        tokio::spawn(taskboard_gateway::response_loop(responses, state.clone()));
    let reconnect_task = tokio::spawn(taskboard_gateway::reconnect_loop(
        state.endpoint().clone(),
        code,
    ));
    let listener = tokio::net::TcpListener::bind(&config.bind).await?;
    tracing::info!(address = %config.bind, endpoint = %state.endpoint().endpoint_id(), "Taskboard gateway ready");
    let server = axum::serve(
        listener,
        taskboard_gateway::router(state.clone(), config.frontend)
            .into_make_service_with_connect_info::<std::net::SocketAddr>(),
    )
    .with_graceful_shutdown(shutdown());
    let outcome = tokio::select! {
        result = server => result.map_err(anyhow::Error::from),
        result = &mut response_task => match result {
            Ok(Ok(())) => Err(anyhow::anyhow!("Gateway response loop stopped unexpectedly")),
            Ok(Err(error)) => Err(error),
            Err(error) => Err(error.into()),
        },
    };
    reconnect_task.abort();
    response_task.abort();
    let _ = state
        .endpoint()
        .shutdown(rtn_mq::ShutdownMode::Drain {
            timeout: std::time::Duration::from_secs(10),
        })
        .await;
    outcome
}

fn healthcheck(bind: &str) -> anyhow::Result<()> {
    let port = bind.rsplit(':').next().unwrap_or("8080");
    let address = format!("127.0.0.1:{port}").parse()?;
    let mut socket =
        std::net::TcpStream::connect_timeout(&address, std::time::Duration::from_secs(3))?;
    socket.set_read_timeout(Some(std::time::Duration::from_secs(3)))?;
    socket
        .write_all(b"GET /health/ready HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n")?;
    let mut response = [0u8; 128];
    let count = socket.read(&mut response)?;
    anyhow::ensure!(
        String::from_utf8_lossy(&response[..count]).starts_with("HTTP/1.1 200"),
        "Gateway is not ready"
    );
    Ok(())
}

async fn shutdown() {
    #[cfg(unix)]
    {
        let mut terminate =
            tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
                .expect("Install SIGTERM handler");
        tokio::select! {_ = tokio::signal::ctrl_c()=>{},_ = terminate.recv()=>{}}
    }
    #[cfg(not(unix))]
    {
        let _ = tokio::signal::ctrl_c().await;
    }
}
