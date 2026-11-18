//! Ágora's domain, shared by the server and every client.
//!
//! Nothing here does I/O: identifiers, input validation, the role and permission model, and the
//! plain-text message format (a small Markdown subset) that clients render without HTML.

pub mod content;
pub mod format;
pub mod ids;
pub mod limits;
pub mod roles;
pub mod validate;

pub use ids::{ChannelId, InviteId, MessageId, OrganizationId, UserId, WorkspaceId};
pub use roles::{ChannelKind, Role};
pub use validate::ValidationError;
