#!/bin/sh
# agora-server .deb/.rpm pre-remove: stop and disable the service on removal (not on upgrade).
# The data in /var/lib/agora and the configuration in /etc/agora stay.
case "${1:-}" in
  remove | purge | 0)
    if [ -d /run/systemd/system ]; then
      systemctl disable --now agora-server.service >/dev/null 2>&1 || true
    fi ;;
esac
exit 0
