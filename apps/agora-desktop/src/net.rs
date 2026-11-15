//! The network side of the app. Requests run on a Tokio runtime; each finished request sends a
//! [`Reply`] through a channel and wakes the UI, which applies it on the next frame. The window
//! thread never waits on the network.

use std::future::Future;
use std::sync::mpsc;

use agora_client::{Client, ClientError, Realtime, RealtimeEvent};
use agora_core::{ChannelId, MessageId, WorkspaceId};
use agora_protocol::{
    Channel, Invite, Member, Message, MessagePage, Reaction, SearchResults, ServerInfo,
    SessionResponse, Thread, User, Workspace,
};

pub type Result<T> = std::result::Result<T, ClientError>;

/// A finished request, or something the real-time connection reported.
#[derive(Debug)]
pub enum Reply {
    ServerInfo(String, Result<ServerInfo>),
    SignedIn(Result<(Client, SessionResponse)>),
    Restored(Result<(Client, User)>),
    Workspaces(Result<Vec<Workspace>>),
    /// A workspace was created or joined: select it.
    WorkspaceOpened(Result<Workspace>),
    Channels(WorkspaceId, Result<Vec<Channel>>),
    /// A channel was created, joined or opened as a conversation: select it.
    ChannelOpened(Result<Channel>),
    ChannelLeft(ChannelId, Result<()>),
    Members(WorkspaceId, Result<Vec<Member>>),
    /// A page of history. `before` is set for older pages; `after` for a catch-up.
    History {
        channel: ChannelId,
        before: Option<MessageId>,
        after: Option<MessageId>,
        result: Result<MessagePage>,
    },
    Posted {
        channel: ChannelId,
        nonce: String,
        result: Result<Message>,
    },
    Thread(Result<Thread>),
    Reacted(MessageId, Result<Vec<Reaction>>),
    Edited(Result<Message>),
    Invite(Result<Invite>),
    Search(String, Result<SearchResults>),
    /// A request whose only interesting outcome is an error (read markers, deletions).
    Done(Result<()>),
    LoggedOut,
    Realtime(RealtimeEvent),
}

pub struct Net {
    runtime: tokio::runtime::Runtime,
    tx: mpsc::Sender<Reply>,
    rx: mpsc::Receiver<Reply>,
    ctx: egui::Context,
}

impl Net {
    pub fn new(ctx: egui::Context) -> std::io::Result<Net> {
        let runtime = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(2)
            .thread_name("agora-net")
            .enable_all()
            .build()?;
        let (tx, rx) = mpsc::channel();
        Ok(Net {
            runtime,
            tx,
            rx,
            ctx,
        })
    }

    /// Run `request` on the runtime; its reply arrives through [`Net::poll`].
    pub fn run<F>(&self, request: F)
    where
        F: Future<Output = Reply> + Send + 'static,
    {
        let tx = self.tx.clone();
        let ctx = self.ctx.clone();
        self.runtime.spawn(async move {
            let reply = request.await;
            let _ = tx.send(reply);
            ctx.request_repaint();
        });
    }

    /// Open the real-time connection for `client`'s session.
    pub fn realtime(&self, client: &Client) -> Realtime {
        let _guard = self.runtime.enter();
        let tx = self.tx.clone();
        let ctx = self.ctx.clone();
        agora_client::connect(client, move |event| {
            let _ = tx.send(Reply::Realtime(event));
            ctx.request_repaint();
        })
    }

    /// The replies that arrived since the last call.
    pub fn poll(&self) -> Vec<Reply> {
        self.rx.try_iter().collect()
    }
}
