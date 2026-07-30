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

# stdout/stderr otherwise go to the real console (tty7), which isn't visible
# once 0xin takes over the display and isn't readable after the fact by a
# separate SSH session — a log file makes post-hoc debugging possible
# without needing eyes on the physical screen. Overwritten each run.
log_file="$HOME/.local/state/0xin-touch-test.log"
mkdir -p "$(dirname "$log_file")"
exec "$repo_dir/target/debug/0xin" > "$log_file" 2>&1
