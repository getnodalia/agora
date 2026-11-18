#!/bin/sh
# Install the Ágora server from this folder on any Linux with systemd (Debian, Ubuntu, a Proxmox
# LXC, Fedora...). Run as root from the unpacked folder:
#
#   sudo ./install.sh              install or upgrade, then start the service
#   sudo ./install.sh --uninstall  remove the program and the service (keeps the data and config)
#
# It installs /usr/local/bin/agora-server, the configuration /etc/agora/agora.toml (kept if it
# already exists), the data folder /var/lib/agora owned by the `agora` system user, and the
# systemd service agora-server.
set -eu
here="$(cd "$(dirname "$0")" && pwd)"
bin=/usr/local/bin/agora-server
unit=/etc/systemd/system/agora-server.service
update_unit=/etc/systemd/system/agora-server-update.service
update_timer=/etc/systemd/system/agora-server-update.timer

if [ "$(id -u)" -ne 0 ]; then
  echo "Run it as root: sudo $0 $*" >&2
  exit 1
fi
systemd=0
[ -d /run/systemd/system ] && systemd=1

if [ "${1:-}" = "--uninstall" ]; then
  if [ "$systemd" = 1 ]; then
    systemctl disable --now agora-server-update.timer 2>/dev/null || true
    systemctl disable --now agora-server.service 2>/dev/null || true
  fi
  rm -f "$bin" "$bin.previous" "$unit" "$update_unit" "$update_timer"
  if [ "$systemd" = 1 ]; then systemctl daemon-reload; fi
  echo "Removed the Ágora server. Data in /var/lib/agora and /etc/agora/agora.toml were kept."
  exit 0
fi
if [ -n "${1:-}" ]; then
  sed -n '2,9p' "$0" >&2
  exit 2
fi

"$here/agora-server" --version >/dev/null
install -d -m 755 /usr/local/bin
install -m 755 "$here/agora-server" "$bin.new"
mv -f "$bin.new" "$bin"

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
install -d -o agora -g agora -m 750 /var/lib/agora
install -d -m 755 /etc/agora
if [ ! -e /etc/agora/agora.toml ]; then
  install -o root -g agora -m 640 "$here/agora.toml" /etc/agora/agora.toml
  echo "Wrote /etc/agora/agora.toml"
else
  echo "Kept your /etc/agora/agora.toml"
fi

install -d -m 755 "$(dirname "$unit")"
sed "s|/usr/bin/agora-server|$bin|" "$here/agora-server.service" >"$unit.new"
chmod 644 "$unit.new"
mv -f "$unit.new" "$unit"
sed "s|/usr/bin/agora-server|$bin|" "$here/agora-server-update.service" >"$update_unit.new"
chmod 644 "$update_unit.new"
mv -f "$update_unit.new" "$update_unit"
install -m 644 "$here/agora-server-update.timer" "$update_timer"

version="$("$bin" --version)"
if [ "$systemd" = 1 ]; then
  systemctl daemon-reload
  systemctl enable agora-server.service >/dev/null 2>&1
  systemctl restart agora-server.service
  # The daily self-update timer, enabled once: a timer someone disabled stays disabled.
  if [ ! -e /var/lib/agora/.update-timer-enabled ]; then
    systemctl enable --now agora-server-update.timer >/dev/null 2>&1 || true
    touch /var/lib/agora/.update-timer-enabled
  fi
  echo "Installed $version and started the agora-server service."
  echo "Check it:  curl http://127.0.0.1:7878/healthz   (answers ok)"
  echo "Logs:      journalctl -u agora-server -f"
else
  echo "Installed $version. systemd is not running here, so start it yourself:"
  echo "  su -s /bin/sh agora -c '$bin --config /etc/agora/agora.toml'"
fi
echo "Next: put a TLS proxy in front and set the address in /etc/agora/agora.toml (see README.md)."
