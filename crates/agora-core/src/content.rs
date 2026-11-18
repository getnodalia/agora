//! What an end-to-end encrypted message carries besides plain text: a file, or a copy of earlier
//! messages shared with someone who joined later.
//!
//! The server never sees these: they are the plaintext of an encrypted message. A message that
//! is not one of them is ordinary text in [`crate::format`].

use serde::{Deserialize, Serialize};

/// Starts a message that carries a file. The private-use character keeps typed text from
/// matching by accident (and a message that only looks like one still has to parse).
pub const FILE_PREFIX: &str = "\u{E000}agora-file:";
/// Starts a message that carries earlier messages for someone who joined later.
pub const HISTORY_PREFIX: &str = "\u{E000}agora-history:";

/// The longest file name kept, in characters.
pub const FILE_NAME_MAX: usize = 255;
/// The most messages one history share carries.
pub const HISTORY_MAX: usize = 100;

/// A file sent in a message: where the server keeps its ciphertext, and the key that opens it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Attachment {
    /// The id the server gave the encrypted upload.
    pub file: i64,
    /// The name it had on the sender's computer, without folders.
    pub name: String,
    /// Its size before encryption, in bytes.
    pub size: u64,
    /// Its media type, when the sender knew it (`image/png`), else empty.
    #[serde(default)]
    pub mime: String,
    /// The AES-256-GCM key, base64. Only devices that can read the message see it.
    pub key: String,
}

/// One message in a history share, as the sharer's device read it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SharedMessage {
    pub author: String,
    pub at: i64,
    pub body: String,
}

/// What a message's decrypted text is.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Content<'a> {
    Text(&'a str),
    File {
        attachment: Attachment,
        caption: &'a str,
    },
    History(Vec<SharedMessage>),
}

/// The text of a message carrying `attachment`, with an optional caption after it.
pub fn file_message(attachment: &Attachment, caption: &str) -> String {
    let json = serde_json::to_string(attachment).unwrap_or_default();
    if caption.is_empty() {
        format!("{FILE_PREFIX}{json}")
    } else {
        format!("{FILE_PREFIX}{json}\n{caption}")
    }
}

/// The text of a message sharing the newest of `messages` with people who joined later: at most
/// [`HISTORY_MAX`], and as many as fit in one message.
pub fn history_message(messages: &[SharedMessage]) -> String {
    let mut start = messages.len().saturating_sub(HISTORY_MAX);
    loop {
        let kept = messages.get(start..).unwrap_or_default();
        let text = format!(
            "{HISTORY_PREFIX}{}",
            serde_json::to_string(kept).unwrap_or_default()
        );
        if text.chars().count() <= crate::limits::MESSAGE_MAX_CHARS || start >= messages.len() {
            return text;
        }
        start += 1;
    }
}

/// Read what a decrypted message carries. Anything malformed is shown as the text it is.
pub fn parse(body: &str) -> Content<'_> {
    if let Some(rest) = body.strip_prefix(FILE_PREFIX) {
        let (json, caption) = rest.split_once('\n').unwrap_or((rest, ""));
        if let Ok(mut attachment) = serde_json::from_str::<Attachment>(json) {
            attachment.name = safe_file_name(&attachment.name);
            return Content::File {
                attachment,
                caption,
            };
        }
    } else if let Some(json) = body.strip_prefix(HISTORY_PREFIX) {
        if let Ok(mut messages) = serde_json::from_str::<Vec<SharedMessage>>(json) {
            messages.truncate(HISTORY_MAX);
            return Content::History(messages);
        }
    }
    Content::Text(body)
}

/// A short line for notifications and lists: the file's name, or the text itself.
pub fn preview(body: &str) -> String {
    match parse(body) {
        Content::Text(text) => text.to_owned(),
        Content::File {
            attachment,
            caption: "",
        } => format!("📎 {}", attachment.name),
        Content::File {
            attachment,
            caption,
        } => format!("📎 {} · {caption}", attachment.name),
        Content::History(messages) => format!("🕘 {}", messages.len()),
    }
}

/// A file name that is safe to show and to save: no folders, no control characters, no
/// leading dots, at most [`FILE_NAME_MAX`] characters, and never empty.
pub fn safe_file_name(name: &str) -> String {
    let base = name.rsplit(['/', '\\']).next().unwrap_or(name);
    let cleaned: String = base
        .chars()
        .filter(|c| !c.is_control() && !matches!(c, ':' | '*' | '?' | '"' | '<' | '>' | '|'))
        .take(FILE_NAME_MAX)
        .collect();
    let cleaned = cleaned.trim().trim_start_matches('.').trim();
    if cleaned.is_empty() {
        "file".to_owned()
    } else {
        cleaned.to_owned()
    }
}

/// A size for people: `840 B`, `12.4 KB`, `3.1 MB`.
pub fn human_size(bytes: u64) -> String {
    const UNITS: [&str; 4] = ["KB", "MB", "GB", "TB"];
    if bytes < 1000 {
        return format!("{bytes} B");
    }
    let mut value = bytes as f64 / 1000.0;
    let mut unit = 0;
    while value >= 1000.0 && unit + 1 < UNITS.len() {
        value /= 1000.0;
        unit += 1;
    }
    let label = UNITS.get(unit).copied().unwrap_or("TB");
    if value < 10.0 {
        format!("{value:.1} {label}")
    } else {
        format!("{value:.0} {label}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn attachment(name: &str) -> Attachment {
        Attachment {
            file: 7,
            name: name.into(),
            size: 1234,
            mime: "application/pdf".into(),
            key: "a2V5".into(),
        }
    }

    #[test]
    fn a_file_message_round_trips_with_its_caption() {
        let a = attachment("Presupuesto 2027.pdf");
        assert_eq!(
            parse(&file_message(&a, "Para el lunes")),
            Content::File {
                attachment: a.clone(),
                caption: "Para el lunes"
            }
        );
        assert_eq!(
            parse(&file_message(&a, "")),
            Content::File {
                attachment: a,
                caption: ""
            }
        );
    }

    #[test]
    fn typed_text_and_broken_envelopes_stay_text() {
        assert_eq!(parse("agora-file:{}"), Content::Text("agora-file:{}"));
        let broken = format!("{FILE_PREFIX}{{not json");
        assert_eq!(parse(&broken), Content::Text(&broken));
    }

    #[test]
    fn names_from_a_message_cannot_point_elsewhere() {
        assert_eq!(safe_file_name("../../etc/passwd"), "passwd");
        assert_eq!(safe_file_name("C:\\Users\\x\\.bashrc"), "bashrc");
        assert_eq!(safe_file_name("..."), "file");
        assert_eq!(safe_file_name("a\u{0}b\nc.txt"), "abc.txt");
        assert_eq!(
            safe_file_name(&"x".repeat(1000)).chars().count(),
            FILE_NAME_MAX
        );
        let text = file_message(&attachment("/tmp/../informe.pdf"), "");
        let parsed = parse(&text);
        assert!(
            matches!(parsed, Content::File { attachment, .. } if attachment.name == "informe.pdf")
        );
    }

    #[test]
    fn history_keeps_the_newest_messages() {
        let messages: Vec<SharedMessage> = (0..150)
            .map(|i| SharedMessage {
                author: "Ana".into(),
                at: i,
                body: format!("m{i}"),
            })
            .collect();
        let Content::History(kept) = parse(&history_message(&messages)) else {
            panic!("not history");
        };
        assert_eq!(kept.len(), HISTORY_MAX);
        assert_eq!(kept.first().map(|m| m.at), Some(50));
    }

    #[test]
    fn a_history_share_always_fits_in_one_message() {
        let messages: Vec<SharedMessage> = (0..80)
            .map(|i| SharedMessage {
                author: "Ana".into(),
                at: i,
                body: "x".repeat(500),
            })
            .collect();
        let text = history_message(&messages);
        assert!(crate::validate::message_body(&text).is_ok());
        let Content::History(kept) = parse(&text) else {
            panic!("not history");
        };
        assert_eq!(kept.last().map(|m| m.at), Some(79));
        assert!(kept.len() < 80);
    }

    #[test]
    fn sizes_read_naturally() {
        assert_eq!(human_size(840), "840 B");
        assert_eq!(human_size(12_400), "12 KB");
        assert_eq!(human_size(3_100_000), "3.1 MB");
    }
}
