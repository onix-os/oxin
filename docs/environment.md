# Environment & toolchain

0xin is built and run on **Arch Linux**, as ordinary stable-Rust userspace
(no `no_std`, no custom target) — the toolchain is pinned in
`rust-toolchain.toml` so a fresh checkout always builds with the exact
version it was developed against.

## System dependencies

```
wlroots0.19 wayland wayland-protocols libxkbcommon libinput libdrm seatd mesa pixman pkgconf clang
```

The version that matters most is **wlroots**: 0xin targets wlroots **0.19**
specifically (Arch package `wlroots0.19`, pkg-config name `wlroots-0.19`).
wlroots' API moves between minor versions, so this pin isn't cosmetic —
`build.rs` resolves flags via `pkg-config wlroots-0.19` rather than a bare
`wlroots`, and every wlroots header requires `-DWLR_USE_UNSTABLE` defined or
it expands to `#error` (wlroots treats most of its own API as unstable by
design; the flag is an explicit "I know" acknowledgement, not a mistake to
work around).

`clang`/`libclang` is a build dependency, not a runtime one — `bindgen` needs
it to parse the wlroots C headers into Rust FFI declarations.

A [direnv](https://direnv.net) `.envrc` is committed (run `direnv allow`
once after cloning, if you use direnv — it's optional). It turns backtraces
on (`RUST_BACKTRACE=1`) and, on a machine that has Nix, enters the flake's
devshell — the check is guarded, so the file stays a no-op here on Arch. It's
also the designated place for `PKG_CONFIG_PATH`/`LD_LIBRARY_PATH` if we ever
pin our own wlroots build instead of the Arch package. Per-scenario variables (`WLR_BACKENDS`,
`WLR_WL_OUTPUTS`, `OXIN_MOD`, …) deliberately stay on the command line —
they select a run mode and don't belong in ambient env.

## Building with Nix

Arch is where 0xin is developed, but it isn't the only way to build it. A `flake.nix`
is committed, and it is what makes 0xin installable on NixOS as a real session rather
than a checkout somebody has to build by hand.

```sh
nix build          # packages.default — the 0xin and 0xinctl binaries
nix develop        # devShells.default — the system dependencies above, plus cargo
nix flake check    # builds the package and runs the test suite in the sandbox
nix run . -- kitty # apps.default
```

On Arch itself, Nix needs three things done once before any of that works. The package
installs the binaries and the systemd units but **does not create the store**, and
because the client resolves its store path before it ever connects to the daemon, every
command fails with `opening file "/nix/store": No such file or directory` until it
exists:

```sh
sudo systemctl enable --now nix-daemon.socket
sudo nix-store --init     # creates /nix/store — the step that is easy to miss
mkdir -p ~/.config/nix && printf 'experimental-features = nix-command flakes\n' >> ~/.config/nix/nix.conf
```

Flakes are not a separate package — they are an experimental feature of the `nix`
already installed, which is what the third line turns on.

The flake follows the one in [oslo](https://github.com/termworks/oslo) — nixpkgs
pinned by revision, `flake-utils` over x86_64 and aarch64 (the FP5 profile is the
aarch64 target), `rust-overlay` for the toolchain, and the version read out of
`Cargo.toml` — with three deliberate differences:

- **The Rust version is not written in `flake.nix`.** oslo deleted its
  `rust-toolchain.toml` and moved the number into the flake, because two copies drift.
  0xin needs the file — it is how the rustup build above gets 1.96.0 — so the flake
  reads the channel out of it instead. The number still lives in exactly one place.
- **No static build.** oslo ships a static musl binary; a compositor cannot. 0xin
  links wlroots, EGL/GLESv2, libinput, libdrm and libseat dynamically, and the GPU
  driver has to come from the host at runtime.
- **A different nixpkgs revision.** Not a preference: crates.io now answers every
  `/api/v1/crates/<crate>/<version>/download` with a 403, and nixpkgs only switched
  crate fetching to the `static.crates.io` CDN after the revision oslo pins. Against
  an older nixpkgs the build cannot vendor its dependencies at all. The revision here
  carries that fix and still has wlroots 0.19.3 — the same version Arch ships.

`buildInputs` is short, and there is one entry per pkg-config module `build.rs`
probes: `wlroots-0.19`, `wayland-server`, `xkbcommon`, `libdrm`, `pixman-1`, `glesv2`
and `egl`, plus `wayland-protocols` for its `pkgdatadir`. Everything else 0xin links —
libseat, libgbm, libinput, libdisplay-info, libliftoff — is pulled in by wlroots and
needs no entry. `libclang` comes from `rustPlatform.bindgenHook`, which sets the
include arguments bindgen needs.

`libdrm` and `pixman-1` are probed by name for a reason worth knowing: wlroots lists
its dependencies under `Requires.private`, and whether their include paths reach a
compile depends on how the `.pc` chain gets resolved. On Arch they arrive for free, so
`shim/output.c`'s `#include <drm_fourcc.h>` and wlroots' own `#include <pixman.h>` just
work; in a Nix build sandbox they do not, and the compile fails on a missing header.
Naming both in `build.rs` costs nothing and makes the build independent of that
difference — which is why the fix lives in `build.rs` rather than in `flake.nix`.

### Running the Nix build on a non-NixOS host

`nix build` produces a working compositor, but **running it on Arch will not get you a
picture**: it fails at `Could not initialize renderer`, with no EGL client extensions
and `ERROR_INCOMPATIBLE_DRIVER` from Vulkan. Nothing is wrong with the build — a
Nix-built binary links Nix's libglvnd, which looks for the GPU driver Nix knows about,
and Arch's Mesa is not it. On NixOS the driver comes from `hardware.graphics`, which
`programs.oxin.enable` turns on, and the same binary runs fine; elsewhere it needs
[nixGL](https://github.com/nix-community/nixGL).

So on Arch, use Nix to *build* and `cargo` to *run* — which is the order this chapter
already recommends. The same caveat applies to running the compositor out of `nix
develop`; building there is fine.

The `luna` dependency is a git tag, so `cargoLock.outputHashes` carries a hash for it.
It only ever changes when the tag in `Cargo.toml` does: set it to `lib.fakeHash`, run
`nix build`, and copy the `got:` hash from the error.

### As a NixOS session

`nixosModules.default` turns the package into a session the machine offers:

```nix
imports = [ inputs.oxin.nixosModules.default ];

programs.oxin = {
  enable = true;
  extraPackages = [ pkgs.kitty ];  # Mod+Return spawns kitty by default
  config = ''
    local oxin = require("oxin")
    oxin.gap = 10
  '';
  plugins = [ ./my-0xin-plugin ];
};
```

`enable` installs both binaries and registers the `wayland-sessions` entry the package
ships, so a display manager lists **0xin** to log into. It also turns on
`hardware.graphics` and polkit; seatd is deliberately left alone, because logind hands
the active VT its devices — the same thing `LIBSEAT_BACKEND=logind` arranges on Arch.

`config` is written to `/etc/xdg/0xin/init.lua` and `plugins` are linked under
`/etc/xdg/0xin/pack/nix/start/`, which is a runtimepath root 0xin already scans (see
[Configuration](../README.md#configuration)). A user's own `~/.config/0xin/init.lua`
still takes precedence over the system one.

## The FFI pipeline

`build.rs` does four things, in order, every build:

1. Resolves wlroots/wayland/etc. include and link flags via `pkg-config`.
2. Generates the `xdg-shell` protocol header with `wayland-scanner` into
   `OUT_DIR` (wlroots' own xdg-shell header `#include`s this, and it isn't a
   system header — it has to be generated from the protocol XML on every
   machine that builds 0xin).
3. Compiles the C shim (`shim/*.c`) via the `cc` crate.
4. Runs `bindgen` over `wrapper.h`, allowlisting only the functions/types
   0xin actually calls (see [Architecture](architecture.md) for why the
   allowlist exists and what it means for opaque struct types).

See [`build.rs`](https://github.com/termworks/0xin/blob/main/build.rs) for the
exact allowlist and flag wiring.

## Running it

Two run modes, both via cargo aliases in `.cargo/config.toml`:

- **`cargo nested`** — the fast dev loop. Inside an existing Wayland session,
  `wlr_backend_autocreate` picks the nested Wayland backend automatically and
  0xin opens as an ordinary window on the host desktop. `OXIN_MOD=alt cargo
  nested -- kitty` sets the modifier to Alt (since the host compositor
  usually grabs Super-chords before a nested client sees them) and launches a
  test client against 0xin's own socket.
- **Real TTY (DRM/KMS)** — from a free virtual terminal, logged in:
  `LIBSEAT_BACKEND=logind ~/proj/0xin/target/debug/0xin kitty
  2>~/0xin-tty.log`. `wlr_backend_autocreate` detects there's no
  `WAYLAND_DISPLAY` and picks the DRM/KMS backend instead — this is 0xin as
  a real session, not a nested toy. `LIBSEAT_BACKEND=logind` lets logind hand
  the active VT its devices without needing the `seat` group.

Full recipes, verification commands, and known gotchas (multi-GPU device
selection, VT-switch repaint behavior, headless screenshot verification) live
in [Running & Verifying](running.md) and the in-repo
[`notes/`](https://github.com/termworks/0xin/tree/main/notes) directory, which
is the day-to-day working reference this chapter is distilled from.
