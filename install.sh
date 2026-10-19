#!/bin/bash
# Builds and installs Douby Commander: the program, its translations, the
# launcher entry and the icon.
#
#   ./install.sh                 for the current user, into ~/.local
#   cargo build --release && sudo PREFIX=/usr/local ./install.sh --no-build
#                                for everyone
#   ./install.sh --uninstall     removes it again (same PREFIX)
#
# PREFIX and DESTDIR work as usual, packages use PREFIX=/usr.

set -euo pipefail
cd "$(dirname "$0")"

APP_ID=cz.douby.DoubyCommander
BINARY=doubycmd
PREFIX=${PREFIX:-$HOME/.local}
ROOT="${DESTDIR:-}$PREFIX"

BIN="$ROOT/bin/$BINARY"
DESKTOP="$ROOT/share/applications/$APP_ID.desktop"
ICON="$ROOT/share/icons/hicolor/scalable/apps/$APP_ID.svg"
LOCALE="$ROOT/share/locale"

build=true
uninstall=false
for arg in "$@"; do
    case "$arg" in
        --no-build) build=false ;;
        --uninstall) uninstall=true ;;
        *) echo "Unknown option: $arg" >&2; exit 1 ;;
    esac
done

refresh_caches() {
    # Only for a real installation, packages do this on their own.
    [ -z "${DESTDIR:-}" ] || return 0
    if command -v update-desktop-database >/dev/null; then
        update-desktop-database -q "$ROOT/share/applications" || true
    fi
    if command -v gtk-update-icon-cache >/dev/null && [ -f "$ROOT/share/icons/hicolor/index.theme" ]; then
        gtk-update-icon-cache -q -t "$ROOT/share/icons/hicolor" || true
    fi
}

if $uninstall; then
    rm -f "$BIN" "$DESKTOP" "$ICON"
    rm -f "$LOCALE"/*/LC_MESSAGES/"$BINARY".mo
    refresh_caches
    echo "Douby Commander removed from $ROOT"
    exit 0
fi

if $build; then
    cargo build --release --locked
fi
[ -x "target/release/$BINARY" ] || { echo "target/release/$BINARY is missing, build it first" >&2; exit 1; }

install -Dm755 "target/release/$BINARY" "$BIN"
# The launcher starts the program by its full path, so it works even
# without PREFIX/bin on the PATH.
install -d "$(dirname "$DESKTOP")"
entry=$(<"extras/linux/$APP_ID.desktop")
printf '%s\n' "${entry//Exec=$BINARY/Exec=$PREFIX/bin/$BINARY}" > "$DESKTOP"
chmod 644 "$DESKTOP"
install -Dm644 "extras/linux/$APP_ID.svg" "$ICON"
for po in po/*.po; do
    language=$(basename "$po" .po)
    install -d "$LOCALE/$language/LC_MESSAGES"
    msgfmt --check -o "$LOCALE/$language/LC_MESSAGES/$BINARY.mo" "$po"
done
refresh_caches

echo "Douby Commander installed into $ROOT"
case ":$PATH:" in
    *":$PREFIX/bin:"*) ;;
    *) echo "Note: $PREFIX/bin is not on your PATH; the launcher works, the doubycmd command does not." ;;
esac
