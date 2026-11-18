# Nodalia migration: Ágora

Category: secondary app (new, built for Nodalia from the start).
Status: first working version on a pull request, 2026-11-15. Not released.

## Current state

Rust workspace: `agora-server` (Tokio, Axum, SQLx/SQLite) and `agora-desktop` (egui/eframe with
`nodalia-theme`), plus `agora-core`, `agora-protocol`, `agora-storage` and `agora-client`. CI calls
Foundation's `rust-gate`, `git-sources` and `agents-check` workflows, pinned by commit.

## Desired state

Released through Foundation's `release-tag`/`release-publish` with release notes, packages for
Linux, macOS (DMG with its own install window) and Windows, the shared in-app updater
(`nodalia-update`), and the Linux global menu over D-Bus.

## Inherited dependencies still pending

None: Ágora has no upstream.

## Reusable Nodalia components

- `nodalia-theme` (egui, `creator` feature): palette with `Accent::Agora` (`#0F7C80`), islands,
  buttons, fields, window controls, icons.
- `tools/app-portrait` in nodalia-design made the icon.

## NDL work pending

- `creator::search_field` draws its text at the top of a tall field; Ágora draws its own centred
  copy (`widgets::search_field`) until nodalia-design centres it.
- An emoji picker and a chat message list are drawn in Ágora; if another app needs them, they move
  to nodalia-design.
- Done: the Linux global menu over D-Bus (`linux_menu.rs`, as Toma and Pluma do). Ágora has no
  in-window menu bar, only NDL's hamburger menu, which stays on every desktop.

## Missing tests

- Desktop app: the model reducer, settings and translations have unit tests; the views are checked
  by hand with screenshots. A headless render test of the main screens would catch layout
  regressions.
