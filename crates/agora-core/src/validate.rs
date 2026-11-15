//! Input validation. Everything that arrives from a client passes through one of these functions
//! before it is stored, and each returns the normalised value that is stored.

use crate::limits::*;

#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
pub enum ValidationError {
    #[error("{field} is empty")]
    Empty { field: &'static str },
    #[error("{field} is too long (at most {max} characters)")]
    TooLong { field: &'static str, max: usize },
    #[error("{field} is too short (at least {min} characters)")]
    TooShort { field: &'static str, min: usize },
    #[error("{field} contains characters that are not allowed")]
    BadCharacters { field: &'static str },
}

impl ValidationError {
    pub fn field(&self) -> &'static str {
        match self {
            Self::Empty { field }
            | Self::TooLong { field, .. }
            | Self::TooShort { field, .. }
            | Self::BadCharacters { field } => field,
        }
    }
}

fn chars(s: &str) -> usize {
    s.chars().count()
}

/// Control characters other than newline and tab never belong in user text: they can reorder or
/// hide what other people see (bidirectional overrides are included).
fn is_forbidden_control(c: char) -> bool {
    (c.is_control() && c != '\n' && c != '\t')
        || matches!(c, '\u{202A}'..='\u{202E}' | '\u{2066}'..='\u{2069}')
}

/// A single-line name: trimmed, no control characters, no newlines.
fn line(field: &'static str, value: &str, max: usize) -> Result<String, ValidationError> {
    let v = value.trim();
    if v.is_empty() {
        return Err(ValidationError::Empty { field });
    }
    if chars(v) > max {
        return Err(ValidationError::TooLong { field, max });
    }
    if v.chars().any(|c| c == '\n' || c == '\t' || is_forbidden_control(c)) {
        return Err(ValidationError::BadCharacters { field });
    }
    Ok(v.to_owned())
}

/// Usernames are lower case so `Ana` and `ana` cannot be two people.
pub fn username(value: &str) -> Result<String, ValidationError> {
    let field = "username";
    let v = value.trim().to_lowercase();
    if v.is_empty() {
        return Err(ValidationError::Empty { field });
    }
    if chars(&v) < USERNAME_MIN {
        return Err(ValidationError::TooShort { field, min: USERNAME_MIN });
    }
    if chars(&v) > USERNAME_MAX {
        return Err(ValidationError::TooLong { field, max: USERNAME_MAX });
    }
    if !v
        .chars()
        .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || matches!(c, '.' | '_' | '-'))
    {
        return Err(ValidationError::BadCharacters { field });
    }
    Ok(v)
}

pub fn display_name(value: &str) -> Result<String, ValidationError> {
    line("display_name", value, DISPLAY_NAME_MAX)
}

/// Passwords are not trimmed or normalised: what the person typed is what is hashed.
pub fn password(value: &str) -> Result<(), ValidationError> {
    let field = "password";
    let n = chars(value);
    if n < PASSWORD_MIN {
        return Err(ValidationError::TooShort { field, min: PASSWORD_MIN });
    }
    if n > PASSWORD_MAX {
        return Err(ValidationError::TooLong { field, max: PASSWORD_MAX });
    }
    Ok(())
}

pub fn workspace_name(value: &str) -> Result<String, ValidationError> {
    line("workspace_name", value, WORKSPACE_NAME_MAX)
}

/// Channel names are lower case words joined by `-`, in any script (`diseño`, `研究`).
pub fn channel_name(value: &str) -> Result<String, ValidationError> {
    let field = "channel_name";
    let v: String = value
        .trim()
        .trim_start_matches('#')
        .to_lowercase()
        .split_whitespace()
        .collect::<Vec<_>>()
        .join("-");
    if v.is_empty() {
        return Err(ValidationError::Empty { field });
    }
    if chars(&v) > CHANNEL_NAME_MAX {
        return Err(ValidationError::TooLong { field, max: CHANNEL_NAME_MAX });
    }
    if !v.chars().all(|c| c.is_alphanumeric() || matches!(c, '-' | '_' | '.')) {
        return Err(ValidationError::BadCharacters { field });
    }
    Ok(v)
}

/// Topics may be empty.
pub fn topic(value: &str) -> Result<String, ValidationError> {
    let v = value.trim();
    if v.is_empty() {
        return Ok(String::new());
    }
    line("topic", v, TOPIC_MAX)
}

/// A message body: line endings normalised to `\n`, trailing whitespace trimmed, not empty.
/// Leading spaces are kept because they matter inside code blocks.
pub fn message_body(value: &str) -> Result<String, ValidationError> {
    let field = "body";
    let v = value.replace("\r\n", "\n").replace('\r', "\n");
    let v = v.trim_end().trim_start_matches('\n');
    if v.trim().is_empty() {
        return Err(ValidationError::Empty { field });
    }
    if chars(v) > MESSAGE_MAX_CHARS {
        return Err(ValidationError::TooLong { field, max: MESSAGE_MAX_CHARS });
    }
    if v.chars().any(is_forbidden_control) {
        return Err(ValidationError::BadCharacters { field });
    }
    Ok(v.to_owned())
}

/// A reaction: one short run of non-space characters (an emoji with its modifiers).
pub fn reaction(value: &str) -> Result<String, ValidationError> {
    let field = "emoji";
    let v = value.trim();
    if v.is_empty() {
        return Err(ValidationError::Empty { field });
    }
    if chars(v) > REACTION_MAX_CHARS {
        return Err(ValidationError::TooLong { field, max: REACTION_MAX_CHARS });
    }
    if v.chars().any(|c| c.is_whitespace() || is_forbidden_control(c) || c.is_ascii_alphanumeric()) {
        return Err(ValidationError::BadCharacters { field });
    }
    Ok(v.to_owned())
}

/// The idempotency key a client picks for a message it sends.
pub fn nonce(value: &str) -> Result<String, ValidationError> {
    let field = "client_nonce";
    if value.is_empty() {
        return Err(ValidationError::Empty { field });
    }
    if value.len() > NONCE_MAX {
        return Err(ValidationError::TooLong { field, max: NONCE_MAX });
    }
    if !value.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_')) {
        return Err(ValidationError::BadCharacters { field });
    }
    Ok(value.to_owned())
}

pub fn search_query(value: &str) -> Result<String, ValidationError> {
    line("query", value, SEARCH_QUERY_MAX)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn usernames_are_lower_case_ascii() {
        assert_eq!(username("  Ana.Lopez ").as_deref(), Ok("ana.lopez"));
        assert!(username("ab").is_err());
        assert!(username("ana lópez").is_err());
        assert!(username("ana<script>").is_err());
    }

    #[test]
    fn channel_names_accept_any_script() {
        assert_eq!(channel_name("#Diseño Web").as_deref(), Ok("diseño-web"));
        assert_eq!(channel_name("研究").as_deref(), Ok("研究"));
        assert!(channel_name("a/b").is_err());
        assert!(channel_name("   ").is_err());
    }

    #[test]
    fn message_bodies_keep_code_indentation() {
        let body = message_body("```\n    indented\n```\r\n\r\n").unwrap();
        assert_eq!(body, "```\n    indented\n```");
        assert!(message_body(" \n\t ").is_err());
    }

    #[test]
    fn message_bodies_refuse_hidden_direction_overrides() {
        assert!(message_body("pay \u{202E}gnp.exe").is_err());
        assert!(message_body("bell\u{7}").is_err());
        assert!(message_body("tab\tand\nnewline").is_ok());
    }

    #[test]
    fn message_body_limit_counts_characters_not_bytes() {
        let ok = "ñ".repeat(MESSAGE_MAX_CHARS);
        assert!(message_body(&ok).is_ok());
        assert!(message_body(&format!("{ok}ñ")).is_err());
    }

    #[test]
    fn emoji_reactions() {
        assert!(reaction("👍").is_ok());
        assert!(reaction("👩🏽‍💻").is_ok());
        assert!(reaction("lol").is_err());
        assert!(reaction("").is_err());
    }

    #[test]
    fn nonces_are_short_ascii_tokens() {
        assert!(nonce("0f8e-12_ab").is_ok());
        assert!(nonce("with space").is_err());
        assert!(nonce(&"a".repeat(65)).is_err());
    }

    #[test]
    fn passwords_are_bounded() {
        assert!(password("short").is_err());
        assert!(password("long enough password").is_ok());
        assert!(password(&"x".repeat(PASSWORD_MAX + 1)).is_err());
    }
}
