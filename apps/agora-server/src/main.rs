//! `agora-server [--config FILE]`: run Ágora's server.

use std::path::PathBuf;
use std::process::ExitCode;

use agora_server::Config;
use tracing_subscriber::EnvFilter;

const HELP: &str = "agora-server: Ágora's self-hostable server

USAGE:
    agora-server [--config FILE]

OPTIONS:
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
    while let Some(arg) = args.next() {
        match arg.as_str() {
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
    match runtime.block_on(run(config)) {
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
