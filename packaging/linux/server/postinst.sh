#!/bin/sh
# agora-server .deb/.rpm post-install: the user and folders, then the service. A first install
# enables and starts it (it listens on loopback until /etc/agora/agora.toml says otherwise); an
# upgrade restarts it if it was running. Without a running systemd (containers) only the files
# are set up.
set -eu
if ! getent passwd agora >/dev/null 2>&1; then
  nologin=/usr/sbin/nologin
  [ -x "$nologin" ] || nologin=/sbin/nologin
  [ -x "$nologin" ] || nologin=/bin/false
  if command -v useradd >/dev/null 2>&1; then
    useradd --system --user-group --home-dir /var/lib/agora --no-create-home \
      --shell "$nologin" --comment "Agora chat server" agora
  else
    adduser -S -D -H -h /var/lib/agora -s "$nologin" agora
  fi
fi
mkdir -p /var/lib/agora
chown agora:agora /var/lib/agora
chmod 750 /var/lib/agora
if [ -f /etc/agora/agora.toml ]; then
  chown root:agora /etc/agora/agora.toml
  chmod 640 /etc/agora/agora.toml
fi
if [ -d /run/systemd/system ]; then
  systemctl daemon-reload || true
  # deb: "configure" with the old version as $2 on upgrade; rpm: $1 is 1 on install, 2 on upgrade.
  case "${1:-}:${2:-}" in
    configure: | 1:)
      systemctl enable --now agora-server.service || true ;;
    *)
      systemctl try-restart agora-server.service || true ;;
  esac
  # The daily self-update timer, enabled once: a timer someone disabled stays disabled.
  if [ ! -e /var/lib/agora/.update-timer-enabled ]; then
    systemctl enable --now agora-server-update.timer || true
    touch /var/lib/agora/.update-timer-enabled
  fi
fi
exit 0
