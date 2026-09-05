{
  description = "0xin — a from-scratch tiling Wayland compositor in Rust on wlroots";

  inputs = {
    # Pinned by revision, as oslo's is. Not the *same* revision, though: oslo's predates
    # nixpkgs switching crate downloads to the static.crates.io CDN, and crates.io's API
    # now answers every download with a 403 (rust-lang/crates.io#13482), so a build
    # against that revision cannot vendor its dependencies at all. This one carries the
    # fix and still has wlroots 0.19.3 — the exact version the Arch build uses.
    nixpkgs.url = "github:NixOS/nixpkgs?rev=e5ead30d0824debba629dcf0720abeddee57b7d6";
    flake-utils.url = "github:numtide/flake-utils";
    rust-overlay.url = "github:oxalica/rust-overlay";
  };

  outputs =
    { self, nixpkgs, flake-utils, rust-overlay, ... }:
    let
      systems = [
        "x86_64-linux"
        "aarch64-linux"
      ];
      manifest = builtins.fromTOML (builtins.readFile ./Cargo.toml);
      version = manifest.package.version;
      # **The toolchain version is NOT pinned here.**
      #
      # `rust-toolchain.toml` already pins it for the rustup/Arch build, which is how 0xin
      # is developed day to day (see `docs/environment.md`). Writing the number a second
      # time in this file would create exactly the drift oslo's flake removed by deleting
      # its own `rust-toolchain.toml` — two copies, neither checking the other. 0xin cannot
      # delete the file, so the flake reads it instead: one number, one place, and a Nix
      # build and a cargo build can never disagree about the compiler.
      rustChannel = (builtins.fromTOML (builtins.readFile ./rust-toolchain.toml)).toolchain.channel;
    in
    flake-utils.lib.eachSystem systems (
      system:
      let
        pkgs = import nixpkgs {
          inherit system;
          overlays = [ rust-overlay.overlays.default ];
        };
        lib = pkgs.lib;
        toolchain = pkgs.rust-bin.stable.${rustChannel}.default.override {
          extensions = [
            "rust-src"
            "rust-analyzer"
            "clippy"
            "rustfmt"
          ];
        };
        # Build the package with the pinned toolchain too, not just the devshell — the
        # point of reading `rust-toolchain.toml` is that every build here uses it.
        rustPlatform = pkgs.makeRustPlatform {
          cargo = toolchain;
          rustc = toolchain;
        };

        # A session entry, so a display manager can offer 0xin as a Wayland session.
        # `services.displayManager.sessionPackages` in `nix/module.nix` picks this up.
        sessionDesktop = pkgs.writeText "0xin.desktop" ''
          [Desktop Entry]
          Name=0xin
          Comment=${manifest.package.description}
          Exec=0xin
          TryExec=0xin
          Type=Application
          DesktopNames=0xin
        '';

        oxin = rustPlatform.buildRustPackage {
          pname = "oxin";
          inherit version;
          src = ./.;

          cargoLock = {
            lockFile = ./Cargo.lock;
            # luna comes from a git tag, so cargo's own lock hash isn't enough. This is
            # for v0.5.1 specifically; oslo's committed luna-0.5.0 hash is a different
            # tree and will not match.
            outputHashes = {
              "luna-0.5.1" = "sha256-gCdX/Ni2RbK69KMJFVzK72QSuDFVVQM2deiKXH3AvoI=";
            };
          };

          # `bindgenHook` is what sets LIBCLANG_PATH and the clang include arguments that
          # `build.rs`'s bindgen pass needs; `wayland-scanner` generates the three protocol
          # headers into OUT_DIR.
          nativeBuildInputs = [
            pkgs.pkg-config
            pkgs.wayland-scanner
            rustPlatform.bindgenHook
          ];

          # Exactly the five modules `build.rs` probes with pkg-config, plus
          # wayland-protocols for its `pkgdatadir` variable. libglvnd is what provides the
          # `egl` and `glesv2` .pc files. Everything else 0xin links (libseat, libgbm,
          # libinput, libdrm, pixman, libdisplay-info, …) propagates through wlroots — see
          # `ldd` on a built binary.
          buildInputs = [
            pkgs.wlroots_0_19
            pkgs.wayland
            pkgs.wayland-protocols
            pkgs.libxkbcommon
            pkgs.libglvnd
          ];

          # `src/lua/tests.rs` reads HOME and XDG_CONFIG_HOME to work out where a config
          # would live; the sandbox sets neither.
          preCheck = ''
            export HOME="$TMPDIR"
            export XDG_CONFIG_HOME="$TMPDIR/config"
          '';

          postInstall = ''
            install -Dm444 ${sessionDesktop} "$out/share/wayland-sessions/0xin.desktop"
          '';

          # A compositor can't be started in a sandbox with no seat, no GPU and no display,
          # so the install check asserts what such a sandbox *can* prove about the build.
          doInstallCheck = true;
          installCheckPhase = ''
            runHook preInstallCheck

            test -x "$out/bin/0xin"
            test -x "$out/bin/0xinctl"
            test -f "$out/share/wayland-sessions/0xin.desktop"

            # 0xinctl with no arguments prints its usage and exits 2 without touching the
            # control socket (src/bin/0xinctl.rs) — the one thing here that actually runs.
            set +e
            usage="$("$out/bin/0xinctl" 2>&1)"
            status=$?
            set -e
            if [ "$status" -ne 2 ]; then
              echo "error: 0xinctl with no arguments exited $status, expected 2" >&2
              exit 1
            fi
            case "$usage" in
              usage:*) ;;
              *)
                echo "error: 0xinctl printed no usage: $usage" >&2
                exit 1
                ;;
            esac

            runHook postInstallCheck
          '';

          meta = {
            description = manifest.package.description;
            homepage = "https://github.com/termworks/0xin";
            license = lib.licenses.mit;
            mainProgram = "0xin";
            platforms = lib.platforms.linux;
          };
        };

        mkApp = package: binaryName: {
          type = "app";
          program = "${package}/bin/${binaryName}";
          meta.description = "Run ${binaryName}";
        };
      in
      {
        packages = {
          inherit oxin;
          default = oxin;
        };

        apps = {
          default = mkApp oxin "0xin";
          "0xin" = mkApp oxin "0xin";
          "0xinctl" = mkApp oxin "0xinctl";
        };

        checks = {
          inherit oxin;
          default = oxin;
        };

        # Unlike oslo's static build, this one needs wlroots and friends on
        # PKG_CONFIG_PATH, which is what `inputsFrom` arranges.
        devShells.default = pkgs.mkShell {
          inputsFrom = [ oxin ];
          packages = [
            toolchain
            pkgs.mdbook
            pkgs.git
          ];
          RUST_BACKTRACE = 1;
        };
      }
    )
    // {
      overlays.default = final: _prev: {
        oxin = self.packages.${final.system}.oxin;
      };

      nixosModules.default = import ./nix/module.nix self;
      nixosModules.oxin = self.nixosModules.default;
    };
}
