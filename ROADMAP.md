# Roadmap

Where Ágora is and what comes next. Only what is under "Done" exists; everything else is a plan.

## Done: first working version (0.1.0-dev)

- Server: accounts, sessions, workspaces with roles and invitations, public and private channels,
  direct messages, threads, reactions, mentions, edits and deletions, unread markers, search,
  real time over an authenticated WebSocket, limits. SQLite with migrations.
- Desktop app for Linux, macOS and Windows: the whole flow above in three panes, NDL, light, dark
  and high-contrast themes, Spanish and English, sessions kept between launches.
- End-to-end encryption of every channel and direct message with MLS (RFC 9420) through OpenMLS:
  per-device keys, the server keeps only ciphertext and the order, local search, device lists
  with fingerprints. Tests against a real server check that no text reaches the database.
- End-to-end tests of the MVP against a real server (two people, restarts, isolation, private
  channels, retries, invalid sessions).

## Next: usable by a small team

- First release with packages (AppImage, .deb, .rpm, DMG, Windows) and release notes.
- Notifications from the operating system, and the unread count on the app icon.
- Files and images, with size limits and stored outside the database.
- Account settings: change password and display name, sign out other sessions, delete account.
- Workspace administration in the app: roles, removing people, archiving channels.
- Global menu over D-Bus on Linux, like every Nodalia app.

## Later

- PostgreSQL as a second storage backend (the storage crate keeps SQL in one place for this).
- A web client from the same Rust code (WebAssembly), and mobile apps.
- Two-factor sign-in, single sign-on.
- Voice and video calls.
- Verified identities for end-to-end encryption (cross-signed devices, a safety number per
  person), so a server cannot add a device unnoticed; reading old messages on a new device by
  handing keys over from another of one's own devices; reactions encrypted too.
- An independent review of how Ágora uses MLS.
- Federation between servers.
