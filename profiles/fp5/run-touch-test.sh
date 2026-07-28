#!/bin/sh
set -eu

script_dir=$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)
repo_dir=$(CDPATH= cd -- "$script_dir/../.." && pwd)

# A greetd session starts without a parent Wayland display. Clear any stale
# inherited display variables anyway so wlroots must choose DRM + libinput.
unset WAYLAND_DISPLAY DISPLAY
export XDG_CURRENT_DESKTOP=0xin
export XDG_CONFIG_HOME="$script_dir/config"
export WLR_BACKENDS=drm,libinput
export LIBSEAT_BACKEND=logind
export LD_LIBRARY_PATH="$repo_dir/.sysroot/usr/lib${LD_LIBRARY_PATH:+:$LD_LIBRARY_PATH}"

exec "$repo_dir/target/debug/0xin" sh -c '
    foot &
    exec wvkbd-mobintl --no-popup -H 300 -L 200
'
