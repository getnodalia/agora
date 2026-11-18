//! Encrypted files shared in conversations. The server stores ciphertext it cannot read.

use agora_core::{ChannelId, UserId};

use crate::{Result, Store};

impl Store {
    /// Store an uploaded file's ciphertext. Returns its id.
    pub async fn insert_file(
        &self,
        channel: ChannelId,
        uploader: UserId,
        data: &[u8],
        now: i64,
    ) -> Result<i64> {
        let size = i64::try_from(data.len()).unwrap_or(i64::MAX);
        let id: i64 = sqlx::query_scalar(
            "INSERT INTO files (channel_id, uploader_id, size, created_at, data)
             VALUES (?, ?, ?, ?, ?) RETURNING id",
        )
        .bind(channel.0)
        .bind(uploader.0)
        .bind(size)
        .bind(now)
        .bind(data)
        .fetch_one(&self.pool)
        .await?;
        Ok(id)
    }

    /// The channel a file belongs to, so access can be checked before reading it.
    pub async fn file_channel(&self, id: i64) -> Result<Option<ChannelId>> {
        let channel: Option<i64> = sqlx::query_scalar("SELECT channel_id FROM files WHERE id = ?")
            .bind(id)
            .fetch_optional(&self.pool)
            .await?;
        Ok(channel.map(ChannelId))
    }

    /// A file's ciphertext.
    pub async fn file_data(&self, id: i64) -> Result<Option<Vec<u8>>> {
        Ok(sqlx::query_scalar("SELECT data FROM files WHERE id = ?")
            .bind(id)
            .fetch_optional(&self.pool)
            .await?)
    }
}
