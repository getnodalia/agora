# Roadmap

Where Ágora is and what comes next. Only what is under "Done" exists; everything else is a plan.

## Done: first release (0.1.0)

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
- Packages for every release, built and started on clean systems by CI: AppImage, .deb, .rpm and
  a tarball for Linux, a universal DMG and a server zip for macOS, a portable zip for Windows.
  The Linux server ships on its own too (tarball with `install.sh`, `.deb`, `.rpm`), with a systemd
  service, its user and its configuration. Merging a version bump with its notes releases it.

## Done since 0.1.4

- Two-step verification (TOTP with recovery codes) and a security dialog with the account's
  devices; signing out and back in on the same computer keeps the same device and its history.
- Sent, delivered and read ticks, with who has read a message.
- Files of any kind, encrypted on the device with their own key (100 MB by default, stored in the
  database for now), sent by picking or dropping them.
- Sharing earlier messages with people who joined a conversation later, by choice of a member.
- The Linux global menu over D-Bus: Ágora's commands in the top bar of desktops that show one.

## Next: usable by a small team

- A Windows installer (MSI) and the app's own icon on `agora.exe`; an in-app updater.
- A container image for the server, and server packages for ARM (Raspberry Pi).
- Native notifications on macOS and Windows, and unread indicators on the app icon. Linux has an
  opt-in generic notification for live messages; desktop integration still needs platform testing.
- Image previews in messages, and files stored outside the database.
- Account settings: change password and display name, sign out other sessions, delete account.
- Workspace administration in the app: roles, removing people, archiving channels.

## Later

- PostgreSQL as a second storage backend (the storage crate keeps SQL in one place for this).
- A web client from the same Rust code (WebAssembly), and mobile apps.
- Single sign-on, and a server-wide requirement for two-step verification.
- Voice and video calls.
- Verified identities for end-to-end encryption (cross-signed devices, a safety number per
  person), so a server cannot add a device unnoticed; reading old messages on a new device by
  handing keys over from another of one's own devices; reactions encrypted too.
- An independent review of how Ágora uses MLS.
- Federation between servers.
