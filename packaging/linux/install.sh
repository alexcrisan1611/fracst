#!/bin/sh
# Install fracst from this folder for the current user (into ~/.local), or
# system-wide with PREFIX=/usr/local (run as root). Adds a menu entry and
# opens .kfr files with fracst. Undo with: ./install.sh --uninstall
set -eu

here=$(cd "$(dirname "$0")" && pwd)
prefix=${PREFIX:-$HOME/.local}
bin="$prefix/bin"
share=${XDG_DATA_HOME:-$HOME/.local/share}
[ "$prefix" = "$HOME/.local" ] || share="$prefix/share"

files="$bin/fracst $bin/fracst-cli $bin/fracst-server
$share/applications/fracst.desktop
$share/icons/hicolor/512x512/apps/fracst.png
$share/mime/packages/fracst.xml"

refresh() {
    command -v update-mime-database >/dev/null && update-mime-database "$share/mime" || true
    command -v update-desktop-database >/dev/null && update-desktop-database "$share/applications" || true
    command -v gtk-update-icon-cache >/dev/null && gtk-update-icon-cache -q -t "$share/icons/hicolor" || true
}

if [ "${1:-}" = "--uninstall" ]; then
    for f in $files; do rm -f "$f"; done
    refresh
    echo "fracst removed from $prefix"
    exit 0
fi

install -Dm755 "$here/fracst" "$bin/fracst"
install -Dm755 "$here/fracst-cli" "$bin/fracst-cli"
install -Dm755 "$here/fracst-server" "$bin/fracst-server"
install -Dm644 "$here/fracst.desktop" "$share/applications/fracst.desktop"
install -Dm644 "$here/fracst.png" "$share/icons/hicolor/512x512/apps/fracst.png"
install -Dm644 "$here/fracst.xml" "$share/mime/packages/fracst.xml"
refresh

echo "fracst installed in $bin"
case ":$PATH:" in
    *":$bin:"*) ;;
    *) echo "Add $bin to your PATH to run fracst-cli from a terminal." ;;
esac
