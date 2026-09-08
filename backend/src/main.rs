use std::io::{self, BufRead};
use taskboard::{auth, config::Config, state::AppState};

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_writer(std::io::stderr)
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| {
                "taskboard=info,tower_http=info,rtn_mq=info,iroh=warn,iroh_relay=warn".into()
            }),
        )
        .init();
    let args: Vec<String> = std::env::args().collect();
    let config = Config::from_env()?;
    let issuing_code = args.get(1).map(String::as_str) == Some("issue-gateway-code");
    let replace_gateway = issuing_code && args.get(2).map(String::as_str) == Some("--replace");
    if issuing_code {
        anyhow::ensure!(
            args.len() == 2 || (replace_gateway && args.len() == 3),
            "Usage: taskboard issue-gateway-code [--replace]"
        );
    }
    if args.get(1).is_some_and(|command| {
        !matches!(
            command.as_str(),
            "seed" | "issue-gateway-code" | "invalidate-sessions" | "status"
        )
    }) {
        anyhow::bail!("Unknown command: {}", args[1]);
    }
    if args.get(1).map(String::as_str) == Some("seed") {
        let email = args
            .get(2)
            .filter(|_| args.len() <= 4)
            .ok_or_else(|| anyhow::anyhow!("Usage: taskboard seed EMAIL [NAME] < password-file"))?;
        let name = args.get(3).map(String::as_str).unwrap_or("First editor");
        eprintln!("Read first editor password from stdin (minimum 15 characters):");
        let mut password = String::new();
        io::stdin().lock().read_line(&mut password)?;
        let state = AppState::new(config).await?;
        auth::seed(&state, email, name, password.trim_end_matches(['\r', '\n']))
            .await
            .map_err(|e| anyhow::anyhow!(e.2))?;
        state.db.checkpoint().await?;
        println!(
            "Database seeded with the first editor and backend identity. Sign in with {email}."
        );
        println!("Run issue-gateway-code next to enroll the gateway.");
        return Ok(());
    }
    anyhow::ensure!(
        config.database.try_exists()?,
        "Database does not exist. Run taskboard seed EMAIL [NAME] with the password on stdin first."
    );
    let state = AppState::new(config).await?;
    if issuing_code {
        let code = taskboard::mesh::issue_gateway_code(&state, replace_gateway).await?;
        state.db.checkpoint().await?;
        if replace_gateway {
            eprintln!(
                "Previous gateway codes and certificates invalidated. Backend identity and application data preserved."
            );
        }
        println!("{code}");
        return Ok(());
    }
    if args.get(1).map(String::as_str) == Some("invalidate-sessions") {
        state.db.connect().await?.execute_batch("BEGIN IMMEDIATE; DELETE FROM sessions; DELETE FROM account_tokens; DELETE FROM discord_link_tokens; COMMIT;")
        .await?;
        println!("Sessions and outstanding account/link tokens invalidated.");
        return Ok(());
    }
    if args.get(1).map(String::as_str) == Some("status") {
        println!(
            "{}",
            serde_json::to_string_pretty(
                &state.storage().await.map_err(|e| anyhow::anyhow!(e.2))?
            )?
        );
        return Ok(());
    }
    let mesh = taskboard::mesh::backend(&state).await?;
    mesh.online(std::time::Duration::from_secs(30)).await?;
    let jobs = tokio::spawn(taskboard::jobs::run(state.clone()));
    let discord = state
        .config
        .discord_token
        .as_ref()
        .map(|_| tokio::spawn(taskboard::discord::run(state.clone())));
    let mut tunnel = tokio::spawn(taskboard::tunnel::run(mesh.clone(), state.clone()));
    tracing::info!(endpoint = %mesh.endpoint_id(), "Taskboard backend ready");
    let outcome = tokio::select! {
        _=shutdown()=>Ok(()),
        result=&mut tunnel=>match result{Ok(Ok(()))=>Err(anyhow::anyhow!("Taskboard tunnel stopped unexpectedly")),Ok(Err(error))=>Err(error),Err(error)=>Err(error.into())},
    };
    jobs.abort();
    let _ = jobs.await;
    if !tunnel.is_finished() {
        tunnel.abort();
        let _ = tunnel.await;
    }
    if let Some(discord) = discord {
        discord.abort();
        let _ = discord.await;
    }
    let _ = mesh
        .shutdown(rtn_mq::ShutdownMode::Drain {
            timeout: std::time::Duration::from_secs(10),
        })
        .await;
    state.db.checkpoint().await?;
    outcome
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
