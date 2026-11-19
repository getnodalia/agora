//! `agora-server [--config FILE]`: run Ágora's server.

use std::path::PathBuf;
use std::process::ExitCode;

use agora_server::Config;
use tracing_subscriber::EnvFilter;

mod self_update;
mod update_check;

const HELP: &str = "agora-server: Ágora's self-hostable server

USAGE:
    agora-server [--config FILE] [init-token | check-updates | self-update [--auto] | backup FILE |
                          reset-password USERNAME [--remove-two-step]]

OPTIONS:
    init-token          Create a one-use setup token, valid for 15 minutes
    check-updates       Check the latest stable release (does not install it)
    self-update         Install the latest stable release (as root): verify it, back up the
                        database, install, restart and check /healthz. With --auto (the
                        agora-server-update.timer) it does nothing if [updates] automatic = false
    backup FILE         Create a consistent SQLite snapshot at a new path
    reset-password USERNAME
                        Print a one-use code, valid for 1 hour, that lets that person choose
                        a new password from the sign-in screen (\"Forgot your password?\").
                        --remove-two-step also turns off their two-step verification
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
    let mut backup_path = None;
    let mut reset_user: Option<String> = None;
    let mut remove_two_step = false;
    let mut self_update = false;
    let mut auto = false;
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "init-token" if !init_token => init_token = true,
            "check-updates" if !check_updates => check_updates = true,
            "self-update" if !self_update => self_update = true,
            "--auto" if self_update && !auto => auto = true,
            "backup" if backup_path.is_none() => match args.next() {
                Some(p) => backup_path = Some(PathBuf::from(p)),
                None => {
                    eprintln!("backup needs a destination file");
                    return ExitCode::from(2);
                }
            },
            "reset-password" if reset_user.is_none() => match args.next() {
                Some(u) if !u.starts_with('-') => reset_user = Some(u),
                _ => {
                    eprintln!("reset-password needs a username");
                    return ExitCode::from(2);
                }
            },
            "--remove-two-step" if !remove_two_step => remove_two_step = true,
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

    if u8::from(init_token)
        + u8::from(check_updates)
        + u8::from(self_update)
        + u8::from(backup_path.is_some())
        + u8::from(reset_user.is_some())
        > 1
    {
        eprintln!(
            "choose only one of init-token, check-updates, self-update, backup or reset-password"
        );
        return ExitCode::from(2);
    }

    if remove_two_step && reset_user.is_none() {
        eprintln!("--remove-two-step belongs to reset-password");
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
    if self_update {
        return match runtime.block_on(self_update::run(&config, auto)) {
            Ok(message) => {
                println!("{message}");
                ExitCode::SUCCESS
            }
            Err(e) => {
                eprintln!("agora-server: self-update failed: {e}");
                ExitCode::FAILURE
            }
        };
    }
    let result: Result<(), Box<dyn std::error::Error>> = if let Some(path) = backup_path {
        match runtime.block_on(agora_storage::backup::create(&config.database, &path)) {
            Ok(()) => {
                println!("Backup created: {}", path.display());
                Ok(())
            }
            Err(e) => Err(Box::new(e)),
        }
    } else if let Some(user) = reset_user {
        runtime.block_on(create_reset_code(config, &user, remove_two_step))
    } else if init_token {
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

/// Issue a one-use code that lets a person who lost their password choose a new one. It needs
/// the server's database; the plaintext code is printed once.
async fn create_reset_code(
    config: Config,
    username: &str,
    remove_two_step: bool,
) -> Result<(), Box<dyn std::error::Error>> {
    use agora_server::recovery::{self, Purpose};
    const VALID_MS: i64 = 60 * 60 * 1_000;
    let store = agora_storage::Store::open(&config.database).await?;
    let Some(user) = store
        .user_id_by_username(&username.trim().to_lowercase())
        .await?
    else {
        store.close().await;
        return Err(format!("there is no account called {username:?}").into());
    };
    let code = recovery::new_code();
    let expires_at = agora_server::auth::now_ms().saturating_add(VALID_MS);
    store
        .set_reset_code(
            user,
            &recovery::hash(user, Purpose::Operator, &code),
            expires_at,
            remove_two_step,
        )
        .await?;
    store.close().await;
    println!("Reset code (valid for 1 hour, works once): {code}");
    println!(
        "Give it to them privately. They choose \"Forgot your password?\" on the sign-in screen."
    );
    if remove_two_step {
        println!("Their two-step verification is turned off when they use it.");
    }
    Ok(())
}
