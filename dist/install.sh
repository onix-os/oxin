#!/bin/sh
# Install a released 0xin build.
#
# Same layout as `make install` from source — PREFIX defaults to /usr/local (what
# Hyprland uses), DESTDIR is honoured — so a binary release and a source build put
# the same files in the same places.
set -eu

PREFIX="${PREFIX:-/usr/local}"
DESTDIR="${DESTDIR:-}"

dir=$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)

install -Dm755 "$dir/0xin" "$DESTDIR$PREFIX/bin/0xin"
install -Dm755 "$dir/0xinctl" "$DESTDIR$PREFIX/bin/0xinctl"
install -Dm644 "$dir/0xin.desktop" "$DESTDIR$PREFIX/share/wayland-sessions/0xin.desktop"

echo "0xin and 0xinctl installed to $DESTDIR$PREFIX/bin"
echo "session entry installed to $DESTDIR$PREFIX/share/wayland-sessions"
