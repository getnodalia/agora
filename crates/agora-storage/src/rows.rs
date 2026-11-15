//! Converting rows into protocol types.

use agora_core::{ChannelKind, MessageId, Role, UserId};
use agora_protocol::User;
use sqlx::sqlite::SqliteRow;
use sqlx::Row;

use crate::{Result, StoreError};

pub(crate) fn user(row: &SqliteRow, prefix: &str) -> Result<User> {
    Ok(User {
        id: UserId(row.try_get(format!("{prefix}id").as_str())?),
        username: row.try_get(format!("{prefix}username").as_str())?,
        display_name: row.try_get(format!("{prefix}display_name").as_str())?,
    })
}

pub(crate) fn role(s: &str) -> Result<Role> {
    Role::parse(s).ok_or_else(|| StoreError::Corrupt(format!("unknown role {s:?}")))
}

pub(crate) fn kind(s: &str) -> Result<ChannelKind> {
    ChannelKind::parse(s).ok_or_else(|| StoreError::Corrupt(format!("unknown channel kind {s:?}")))
}

pub(crate) fn opt_message_id(v: Option<i64>) -> Option<MessageId> {
    v.map(MessageId)
}

pub(crate) fn count(v: i64) -> u32 {
    u32::try_from(v.max(0)).unwrap_or(u32::MAX)
}
