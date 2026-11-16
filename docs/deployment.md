# Running a server

Ágora's server is one binary and one SQLite file. Run it on loopback and let a reverse proxy
handle TLS. Nothing in Ágora talks to an outside service.

## Install

Each [release](https://github.com/getnodalia/agora/releases/latest) has the server on its own,
apart from the desktop app, with no desktop libraries:

| File | For |
| --- | --- |
| `agora-server-<version>-linux-x86_64.tar.gz` | any Linux with systemd: unpack it and run `sudo ./install.sh` |
| `agora-server-<version>-linux-x86_64.deb` | Debian, Ubuntu, a Proxmox LXC: `sudo apt install ./agora-server-*.deb` |
| `agora-server-<version>-linux-x86_64.rpm` | Fedora, RHEL: `sudo dnf install ./agora-server-*.rpm` |

All three do the same: the program, the `agora` system user, the data folder `/var/lib/agora`,
the configuration `/etc/agora/agora.toml` (kept on upgrades) and the systemd service
`agora-server`, enabled and started on loopback. Check it with
`curl http://127.0.0.1:7878/healthz`, which answers `ok`. The tarball's `README.md` walks through
it, and `sudo ./install.sh --uninstall` removes the program and the service and keeps the data.

`releases/latest/download/agora-server-linux-x86_64.tar.gz` always points at the newest one.
The macOS and Windows downloads carry `agora-server` too. Or build it:

```sh
cargo build --release -p agora-server
```

## Proxmox

A small unprivileged container is enough: the Debian 12 template, 1 core, 512 MB of memory and
4 GB of disk, with a fixed address. In its console, as root:

```sh
apt update && apt install -y curl ca-certificates
curl -LO https://github.com/getnodalia/agora/releases/latest/download/agora-server-linux-x86_64.tar.gz
tar xzf agora-server-linux-x86_64.tar.gz && cd agora-server-*-linux-x86_64/ && ./install.sh
```

Then make it reachable with TLS as below. With the reverse proxy in another container or machine
(Nginx Proxy Manager, Traefik...), set `bind = "0.0.0.0:7878"` and `trust_proxy = true` in
`/etc/agora/agora.toml`, `systemctl restart agora-server`, point the proxy at
`http://<container IP>:7878` with WebSockets on, and let only the proxy reach that port.
Proxmox's own container backups cover the database.

## Configure

`/etc/agora/agora.toml`, then `sudo systemctl restart agora-server`:

```toml
name = "Our team"
bind = "127.0.0.1:7878"
database = "sqlite:///var/lib/agora/agora.db"
registration = "invite_only"   # subsequent accounts require invitations
trust_proxy = true             # the proxy below terminates TLS
```

## Initialize a new server

An empty installation needs a one-use token before the first account can register. On the server,
using the same configuration and the service account that owns the database, run:

```sh
sudo -u agora agora-server --config /etc/agora/agora.toml init-token
```

Enter the displayed token in the desktop registration screen's invitation code field within
15 minutes. Treat it like a password. Running the command again invalidates the previous token.
Once the first account is created, initialization cannot be repeated. Existing installations
with accounts are marked initialized during migration and do not need a token. Back up the SQLite
database before upgrading, and test the upgrade on a copy first.

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

The packages and `install.sh` install the unit
([`packaging/linux/server/agora-server.service`](../packaging/linux/server/agora-server.service)):
it runs as `agora`, can write only to `/var/lib/agora`, and restarts on failure.

```sh
sudo systemctl status agora-server
journalctl -u agora-server -f
```

In the desktop app, the server is `https://chat.example.org`.

## Backups

The database is `agora.db` with its `-wal` file. Back up with SQLite's online backup, which is
safe while the server runs:

```sh
sqlite3 /var/lib/agora/agora.db ".backup /var/backups/agora-$(date +%F).db"
```

Backups hold accounts, metadata (who talks in which conversation, and when) and the messages'
ciphertext: store them as carefully as the server.

The server can also create a consistent snapshot without a separate SQLite program. Use a
private directory writable by the service account; the destination must not already exist:

```sh
sudo -u agora agora-server --config /etc/agora/agora.toml backup /var/lib/agora/backup.db
```

The command checks the copy's integrity and does not apply migrations to the source. The copy
contains sensitive data and needs independent protection and a tested restore procedure.

## Upgrading

Back up, then install the new release the way you installed the old one (`./install.sh` again,
or the new package). Migrations run on start and only move forward.

To check whether a newer stable release exists without changing the installation, run
`agora-server check-updates`. This command queries the public GitHub releases API over HTTPS;
it does not start the server, access the database, download an executable or install updates.
It requires network access and can fail if GitHub is unreachable or rate-limits the request.
Automatic installation is not yet implemented.
