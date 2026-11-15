//! A real server on a random local port, and real clients talking to it.

#![allow(dead_code, clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::time::Duration;

use agora_client::{connect, parse_server_url, Client, Realtime, RealtimeEvent};
use agora_protocol::{RegisterRequest, ServerEvent, User};
use agora_server::config::{Config, RegistrationPolicy};
use tokio::sync::{mpsc, oneshot};
use tokio::task::JoinHandle;

pub struct Server {
    pub url: String,
    stop: Option<oneshot::Sender<()>>,
    task: JoinHandle<()>,
    pub state: agora_server::AppState,
}

impl Server {
    pub async fn start(database: &str) -> Server {
        Self::start_with(Config { database: database.into(), ..Config::default() }, 0).await
    }

    pub async fn start_with(mut config: Config, port: u16) -> Server {
        config.bind = format!("127.0.0.1:{port}").parse().unwrap();
        // Tests log in and post far faster than a person.
        config.limits.auth_per_minute = 1_000;
        config.limits.writes_per_minute = 10_000;
        let (router, state) = agora_server::build(config).await.unwrap();
        let listener = tokio::net::TcpListener::bind(state.config.bind).await.unwrap();
        let url = format!("http://{}", listener.local_addr().unwrap());
        let (stop, stopped) = oneshot::channel::<()>();
        let serve_state = state.clone();
        let task = tokio::spawn(async move {
            agora_server::serve_state(listener, router, serve_state, async {
                let _ = stopped.await;
            })
            .await
            .unwrap();
        });
        Server { url, stop: Some(stop), task, state }
    }

    pub fn port(&self) -> u16 {
        self.url.rsplit(':').next().unwrap().parse().unwrap()
    }

    pub fn anonymous(&self) -> Client {
        Client::new(parse_server_url(&self.url).unwrap()).unwrap()
    }

    /// Stop accepting, drop every connection and close the database.
    pub async fn stop(mut self) {
        if let Some(stop) = self.stop.take() {
            let _ = stop.send(());
        }
        // Graceful shutdown waits for open sockets; they close once the hub tells them to.
        if tokio::time::timeout(std::time::Duration::from_secs(5), &mut self.task).await.is_err() {
            self.task.abort();
        }
        self.state.store.close().await;
    }
}

pub fn temp_db(name: &str) -> (std::path::PathBuf, String) {
    let dir = std::env::temp_dir().join(format!("agora-it-{name}-{}-{}", std::process::id(), agora_client::new_nonce()));
    std::fs::create_dir_all(&dir).unwrap();
    let url = format!("sqlite://{}", dir.join("agora.db").display());
    (dir, url)
}

pub async fn register(server: &Server, username: &str) -> (Client, User) {
    register_with_invite(server, username, None).await
}

pub async fn register_with_invite(server: &Server, username: &str, invite: Option<&str>) -> (Client, User) {
    let (client, session) = server
        .anonymous()
        .register(&RegisterRequest {
            username: username.into(),
            display_name: username.to_uppercase(),
            password: format!("{username}-test-password"),
            invite_code: invite.map(str::to_owned),
        })
        .await
        .unwrap();
    (client, session.user)
}

/// A live connection whose events can be awaited.
pub struct Live {
    pub conn: Realtime,
    pub rx: mpsc::UnboundedReceiver<RealtimeEvent>,
}

impl Live {
    pub fn open(client: &Client) -> Live {
        let (tx, rx) = mpsc::unbounded_channel();
        let conn = connect(client, move |e| {
            let _ = tx.send(e);
        });
        Live { conn, rx }
    }

    /// Wait for the first event that `pick` accepts, up to five seconds.
    pub async fn wait<T>(&mut self, mut pick: impl FnMut(&RealtimeEvent) -> Option<T>) -> T {
        let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
        loop {
            let event = tokio::time::timeout_at(deadline, self.rx.recv())
                .await
                .expect("timed out waiting for an event")
                .expect("connection task ended");
            if let Some(v) = pick(&event) {
                return v;
            }
        }
    }

    pub async fn ready(&mut self) -> bool {
        self.wait(|e| match e {
            RealtimeEvent::Connected { reconnected, .. } => Some(*reconnected),
            _ => None,
        })
        .await
    }

    pub async fn server_event<T>(&mut self, mut pick: impl FnMut(&ServerEvent) -> Option<T>) -> T {
        self.wait(|e| match e {
            RealtimeEvent::Event(ev) => pick(ev),
            _ => None,
        })
        .await
    }

    /// Assert that no event matching `pick` arrives within `wait`.
    pub async fn assert_none(&mut self, wait: Duration, mut pick: impl FnMut(&ServerEvent) -> bool) {
        let deadline = tokio::time::Instant::now() + wait;
        while let Ok(Some(event)) = tokio::time::timeout_at(deadline, self.rx.recv()).await {
            if let RealtimeEvent::Event(ev) = &event {
                assert!(!pick(ev), "unexpected event: {ev:?}");
            }
        }
    }
}

pub fn invite_only() -> Config {
    Config { registration: RegistrationPolicy::InviteOnly, database: "sqlite::memory:".into(), ..Config::default() }
}
