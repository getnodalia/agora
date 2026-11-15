# Running a server

Ágora's server is one binary and one SQLite file. Run it on loopback and let a reverse proxy
handle TLS. Nothing in Ágora talks to an outside service.

## Install

The `.deb` and `.rpm` packages install `/usr/bin/agora-server`, and the Linux tarball and the macOS
and Windows zips carry it too ([releases](https://github.com/getnodalia/agora/releases/latest)).
Or build it:

```sh
cargo build --release -p agora-server
install -m 755 target/release/agora-server /usr/local/bin/
```

## Configure

```sh
sudo useradd --system --home /var/lib/agora --create-home agora
sudo install -o agora -m 600 agora.example.toml /etc/agora.toml
```

Edit `/etc/agora.toml`:

```toml
name = "Our team"
bind = "127.0.0.1:7878"
database = "sqlite:///var/lib/agora/agora.db"
registration = "invite_only"   # the first account can always register
trust_proxy = true             # the proxy below terminates TLS
```

## TLS with Caddy

Caddy gets and renews a certificate by itself. `/etc/caddy/Caddyfile`:

```
chat.example.org {
    reverse_proxy 127.0.0.1:7878
}
```

WebSockets pass through `reverse_proxy` without extra settings. With nginx, forward
`Upgrade`/`Connection` headers on `/api/v1/ws` and set `X-Forwarded-For`.

## Run as a service

`/etc/systemd/system/agora.service`:

```ini
[Unit]
Description=Ágora chat server
After=network.target

[Service]
User=agora
ExecStart=/usr/local/bin/agora-server --config /etc/agora.toml
Restart=on-failure
NoNewPrivileges=true
ProtectSystem=strict
ReadWritePaths=/var/lib/agora
PrivateTmp=true

[Install]
WantedBy=multi-user.target
```

```sh
sudo systemctl enable --now agora
```

In the desktop app, the server is `https://chat.example.org`.

## Backups

The database is `agora.db` with its `-wal` file. Back up with SQLite's online backup, which is
safe while the server runs:

```sh
sqlite3 /var/lib/agora/agora.db ".backup /var/backups/agora-$(date +%F).db"
```

Backups hold every message in clear text: store them as carefully as the server.

## Upgrading

Stop the server, back up, replace the binary, start it. Migrations run on start and only move
forward.
