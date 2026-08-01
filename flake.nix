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

        # `use nvidia` in .envrc exports NVIDIA_VERSION from
        # /proc/driver/nvidia/version, before `use flake`, so it is always set
        # by the time this is read (hence --impure).
        nvidiaVersion = let v = builtins.getEnv "NVIDIA_VERSION";
        in if v != "" then v
           else throw "0xin: NVIDIA_VERSION is unset — is direnv loaded and is the NVIDIA driver running?";
        hasNvidia = true;

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

          # cargo-built binaries carry no RPATH for these, and the EGL/GLES
          # stack is loaded at run time.
          LD_LIBRARY_PATH = pkgs.lib.makeLibraryPath compositorLibs;
          RUST_SRC_PATH = "${pkgs.rust-bin.stable."1.96.0".rust-src}/lib/rustlib/src/rust/library";
        };
      }
    );
}
