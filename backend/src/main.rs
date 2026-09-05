use std::io::{self, BufRead, Read, Write};
use taskboard::{auth, config::Config, state::AppState};

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt().with_env_filter(tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| "taskboard=info,tower_http=info".into())).init();
    let args: Vec<String> = std::env::args().collect();
    let config = Config::from_env()?;
    if args.get(1).map(String::as_str) == Some("healthcheck") {
        let port = config.bind.rsplit(':').next().unwrap_or("8080");
        let address = format!("127.0.0.1:{port}").parse()?;
        let mut socket = std::net::TcpStream::connect_timeout(&address,std::time::Duration::from_secs(3))?;
        socket.set_read_timeout(Some(std::time::Duration::from_secs(3)))?;
        socket.write_all(b"GET /health/ready HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n")?;
        let mut response = [0u8;128]; let count = socket.read(&mut response)?;
        anyhow::ensure!(String::from_utf8_lossy(&response[..count]).starts_with("HTTP/1.1 200"),"Application is not ready");
        return Ok(());
    }
    let state = AppState::new(config).await?;
    if args.get(1).map(String::as_str) == Some("bootstrap") {
        let email = args.get(2).ok_or_else(|| anyhow::anyhow!("Usage: taskboard bootstrap EMAIL [NAME] < password-file"))?;
        let name = args.get(3).map(String::as_str).unwrap_or("First editor");
        eprintln!("Read first editor password from stdin (minimum 15 characters):");
        let mut password = String::new(); io::stdin().lock().read_line(&mut password)?;
        auth::bootstrap(&state, email, name, password.trim_end_matches(['\r','\n'])).await.map_err(|e| anyhow::anyhow!(e.2))?;
        println!("Editor created. Sign in with {email}.");
        return Ok(());
    }
    if args.get(1).map(String::as_str) == Some("invalidate-sessions") {
        sqlx::query("DELETE FROM sessions; DELETE FROM account_tokens; DELETE FROM discord_link_tokens;").execute(&state.pool).await?;
        println!("Sessions and outstanding account/link tokens invalidated."); return Ok(());
    }
    if args.get(1).map(String::as_str) == Some("status") {
        println!("{}", serde_json::to_string_pretty(&state.storage().await.map_err(|e| anyhow::anyhow!(e.2))?)?); return Ok(());
    }
    let jobs = tokio::spawn(taskboard::jobs::run(state.clone()));
    let discord = state.config.discord_token.as_ref().map(|_| tokio::spawn(taskboard::discord::run(state.clone())));
    let listener = tokio::net::TcpListener::bind(&state.config.bind).await?;
    tracing::info!(address = %state.config.bind, "Taskboard ready");
    axum::serve(listener, taskboard::http::router(state.clone()).into_make_service_with_connect_info::<std::net::SocketAddr>())
        .with_graceful_shutdown(shutdown()).await?;
    jobs.abort();
    if let Some(discord) = discord { discord.abort(); }
    state.pool.close().await;
    Ok(())
}

async fn shutdown() {
    #[cfg(unix)] {
        let mut terminate=tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate()).expect("Install SIGTERM handler");
        tokio::select!{_ = tokio::signal::ctrl_c()=>{},_ = terminate.recv()=>{}}
    }
    #[cfg(not(unix))] { let _=tokio::signal::ctrl_c().await; }
}
