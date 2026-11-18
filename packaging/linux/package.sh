#!/usr/bin/env bash
# Build and package Ágora for Linux x86_64 or aarch64:
#
#   $DIST/agora-<version>-linux-<arch>.AppImage        the desktop app, any distro with glibc >= the build host's
#   $DIST/agora-<version>-linux-<arch>.AppImage.zsync  delta updates (needs zsyncmake)
#   $DIST/agora-<version>-linux-<arch>.deb             the desktop app: Debian, Ubuntu, Mint...
#   $DIST/agora-<version>-linux-<arch>.rpm             the desktop app: Fedora, openSUSE, RHEL...
#   $DIST/agora-<version>-linux-<arch>.tar.gz          the desktop app in a plain bin/ + share/ tree
#   $DIST/agora-server-<version>-linux-<arch>.deb      the server, its systemd service and config
#   $DIST/agora-server-<version>-linux-<arch>.rpm      the same for Fedora, RHEL...
#   $DIST/agora-server-<version>-linux-<arch>.tar.gz   the server with install.sh (any systemd Linux)
#   $DIST/agora-server-linux-<arch>.tar.gz             the same, under a name .../latest/download/ finds
#
# The desktop packages install the app (agora), its desktop entry, AppStream metainfo and hicolor
# icons. The server packages (packaging/linux/server/) carry no desktop libraries, so they suit a
# headless machine or a Proxmox container.
#
# Usage: packaging/linux/package.sh [--skip-build] [--formats "appimage deb rpm tar server"]
#
# Needs: cargo; nfpm for deb/rpm (https://nfpm.goreleaser.com). The AppImage tooling is downloaded
# at verified pins into $CARGO_TARGET_DIR. Build on an old distro (CI: Ubuntu 22.04, glibc 2.35)
# so the binaries run on newer ones. Optional: desktop-file-validate, appstreamcli, zsyncmake.
set -euo pipefail
# shellcheck source=../env.sh
. "$(dirname "${BASH_SOURCE[0]}")/../env.sh"
HERE="$ROOT/packaging/linux"
# shellcheck source=verified-download.sh
. "$HERE/verified-download.sh"

SKIP_BUILD=0
FORMATS="appimage deb rpm tar server"
while [[ $# -gt 0 ]]; do
  case "$1" in
    --skip-build) SKIP_BUILD=1; shift ;;
    --formats) FORMATS="$2"; shift 2 ;;
    -h | --help) sed -n '2,18p' "$0"; exit 0 ;;
    *) echo "unknown argument: $1" >&2; exit 2 ;;
  esac
done

ARCH="$(uname -m)"
case "$ARCH" in
  x86_64) DEB_ARCH=amd64 ;;
  aarch64 | arm64) ARCH=aarch64; DEB_ARCH=arm64 ;;
  *) echo "unsupported architecture $ARCH" >&2; exit 2 ;;
esac
export AGORA_MAINTAINER="${AGORA_MAINTAINER:-Nodalia <https://github.com/getnodalia/agora/issues>}"
BASENAME="agora-$VERSION-linux-$ARCH"

echo "==> Ágora $VERSION for Linux $ARCH ($FORMATS)"

if [[ "$SKIP_BUILD" = 0 ]]; then
  (cd "$ROOT" && cargo build --release --locked -p agora-desktop -p agora-server --bins)
fi
BIN="$CARGO_TARGET_DIR/release"
WORK="$CARGO_TARGET_DIR/linux-package"
STAGE="$WORK/root"
rm -rf "$WORK"

# ---- stage an FHS tree (shared by every format) -------------------------------------------------
install -Dm755 "$BIN/agora" "$STAGE/usr/bin/agora"
install -Dm755 "$BIN/agora-server" "$STAGE/usr/bin/agora-server"
strip "$STAGE/usr/bin/agora" "$STAGE/usr/bin/agora-server" 2>/dev/null || true
install -Dm644 "$HERE/$APP_ID.desktop" "$STAGE/usr/share/applications/$APP_ID.desktop"
mkdir -p "$STAGE/usr/share/metainfo"
sed -e "s/@VERSION@/$VERSION/g" -e "s/@DATE@/$AGORA_BUILD_DATE/g" \
  "$HERE/$APP_ID.metainfo.xml.in" >"$STAGE/usr/share/metainfo/$APP_ID.metainfo.xml"
mkdir -p "$STAGE/usr/share/icons"
cp -R "$ROOT/assets/icons/hicolor" "$STAGE/usr/share/icons/"
mkdir -p "$STAGE/usr/share/doc/agora"
copy_docs "$STAGE/usr/share/doc/agora"

if command -v desktop-file-validate >/dev/null; then
  desktop-file-validate "$STAGE/usr/share/applications/$APP_ID.desktop"
fi
if command -v appstreamcli >/dev/null; then
  appstreamcli validate --no-net --explain "$STAGE/usr/share/metainfo/$APP_ID.metainfo.xml"
fi

has() { case " $FORMATS " in *" $1 "*) return 0 ;; *) return 1 ;; esac; }

# ---- .tar.gz ------------------------------------------------------------------------------------
if has tar; then
  mkdir -p "$WORK/tar"
  cp -R "$STAGE/usr" "$WORK/tar/$BASENAME"
  rm -f "$WORK/tar/$BASENAME/bin/agora-server"
  tar -C "$WORK/tar" -czf "$DIST/$BASENAME.tar.gz" "$BASENAME"
  echo "wrote $DIST/$BASENAME.tar.gz"
fi

# ---- .deb / .rpm --------------------------------------------------------------------------------
if has deb || has rpm; then
  command -v nfpm >/dev/null || { echo "error: nfpm not found (https://nfpm.goreleaser.com/install/)" >&2; exit 1; }
  export VERSION
  export NFPM_ARCH="$DEB_ARCH"
  # nfpm expands env vars in fields like `version` and `arch`, but not in `contents[].src`.
  for spec in nfpm.yaml server/nfpm.yaml; do
    sed -e "s|\${STAGE}|$STAGE|g" -e "s|@SERVER@|$HERE/server|g" \
      -e "s|@GLIBC_FLOOR@|${GLIBC_FLOOR:-2.35}|g" "$HERE/$spec" >"$WORK/${spec//\//-}"
  done
  SERVER_BASENAME="agora-server-$VERSION-linux-$ARCH"
  for fmt in deb rpm; do
    if has "$fmt"; then
      (cd "$ROOT" && nfpm package -f "$WORK/nfpm.yaml" -p "$fmt" -t "$DIST/$BASENAME.$fmt")
      if has server; then
        (cd "$ROOT" && nfpm package -f "$WORK/server-nfpm.yaml" -p "$fmt" -t "$DIST/$SERVER_BASENAME.$fmt")
      fi
    fi
  done
fi

# ---- server tarball ------------------------------------------------------------------------------
if has server; then
  SERVER_BASENAME="agora-server-$VERSION-linux-$ARCH"
  SDIR="$WORK/server/$SERVER_BASENAME"
  mkdir -p "$SDIR"
  install -m755 "$STAGE/usr/bin/agora-server" "$SDIR/agora-server"
  install -m755 "$HERE/server/install.sh" "$SDIR/install.sh"
  install -m644 "$HERE/server/agora.toml" "$HERE/server/agora-server.service" \
    "$HERE/server/agora-server-update.service" "$HERE/server/agora-server-update.timer" \
    "$HERE/server/README.md" "$ROOT/LICENSE-MIT" "$ROOT/LICENSE-APACHE" "$SDIR/"
  tar -C "$WORK/server" -czf "$DIST/$SERVER_BASENAME.tar.gz" "$SERVER_BASENAME"
  cp "$DIST/$SERVER_BASENAME.tar.gz" "$DIST/agora-server-linux-$ARCH.tar.gz"
  echo "wrote $DIST/$SERVER_BASENAME.tar.gz (and agora-server-linux-$ARCH.tar.gz)"
fi

# ---- AppImage -----------------------------------------------------------------------------------
if has appimage; then
  APPDIR="$WORK/Agora.AppDir"
  mkdir -p "$APPDIR/usr/bin"
  cp "$STAGE/usr/bin/agora" "$APPDIR/usr/bin/agora"
  cp -R "$STAGE/usr/share" "$APPDIR/usr/share"
  # Calls need ALSA, which the binary links at start-up and a bare system may lack. The image
  # carries one and AppRun uses it only when the system has none of its own, so a host's
  # sound server plugins keep matching the library they were built for.
  LDCONFIG="$(command -v ldconfig || echo /sbin/ldconfig)"
  LIBASOUND="$("$LDCONFIG" -p | awk '/libasound\.so\.2 / && /(x86-64|AArch64)/ { print $NF; exit }')"
  [[ -n "$LIBASOUND" ]] || { echo "libasound.so.2 not found: install libasound2 to build the AppImage" >&2; exit 1; }
  mkdir -p "$APPDIR/usr/lib"
  cp -L "$LIBASOUND" "$APPDIR/usr/lib/libasound.so.2"
  cat >"$APPDIR/AppRun" <<'APPRUN'
#!/bin/sh
here="$(dirname "$(readlink -f "$0")")"
if ! { command -v ldconfig >/dev/null 2>&1 && ldconfig -p 2>/dev/null | grep -q 'libasound\.so\.2 '; }; then
  export LD_LIBRARY_PATH="$here/usr/lib${LD_LIBRARY_PATH:+:$LD_LIBRARY_PATH}"
fi
exec "$here/usr/bin/agora" "$@"
APPRUN
  chmod +x "$APPDIR/AppRun"
  cp "$HERE/$APP_ID.desktop" "$APPDIR/$APP_ID.desktop"
  cp "$ROOT/assets/icons/hicolor/256x256/apps/$APP_ID.png" "$APPDIR/$APP_ID.png"
  ln -s "$APP_ID.png" "$APPDIR/.DirIcon"

  # Digests from the official GitHub release asset metadata:
  # https://github.com/AppImage/appimagetool/releases/tag/1.9.1
  # https://github.com/AppImage/type2-runtime/releases/tag/20251108
  # Keep both pins together: appimagetool otherwise downloads the mutable latest type2 runtime.
  TOOL_VERSION=1.9.1
  RUNTIME_VERSION=20251108
  case "$ARCH" in
    x86_64)
      TOOL_SHA256=ed4ce84f0d9caff66f50bcca6ff6f35aae54ce8135408b3fa33abfc3cb384eb0
      RUNTIME_SHA256=2fca8b443c92510f1483a883f60061ad09b46b978b2631c807cd873a47ec260d
      ;;
    aarch64)
      TOOL_SHA256=f0837e7448a0c1e4e650a93bb3e85802546e60654ef287576f46c71c126a9158
      RUNTIME_SHA256=00cbdfcf917cc6c0ff6d3347d59e0ca1f7f45a6df1a428a0d6d8a78664d87444
      ;;
  esac
  mkdir -p "$CARGO_TARGET_DIR"
  TOOL="${APPIMAGETOOL:-$CARGO_TARGET_DIR/appimagetool-$TOOL_VERSION-$ARCH.AppImage}"
  if [[ -n "${APPIMAGETOOL:-}" ]]; then
    verify_appimagetool_override "$TOOL" "$TOOL_SHA256" "$TOOL_VERSION" "$ARCH"
  else
    download_verified \
      "https://github.com/AppImage/appimagetool/releases/download/$TOOL_VERSION/appimagetool-$ARCH.AppImage" \
      "$TOOL" "$TOOL_SHA256" true
  fi
  RUNTIME="$CARGO_TARGET_DIR/type2-runtime-$RUNTIME_VERSION-$ARCH"
  download_verified \
    "https://github.com/AppImage/type2-runtime/releases/download/$RUNTIME_VERSION/runtime-$ARCH" \
    "$RUNTIME" "$RUNTIME_SHA256"
  # Absolute, because appimagetool runs in $DIST below.
  OUT="$(cd "$DIST" && pwd)/$BASENAME.AppImage"
  APPDIR="$(cd "$APPDIR" && pwd)"
  TOOL="$(cd "$(dirname "$TOOL")" && pwd)/$(basename "$TOOL")"
  RUNTIME="$(cd "$(dirname "$RUNTIME")" && pwd)/$(basename "$RUNTIME")"
  rm -f "$OUT.zsync"
  # Update information: AppImageUpdate fetches only the blocks that changed in a newer release.
  REPO="${GITHUB_REPOSITORY:-getnodalia/agora}"
  UPDATE_INFO="gh-releases-zsync|${REPO%%/*}|${REPO#*/}|latest|agora-*-linux-$ARCH.AppImage.zsync"
  # Extract-and-run works without FUSE (containers, CI); the output embeds the static runtime.
  (cd "$DIST" && ARCH="$ARCH" APPIMAGE_EXTRACT_AND_RUN=1 "$TOOL" --no-appstream \
    --runtime-file "$RUNTIME" -u "$UPDATE_INFO" "$APPDIR" "$OUT")
  echo "wrote $OUT"
  if [[ -s "$OUT.zsync" ]]; then
    echo "wrote $OUT.zsync"
  else
    warn "zsyncmake not found, so $OUT.zsync was not written; AppImage delta updates need it"
  fi
fi

"$STAGE/usr/bin/agora" --version
"$STAGE/usr/bin/agora-server" --version
echo "==> done"
ls -lh "$DIST"
