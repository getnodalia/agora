//! `agora-server [--config FILE]`: run Ágora's server.

use std::path::PathBuf;
use std::process::ExitCode;

use agora_server::Config;
use tracing_subscriber::EnvFilter;

mod update_check;

const HELP: &str = "agora-server: Ágora's self-hostable server

USAGE:
    agora-server [--config FILE] [init-token | check-updates]

OPTIONS:
    init-token          Create a one-use setup token, valid for 15 minutes
    check-updates       Check the latest stable release (does not install it)
    -c, --config FILE   TOML configuration (see docs/deployment.md)
    -h, --help          Print this help
    -V, --version       Print the version

ENVIRONMENT:
    AGORA_CONFIG        Configuration file, when --config is not given
    AGORA_BIND          Address to listen on (default 127.0.0.1:7878)
    AGORA_DATABASE      SQLite URL (default sqlite://agora.db)
    AGORA_REGISTRATION  open, invite_only or closed
    AGORA_TRUST_PROXY   true behind a TLS reverse proxy
    RUST_LOG            Log filter (default info)
";

fn main() -> ExitCode {
    let mut config_path = std::env::var_os("AGORA_CONFIG").map(PathBuf::from);
    let mut args = std::env::args().skip(1);
    let mut init_token = false;
    let mut check_updates = false;
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "init-token" if !init_token => init_token = true,
            "check-updates" if !check_updates => check_updates = true,
            "-h" | "--help" => {
                print!("{HELP}");
                return ExitCode::SUCCESS;
            }
            "-V" | "--version" => {
                println!("agora-server {}", env!("CARGO_PKG_VERSION"));
                return ExitCode::SUCCESS;
            }
            "-c" | "--config" => match args.next() {
                Some(p) => config_path = Some(PathBuf::from(p)),
                None => {
                    eprintln!("--config needs a file");
                    return ExitCode::from(2);
                }
            },
            other => {
                eprintln!("unknown argument {other:?}; see --help");
                return ExitCode::from(2);
            }
        }
    }

    if init_token && check_updates {
        eprintln!("choose either init-token or check-updates");
        return ExitCode::from(2);
    }

    if check_updates {
        let runtime = match tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
        {
            Ok(r) => r,
            Err(e) => {
                eprintln!("agora-server: cannot start the runtime: {e}");
                return ExitCode::FAILURE;
            }
        };
        return match runtime.block_on(update_check::check()) {
            Ok(message) => {
                println!("{message}");
                ExitCode::SUCCESS
            }
            Err(e) => {
                eprintln!("agora-server: update check failed: {e}");
                ExitCode::FAILURE
            }
        };
    }

    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info")),
        )
        .init();

    let config = match Config::load(config_path.as_deref()) {
        Ok(c) => c,
        Err(e) => {
            eprintln!("agora-server: {e}");
            return ExitCode::from(2);
        }
    };
    let runtime = match tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
    {
        Ok(r) => r,
        Err(e) => {
            eprintln!("agora-server: cannot start the runtime: {e}");
            return ExitCode::FAILURE;
        }
    };
    let result = if init_token {
        runtime.block_on(create_init_token(config))
    } else {
        runtime.block_on(run(config))
    };
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("agora-server: {e}");
            ExitCode::FAILURE
        }
    }
}

async fn run(config: Config) -> Result<(), Box<dyn std::error::Error>> {
    let bind = config.bind;
    let registration = config.registration;
    let (router, state) = agora_server::build(config).await?;
    agora_server::spawn_housekeeping(state.clone());
    let listener = tokio::net::TcpListener::bind(bind).await?;
    tracing::info!(address = %listener.local_addr()?, ?registration, "Ágora server listening");
    agora_server::serve_state(listener, router, state.clone(), async {
        let _ = tokio::signal::ctrl_c().await;
        tracing::info!("shutting down");
    })
    .await?;
    state.store.close().await;
    Ok(())
}

/// This command needs access to the server's database. The plaintext token is printed once.
async fn create_init_token(config: Config) -> Result<(), Box<dyn std::error::Error>> {
    let store = agora_storage::Store::open(&config.database).await?;
    let token = agora_server::auth::random_token(32);
    let expires_at = agora_server::auth::now_ms().saturating_add(15 * 60 * 1_000);
    store
        .set_initialization_token(&agora_server::auth::token_hash(&token), expires_at)
        .await?;
    store.close().await;
    println!("Setup token (valid for 15 minutes; enter as the invitation code): {token}");
    Ok(())
}
