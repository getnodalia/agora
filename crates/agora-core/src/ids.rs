//! Typed identifiers. Each is an opaque 64-bit number; the newtypes stop a channel id from being
//! passed where a message id is expected.

use serde::{Deserialize, Serialize};

macro_rules! id {
    ($(#[$doc:meta])* $name:ident) => {
        $(#[$doc])*
        #[derive(
            Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize,
        )]
        #[serde(transparent)]
        pub struct $name(pub i64);

        impl std::fmt::Display for $name {
            fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                self.0.fmt(f)
            }
        }
    };
}

id!(
    /// A person's account on one server.
    UserId
);
id!(
    /// A workspace: the unit of isolation between organizations on one server.
    WorkspaceId
);
id!(
    /// A channel, private channel or direct conversation inside a workspace.
    ChannelId
);
id!(
    /// A message. Ids grow with time inside a server, so they double as a pagination cursor.
    MessageId
);
id!(
    /// An invitation to a workspace.
    InviteId
);

id!(
    /// A company or organization, which may contain several workspaces.
    OrganizationId
);
