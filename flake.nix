{
  description = "0xin — tiling Wayland compositor on Smithay, development shell";

  inputs = {
    nixpkgs.url = "github:NixOS/nixpkgs/nixos-unstable";
    rust-overlay.url = "github:oxalica/rust-overlay";
    flake-utils.url = "github:numtide/flake-utils";
    nixgl.url = "github:nix-community/nixGL";
  };

  outputs =
    { self, nixpkgs, rust-overlay, flake-utils, nixgl, ... }:
    flake-utils.lib.eachDefaultSystem (
      system:
      let
        pkgs = import nixpkgs {
          inherit system;
          overlays = [ (import rust-overlay) ];
          config = {
            allowUnfree = true;
            nvidia.acceptLicense = true;
          };
        };

        # The NVIDIA userspace nixGL builds has to match the running kernel
        # module exactly, so we need its version — read straight out of /proc,
        # with no environment variable and no cooperation from .envrc. Needs
        # --impure; a pure evaluation falls back to the Mesa wrappers rather
        # than failing.
        #
        # nixGL has this auto-detection built in, but its regex only matches
        # the classic module string; a 595 *open* kernel module reports
        # "Open Kernel Module for x86_64  595.71.05  Release Build", which it
        # misses and then silently degrades to Mesa. Hence our own match.
        #
        # builtins.readFile cannot read /proc (files there report size 0), so
        # the copy happens in a derivation — the same trick nixGL uses. The
        # timestamp keeps it from being cached across driver updates.
        nvidiaVersionFile = pkgs.runCommand "impure-nvidia-version" {
          time = builtins.currentTime;
          preferLocalBuild = true;
          allowSubstitutes = false;
        } "cp /proc/driver/nvidia/version $out 2>/dev/null || touch $out";

        detectedNvidiaVersion =
          let
            data = builtins.readFile nvidiaVersionFile;
            match = builtins.match ".*  ([0-9]+\\.[0-9.]+)  .*" data;
          in if match == null then null else builtins.head match;

        # A pure evaluation cannot read /proc at all; getEnv returning "" for
        # a variable that always exists is how we spot it.
        pureEval = builtins.getEnv "HOME" == "";

        nvidiaVersion = if pureEval then null else detectedNvidiaVersion;
        hasNvidia = nvidiaVersion != null;

        # nixGL packages the NVIDIA userspace itself, so it has to be evaluated
        # against the nixpkgs *it* pins: nixos-unstable changed the argument
        # set of nvidia-x11/generic.nix and nixGL's call now fails there with
        # "unexpected argument 'kernel'".
        nixglPkgs = import "${nixgl}/default.nix" {
          pkgs = import nixgl.inputs.nixpkgs {
            inherit system;
            config = {
              allowUnfree = true;
              nvidia.acceptLicense = true;
            };
          };
          inherit nvidiaVersion;
          nvidiaHash = null;
        };

        # Stable command names over the version-stamped binaries nixGL
        # installs, so RUN_WITH in .envrc never has to know the driver
        # revision.
        mkAlias = name: drv: bin: pkgs.runCommand name { } ''
          mkdir -p $out/bin
          ln -s ${drv}/bin/${bin} $out/bin/${name}
        '';

        nixGLAlias =
          if hasNvidia then
            mkAlias "nixGL" nixglPkgs.nixGLNvidia "nixGLNvidia-${nvidiaVersion}"
          else
            mkAlias "nixGL" nixglPkgs.nixGLIntel "nixGLIntel";
        nixVulkanAlias =
          if hasNvidia then
            mkAlias "nixVulkan" nixglPkgs.nixVulkanNvidia "nixVulkanNvidia-${nvidiaVersion}"
          else
            mkAlias "nixVulkan" nixglPkgs.nixVulkanIntel "nixVulkanIntel";

        lavapipeAlias = pkgs.writeShellScriptBin "nixLavapipe" ''
          set -eu

          icd_dir=${pkgs.mesa}/share/vulkan/icd.d
          icd=
          for candidate in "$icd_dir"/lvp_icd.*.json; do
            if [ -f "$candidate" ]; then
              icd="$candidate"
              break
            fi
          done

          if [ -z "$icd" ]; then
            echo "nixLavapipe: unable to find the Lavapipe Vulkan ICD in $icd_dir" >&2
            exit 1
          fi

          export VK_DRIVER_FILES="$icd"
          exec "$@"
        '';

        # What Smithay's backends link against: wayland + xkbcommon for the
        # frontend, libinput/libseat/udev for the TTY session, libdrm/gbm/mesa
        # for DRM-KMS and the GLES renderer, pixman for the software path.
        compositorLibs = with pkgs; [
          wayland
          libxkbcommon
          libinput
          seatd
          udev
          libdrm
          libgbm
          mesa
          libGL
          vulkan-loader
          pixman
        ];

        # Honours rust-toolchain.toml (1.96.0 + rustfmt/clippy), so this shell
        # and a rustup checkout agree on the compiler.
        rustToolchain = pkgs.rust-bin.fromRustupToolchainFile ./rust-toolchain.toml;
      in
      {
        devShells.default = pkgs.mkShell {
          nativeBuildInputs = [
            rustToolchain
            pkgs.pkg-config
            pkgs.clang
          ];

          buildInputs = compositorLibs;

          packages = [
            nixGLAlias
            nixVulkanAlias
            lavapipeAlias
            nixglPkgs.nixGLIntel
            nixglPkgs.nixVulkanIntel
          ]
          ++ pkgs.lib.optionals hasNvidia [
            nixglPkgs.nixGLNvidia
            nixglPkgs.nixVulkanNvidia
          ];

          # Marker for the Makefile: inside this shell it runs cargo directly,
          # outside it re-enters through `nix develop`. IN_NIX_SHELL is no good
          # for that — any unrelated nix shell sets it too.
          OXIN_DEVSHELL = "1";

          # cargo-built binaries carry no RPATH for these, and the EGL/GLES
          # stack is loaded at run time.
          LD_LIBRARY_PATH = pkgs.lib.makeLibraryPath compositorLibs;
          RUST_SRC_PATH = "${pkgs.rust-bin.stable."1.96.0".rust-src}/lib/rustlib/src/rust/library";
        };
      }
    );
}
