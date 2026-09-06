use std::io::{self, BufRead};
use taskboard::{auth, config::Config, state::AppState};

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt().with_env_filter(tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| "taskboard=info,tower_http=info,rtn_mq=info,iroh=warn,iroh_relay=warn".into())).init();
    let args: Vec<String> = std::env::args().collect();
    let config = Config::from_env()?;
    if args.get(1).map(String::as_str) == Some("issue-gateway-code") {
        println!("{}",taskboard::mesh::issue_gateway_code(&config).await?);
        return Ok(());
    }
    if args.get(1).is_some_and(|command| !matches!(command.as_str(), "bootstrap" | "invalidate-sessions" | "status")) {
        anyhow::bail!("Unknown command: {}", args[1]);
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
    let mesh=taskboard::mesh::backend(&state.config).await?;
    mesh.online(std::time::Duration::from_secs(30)).await?;
    let mut tunnel=tokio::spawn(taskboard::tunnel::run(mesh.clone(),state.clone()));
    tracing::info!(endpoint = %mesh.endpoint_id(), "Taskboard backend ready");
    let outcome=tokio::select!{
        _=shutdown()=>Ok(()),
        result=&mut tunnel=>match result{Ok(Ok(()))=>Err(anyhow::anyhow!("Taskboard tunnel stopped unexpectedly")),Ok(Err(error))=>Err(error),Err(error)=>Err(error.into())},
    };
    jobs.abort();
    tunnel.abort();
    if let Some(discord) = discord { discord.abort(); }
    let _=mesh.shutdown(rtn_mq::ShutdownMode::Drain{timeout:std::time::Duration::from_secs(10)}).await;
    state.pool.close().await;
    outcome
}

async fn shutdown() {
    #[cfg(unix)] {
        let mut terminate=tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate()).expect("Install SIGTERM handler");
        tokio::select!{_ = tokio::signal::ctrl_c()=>{},_ = terminate.recv()=>{}}
    }
    #[cfg(not(unix))] { let _=tokio::signal::ctrl_c().await; }
}
