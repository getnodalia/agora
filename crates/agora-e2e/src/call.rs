//! Sealing a call's audio and video.
//!
//! Every device in a conversation derives the same key from the MLS group
//! ([`crate::Device::call_key`]) and seals each frame with AES-256-GCM. A sealed frame is
//! `[epoch: u64 BE][nonce: 12 bytes][ciphertext]`; the ciphertext holds `[sequence: u64 BE]`
//! and the media. The associated data binds the conversation, the sender and the kind of media,
//! so the server cannot pass one person's frame off as another's, and the sequence lets a
//! receiver drop a frame the server replays. A sender's sequence starts at the time in
//! microseconds and counts up, so a sender who rejoins carries on above where it was.

use std::collections::HashMap;

use aes_gcm::aead::{Aead, KeyInit, Payload};
use aes_gcm::{Aes256Gcm, Nonce};
use agora_core::{ChannelId, UserId};
use rand::RngCore;

/// The MLS exporter label for call keys.
pub const EXPORTER_LABEL: &str = "agora call v1";
const HEADER: usize = 8 + 12;
/// Keys a receiver keeps: the current epoch's and the one before, for frames in flight while
/// the group moves on.
const KEPT_KEYS: usize = 2;

/// The key for one epoch of a conversation's group.
#[derive(Clone, PartialEq, Eq)]
pub struct CallKey {
    pub epoch: u64,
    pub key: [u8; 32],
}

impl std::fmt::Debug for CallKey {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CallKey")
            .field("epoch", &self.epoch)
            .finish_non_exhaustive()
    }
}

/// Which stream a frame belongs to. The byte is part of the associated data.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Stream {
    Audio,
    Video,
}

impl Stream {
    fn byte(self) -> u8 {
        match self {
            Stream::Audio => 1,
            Stream::Video => 2,
        }
    }
}

/// Why a frame could not be opened. Receivers drop such frames.
#[derive(Debug, PartialEq, Eq, thiserror::Error)]
pub enum OpenError {
    #[error("the frame is malformed")]
    Malformed,
    #[error("no key for that epoch")]
    UnknownEpoch,
    #[error("the frame does not authenticate")]
    Forged,
    #[error("the frame is older than one already played")]
    Replayed,
}

fn aad(channel: ChannelId, sender: UserId, stream: Stream, epoch: u64) -> [u8; 26] {
    let mut out = [0u8; 26];
    out[..8].copy_from_slice(&channel.0.to_be_bytes());
    out[8..16].copy_from_slice(&sender.0.to_be_bytes());
    out[16] = stream.byte();
    out[17] = 1;
    out[18..].copy_from_slice(&epoch.to_be_bytes());
    out
}

/// Seals what this device sends in a call.
#[derive(Debug)]
pub struct Sealer {
    channel: ChannelId,
    me: UserId,
    key: CallKey,
    sequence: u64,
}

impl Sealer {
    pub fn new(channel: ChannelId, me: UserId, key: CallKey) -> Sealer {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |d| u64::try_from(d.as_micros()).unwrap_or(u64::MAX / 2));
        Sealer {
            channel,
            me,
            key,
            sequence: now,
        }
    }

    /// Start using the key of a newer epoch.
    pub fn rekey(&mut self, key: CallKey) {
        if key.epoch != self.key.epoch {
            self.key = key;
        }
    }

    pub fn epoch(&self) -> u64 {
        self.key.epoch
    }

    /// Seal one frame. `None` only if the cipher fails, which it does not for these sizes.
    pub fn seal(&mut self, stream: Stream, media: &[u8]) -> Option<Vec<u8>> {
        self.sequence = self.sequence.saturating_add(1);
        let mut nonce = [0u8; 12];
        rand::thread_rng().fill_bytes(&mut nonce);
        let mut plain = Vec::with_capacity(8 + media.len());
        plain.extend_from_slice(&self.sequence.to_be_bytes());
        plain.extend_from_slice(media);
        let cipher = Aes256Gcm::new_from_slice(&self.key.key).ok()?;
        let sealed = cipher
            .encrypt(
                Nonce::from_slice(&nonce),
                Payload {
                    msg: &plain,
                    aad: &aad(self.channel, self.me, stream, self.key.epoch),
                },
            )
            .ok()?;
        let mut out = Vec::with_capacity(HEADER + sealed.len());
        out.extend_from_slice(&self.key.epoch.to_be_bytes());
        out.extend_from_slice(&nonce);
        out.extend_from_slice(&sealed);
        Some(out)
    }
}

/// Opens what the others in a call send.
#[derive(Debug)]
pub struct Opener {
    channel: ChannelId,
    keys: Vec<CallKey>,
    last: HashMap<(UserId, Stream), u64>,
}

impl Opener {
    pub fn new(channel: ChannelId) -> Opener {
        Opener {
            channel,
            keys: Vec::new(),
            last: HashMap::new(),
        }
    }

    /// Learn the key of an epoch. The oldest is forgotten beyond [`KEPT_KEYS`].
    pub fn add_key(&mut self, key: CallKey) {
        if self.keys.iter().any(|k| k.epoch == key.epoch) {
            return;
        }
        self.keys.push(key);
        self.keys.sort_by_key(|k| k.epoch);
        while self.keys.len() > KEPT_KEYS {
            self.keys.remove(0);
        }
    }

    /// The epoch of a sealed frame, so the caller can derive a key it lacks.
    pub fn epoch_of(sealed: &[u8]) -> Option<u64> {
        let raw: [u8; 8] = sealed.get(..8)?.try_into().ok()?;
        Some(u64::from_be_bytes(raw))
    }

    pub fn open(
        &mut self,
        sender: UserId,
        stream: Stream,
        sealed: &[u8],
    ) -> Result<Vec<u8>, OpenError> {
        let epoch = Opener::epoch_of(sealed).ok_or(OpenError::Malformed)?;
        let nonce = sealed.get(8..HEADER).ok_or(OpenError::Malformed)?;
        let body = sealed.get(HEADER..).ok_or(OpenError::Malformed)?;
        let key = self
            .keys
            .iter()
            .find(|k| k.epoch == epoch)
            .ok_or(OpenError::UnknownEpoch)?;
        let cipher = Aes256Gcm::new_from_slice(&key.key).map_err(|_| OpenError::Forged)?;
        let plain = cipher
            .decrypt(
                Nonce::from_slice(nonce),
                Payload {
                    msg: body,
                    aad: &aad(self.channel, sender, stream, epoch),
                },
            )
            .map_err(|_| OpenError::Forged)?;
        let raw: [u8; 8] = plain
            .get(..8)
            .and_then(|b| b.try_into().ok())
            .ok_or(OpenError::Malformed)?;
        let sequence = u64::from_be_bytes(raw);
        let last = self.last.entry((sender, stream)).or_insert(0);
        if sequence <= *last {
            return Err(OpenError::Replayed);
        }
        *last = sequence;
        Ok(plain.get(8..).unwrap_or_default().to_vec())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key(epoch: u64, byte: u8) -> CallKey {
        CallKey {
            epoch,
            key: [byte; 32],
        }
    }

    #[test]
    fn frames_open_only_for_the_right_sender_stream_and_key() {
        let ch = ChannelId(3);
        let ana = UserId(1);
        let mut sealer = Sealer::new(ch, ana, key(5, 7));
        let mut opener = Opener::new(ch);
        let first = sealer.seal(Stream::Audio, b"hola").unwrap();
        assert_eq!(
            opener.open(ana, Stream::Audio, &first),
            Err(OpenError::UnknownEpoch)
        );
        opener.add_key(key(5, 7));
        assert_eq!(opener.open(ana, Stream::Audio, &first).unwrap(), b"hola");
        // Replayed by the server.
        assert_eq!(
            opener.open(ana, Stream::Audio, &first),
            Err(OpenError::Replayed)
        );

        let second = sealer.seal(Stream::Audio, b"adios").unwrap();
        // Passed off as someone else's, or as video.
        assert_eq!(
            opener.open(UserId(2), Stream::Audio, &second),
            Err(OpenError::Forged)
        );
        assert_eq!(
            opener.open(ana, Stream::Video, &second),
            Err(OpenError::Forged)
        );
        // Another conversation's opener cannot read it, even with the same key bytes.
        let mut elsewhere = Opener::new(ChannelId(4));
        elsewhere.add_key(key(5, 7));
        assert_eq!(
            elsewhere.open(ana, Stream::Audio, &second),
            Err(OpenError::Forged)
        );
        assert_eq!(opener.open(ana, Stream::Audio, &second).unwrap(), b"adios");

        // A tampered byte fails; so does a truncated frame.
        let mut third = sealer.seal(Stream::Audio, b"x").unwrap();
        if let Some(b) = third.last_mut() {
            *b ^= 1;
        }
        assert_eq!(
            opener.open(ana, Stream::Audio, &third),
            Err(OpenError::Forged)
        );
        assert_eq!(
            opener.open(ana, Stream::Audio, &third[..10]),
            Err(OpenError::Malformed)
        );
    }

    #[test]
    fn old_epochs_are_forgotten() {
        let mut opener = Opener::new(ChannelId(1));
        let mut sealer = Sealer::new(ChannelId(1), UserId(1), key(1, 1));
        let old = sealer.seal(Stream::Video, b"v").unwrap();
        for e in 1..=3 {
            opener.add_key(key(e, e as u8));
        }
        assert_eq!(
            opener.open(UserId(1), Stream::Video, &old),
            Err(OpenError::UnknownEpoch)
        );
        sealer.rekey(key(3, 3));
        let new = sealer.seal(Stream::Video, b"v").unwrap();
        assert_eq!(opener.open(UserId(1), Stream::Video, &new).unwrap(), b"v");
    }
}
