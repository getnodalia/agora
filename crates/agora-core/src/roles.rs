//! Roles in a workspace and what each may do.
//!
//! The model is deliberately small: four workspace roles and three channel kinds. Every check the
//! server makes goes through a function here, so the rules are tested in one place.

use serde::{Deserialize, Serialize};

/// A member's role in one workspace, from most to least privileged.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Role {
    /// Created the workspace; cannot be removed.
    Owner,
    /// Manages members, invitations and every channel.
    Admin,
    /// Reads public channels, creates channels, writes where they belong.
    Member,
    /// Sees only the channels they were added to.
    Guest,
}

impl Role {
    pub const fn as_str(self) -> &'static str {
        match self {
            Role::Owner => "owner",
            Role::Admin => "admin",
            Role::Member => "member",
            Role::Guest => "guest",
        }
    }

    pub fn parse(s: &str) -> Option<Role> {
        Some(match s {
            "owner" => Role::Owner,
            "admin" => Role::Admin,
            "member" => Role::Member,
            "guest" => Role::Guest,
            _ => return None,
        })
    }

    pub const fn is_admin(self) -> bool {
        matches!(self, Role::Owner | Role::Admin)
    }

    /// Invite people to the workspace.
    pub const fn can_invite(self) -> bool {
        self.is_admin()
    }

    /// Create public or private channels.
    pub const fn can_create_channel(self) -> bool {
        !matches!(self, Role::Guest)
    }

    /// Delete or hide anyone's message (moderation).
    pub const fn can_moderate(self) -> bool {
        self.is_admin()
    }
}

/// What kind of conversation a channel is.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ChannelKind {
    /// Every non-guest member of the workspace can read and join it.
    Public,
    /// Only its members can see that it exists.
    Private,
    /// A conversation between two people.
    Direct,
}

impl ChannelKind {
    pub const fn as_str(self) -> &'static str {
        match self {
            ChannelKind::Public => "public",
            ChannelKind::Private => "private",
            ChannelKind::Direct => "direct",
        }
    }

    pub fn parse(s: &str) -> Option<ChannelKind> {
        Some(match s {
            "public" => ChannelKind::Public,
            "private" => ChannelKind::Private,
            "direct" => ChannelKind::Direct,
            _ => return None,
        })
    }
}

/// Whether a workspace member with `role` may read a channel of `kind`, given whether they are a
/// member of that channel. Reading a channel also means receiving its live events.
pub const fn can_read(role: Role, kind: ChannelKind, channel_member: bool) -> bool {
    match kind {
        ChannelKind::Public => channel_member || !matches!(role, Role::Guest),
        ChannelKind::Private | ChannelKind::Direct => channel_member,
    }
}

/// Whether they may post. Archived channels are read-only for everyone.
pub const fn can_post(role: Role, kind: ChannelKind, channel_member: bool, archived: bool) -> bool {
    !archived && can_read(role, kind, channel_member)
}

/// Whether they may join a channel on their own (without being added).
pub const fn can_self_join(role: Role, kind: ChannelKind) -> bool {
    matches!(kind, ChannelKind::Public) && !matches!(role, Role::Guest)
}

/// Whether they may rename, change the topic of, or archive a channel.
pub const fn can_manage_channel(
    role: Role,
    kind: ChannelKind,
    channel_member: bool,
    created_it: bool,
) -> bool {
    match kind {
        ChannelKind::Direct => false,
        _ => role.is_admin() || (created_it && channel_member),
    }
}

/// Whether they may add someone else to a channel.
pub const fn can_add_members(role: Role, kind: ChannelKind, channel_member: bool) -> bool {
    match kind {
        ChannelKind::Direct => false,
        ChannelKind::Public => channel_member || role.is_admin(),
        ChannelKind::Private => channel_member && !matches!(role, Role::Guest),
    }
}

/// Whether they may edit a message: only its author, and only while it is not deleted.
pub fn can_edit_message(me: crate::UserId, author: crate::UserId, deleted: bool) -> bool {
    me == author && !deleted
}

/// Whether they may delete a message: its author, or a moderator who can read it.
pub fn can_delete_message(me: crate::UserId, author: crate::UserId, role: Role) -> bool {
    me == author || role.can_moderate()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::UserId;

    #[test]
    fn guests_only_see_channels_they_belong_to() {
        assert!(!can_read(Role::Guest, ChannelKind::Public, false));
        assert!(can_read(Role::Guest, ChannelKind::Public, true));
        assert!(can_read(Role::Member, ChannelKind::Public, false));
    }

    #[test]
    fn private_and_direct_need_membership_even_for_admins() {
        for role in [Role::Owner, Role::Admin, Role::Member, Role::Guest] {
            assert!(!can_read(role, ChannelKind::Private, false));
            assert!(!can_read(role, ChannelKind::Direct, false));
            assert!(can_read(role, ChannelKind::Private, true));
        }
    }

    #[test]
    fn archived_channels_are_read_only() {
        assert!(!can_post(Role::Owner, ChannelKind::Public, true, true));
        assert!(can_post(Role::Member, ChannelKind::Public, true, false));
    }

    #[test]
    fn only_public_channels_can_be_joined_freely() {
        assert!(can_self_join(Role::Member, ChannelKind::Public));
        assert!(!can_self_join(Role::Guest, ChannelKind::Public));
        assert!(!can_self_join(Role::Admin, ChannelKind::Private));
        assert!(!can_self_join(Role::Admin, ChannelKind::Direct));
    }

    #[test]
    fn messages_are_edited_by_their_author_only() {
        assert!(can_edit_message(UserId(1), UserId(1), false));
        assert!(!can_edit_message(UserId(2), UserId(1), false));
        assert!(!can_edit_message(UserId(1), UserId(1), true));
        assert!(can_delete_message(UserId(2), UserId(1), Role::Admin));
        assert!(!can_delete_message(UserId(2), UserId(1), Role::Member));
    }

    #[test]
    fn roles_round_trip_through_their_names() {
        for role in [Role::Owner, Role::Admin, Role::Member, Role::Guest] {
            assert_eq!(Role::parse(role.as_str()), Some(role));
        }
        for kind in [ChannelKind::Public, ChannelKind::Private, ChannelKind::Direct] {
            assert_eq!(ChannelKind::parse(kind.as_str()), Some(kind));
        }
        assert_eq!(Role::parse("root"), None);
    }
}
