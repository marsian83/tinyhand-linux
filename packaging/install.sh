#!/bin/sh
# Install TinyHand for Linux from a release tarball (or from a built checkout).
#
#   ./install.sh                  install into ~/.local
#   sudo ./install.sh --system    install into /usr/local
#   ./install.sh --prefix DIR     install into DIR (bin/ and share/ under it)
#   ./install.sh --uninstall      remove it again (combine with --system or --prefix)
set -eu

usage() {
    sed -n '2,8s/^# \{0,1\}//p' "$0"
    exit "${1:-0}"
}

prefix="$HOME/.local"
action=install
while [ $# -gt 0 ]; do
    case "$1" in
        --system) prefix=/usr/local ;;
        --prefix) [ $# -ge 2 ] || usage 2; prefix="$2"; shift ;;
        --prefix=*) prefix="${1#--prefix=}" ;;
        --uninstall) action=uninstall ;;
        -h|--help) usage ;;
        *) echo "install.sh: unknown option $1" >&2; usage 2 ;;
    esac
    shift
done

bindir="$prefix/bin"
datadir="$prefix/share"
handsdir="$datadir/tinyhand/hands"
appsdir="$datadir/applications"
icondir="$datadir/icons/hicolor"

# Refresh the icon cache only where one already exists; creating a new one in
# ~/.local would go stale. (The .desktop file has no MimeType, so there is no
# desktop database to update.)
refresh_caches() {
    if [ -f "$icondir/icon-theme.cache" ] && command -v gtk-update-icon-cache >/dev/null 2>&1; then
        gtk-update-icon-cache -q -t "$icondir" 2>/dev/null || true
    fi
}

if [ "$action" = uninstall ]; then
    [ -x "$bindir/tinyhand" ] && "$bindir/tinyhand" off 2>/dev/null || true
    rm -f "$bindir/tinyhand" \
          "$bindir/tinyhand-settings" \
          "$appsdir/tinyhand.desktop" \
          "$appsdir/tinyhand-settings.desktop" \
          "$icondir/scalable/apps/tinyhand.svg" \
          "$icondir/512x512/apps/tinyhand.png"
    rm -rf "$handsdir"
    rmdir "$datadir/tinyhand" 2>/dev/null || true
    refresh_caches
    echo "TinyHand removed from $prefix"
    exit 0
fi

# Files sit next to this script in the tarball; in a checkout (packaging/) the binary
# is in target/release and the hands in assets/hands, one level up.
here=$(cd "$(dirname "$0")" && pwd)
first() {
    for p in "$@"; do
        if [ -e "$p" ]; then echo "$p"; return 0; fi
    done
    return 1
}
bin=$(first "$here/tinyhand" "$here/../target/release/tinyhand") ||
    { echo "install.sh: can't find the tinyhand binary (run cargo build --release)" >&2; exit 1; }
hands=$(first "$here/hands" "$here/../assets/hands") ||
    { echo "install.sh: can't find the hands" >&2; exit 1; }
desktop=$(first "$here/tinyhand.desktop") || desktop=
# The settings window is optional: it needs GTK 4 and libadwaita, and builds that leave it
# out (like the CI tarball) install just the pointer.
settings=$(first "$here/tinyhand-settings" "$here/../target/release/tinyhand-settings") || settings=
settings_desktop=$(first "$here/tinyhand-settings.desktop") || settings_desktop=
icons=$(first "$here/icons") || icons=

mkdir -p "$bindir" "$handsdir" 2>/dev/null ||
    { echo "install.sh: can't write to $prefix (use sudo for --system)" >&2; exit 1; }

install -m 755 "$bin" "$bindir/tinyhand"
if [ -n "$settings" ]; then
    install -m 755 "$settings" "$bindir/tinyhand-settings"
fi
rm -f "$handsdir"/hand_*.png
install -m 644 "$hands"/hand_*.png "$handsdir/"

if [ -n "$desktop" ]; then
    mkdir -p "$appsdir"
    # Use the full path so the menu entry works even if $bindir isn't on the session's PATH.
    sed "s|^Exec=tinyhand|Exec=$bindir/tinyhand|" "$desktop" > "$appsdir/tinyhand.desktop"
    chmod 644 "$appsdir/tinyhand.desktop"
fi
if [ -n "$settings" ] && [ -n "$settings_desktop" ]; then
    mkdir -p "$appsdir"
    sed "s|^Exec=tinyhand-settings|Exec=$bindir/tinyhand-settings|" "$settings_desktop" > "$appsdir/tinyhand-settings.desktop"
    chmod 644 "$appsdir/tinyhand-settings.desktop"
fi
if [ -n "$icons" ]; then
    install -D -m 644 "$icons/tinyhand.svg" "$icondir/scalable/apps/tinyhand.svg"
    install -D -m 644 "$icons/tinyhand.png" "$icondir/512x512/apps/tinyhand.png"
fi
refresh_caches

echo "TinyHand installed into $prefix"
case ":$PATH:" in
    *":$bindir:"*) ;;
    *) echo "Note: $bindir is not on your PATH. Add it, or use $bindir/tinyhand in your shortcuts." ;;
esac
echo "Next, bind 'tinyhand toggle' to a key (Ctrl+Alt+H) in your desktop's keyboard settings."
