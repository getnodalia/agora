//! Size limits shared by the server (which enforces them) and clients (which show them).

/// Longest message body, in Unicode scalar values.
pub const MESSAGE_MAX_CHARS: usize = 8_000;
/// Most messages one history request returns.
pub const HISTORY_PAGE_MAX: u32 = 200;
/// Default page size for history requests.
pub const HISTORY_PAGE_DEFAULT: u32 = 50;
/// Usernames: `3..=32` characters of `a-z`, `0-9`, `.`, `_`, `-`.
pub const USERNAME_MIN: usize = 3;
pub const USERNAME_MAX: usize = 32;
pub const DISPLAY_NAME_MAX: usize = 64;
/// Passwords: long enough to resist guessing, bounded so hashing cannot be used to exhaust the CPU.
pub const PASSWORD_MIN: usize = 10;
pub const PASSWORD_MAX: usize = 256;
pub const WORKSPACE_NAME_MAX: usize = 64;
pub const CHANNEL_NAME_MAX: usize = 80;
pub const TOPIC_MAX: usize = 250;
/// A reaction is one emoji, possibly with modifiers and joiners.
pub const REACTION_MAX_CHARS: usize = 16;
/// Client-chosen idempotency key for a message.
pub const NONCE_MAX: usize = 64;
/// Members a workspace can have before the server refuses new joins.
pub const WORKSPACE_MEMBERS_MAX: u32 = 10_000;
/// Largest search query, in characters.
pub const SEARCH_QUERY_MAX: usize = 200;
/// The largest file anyone can share, in bytes before encryption; a server may allow less.
pub const FILE_MAX_BYTES: u64 = 512 * 1024 * 1024;
