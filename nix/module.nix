# NixOS module for 0xin.
#
# oslo has no equivalent — a shell is a package you install, a compositor is a session
# the machine has to be configured to offer. This is the "declare it in NixOS" half:
# `programs.oxin.enable = true` gets you the binaries, a session entry the display
# manager lists, and somewhere to put config and plugins declaratively.
self:
{ config, lib, pkgs, ... }:

let
  cfg = config.programs.oxin;
  inherit (lib) mkDefault mkEnableOption mkIf mkOption types;
in
{
  options.programs.oxin = {
    enable = mkEnableOption "0xin, a tiling Wayland compositor";

    package = mkOption {
      type = types.package;
      default = self.packages.${pkgs.stdenv.hostPlatform.system}.oxin;
      defaultText = lib.literalMD "the `oxin` package from this flake";
      description = "The 0xin package to use.";
    };

    config = mkOption {
      type = types.nullOr types.lines;
      default = null;
      example = lib.literalExpression ''
        '''
          local oxin = require("oxin")
          oxin.modifier = "super"
          oxin.gap = 10
        '''
      '';
      description = ''
        Lua written to `/etc/xdg/0xin/init.lua`, the system-wide config. A user's own
        `~/.config/0xin/init.lua` takes precedence over it.
      '';
    };

    plugins = mkOption {
      type = types.listOf types.path;
      default = [ ];
      example = lib.literalExpression "[ ./my-0xin-plugin ]";
      description = ''
        Plugin directories, each laid out like a config root (`plugin/`, `lua/`,
        `after/`). They are linked under `/etc/xdg/0xin/pack/nix/start/`, which 0xin
        already scans as part of its runtimepath.
      '';
    };

    extraPackages = mkOption {
      type = types.listOf types.package;
      default = [ ];
      example = lib.literalExpression "[ pkgs.kitty pkgs.grim ]";
      description = ''
        Extra packages to install alongside 0xin. Worth setting: the default
        `Mod+Return` binding spawns `kitty`, which is not otherwise pulled in.
      '';
    };
  };

  config = mkIf cfg.enable {
    environment.systemPackages = [ cfg.package ] ++ cfg.extraPackages;

    # The package ships share/wayland-sessions/0xin.desktop; this is what makes a
    # display manager list "0xin" as a session to log into.
    services.displayManager.sessionPackages = [ cfg.package ];

    environment.etc =
      lib.optionalAttrs (cfg.config != null) {
        "xdg/0xin/init.lua".text = cfg.config;
      }
      // lib.listToAttrs (
        map (plugin: {
          name = "xdg/0xin/pack/nix/start/${baseNameOf plugin}";
          value.source = plugin;
        }) cfg.plugins
      );

    # 0xin drives DRM/KMS and libinput directly. logind hands the active VT its devices
    # (the same thing LIBSEAT_BACKEND=logind arranges on Arch), so seatd is not needed.
    hardware.graphics.enable = mkDefault true;
    security.polkit.enable = true;
    fonts.enableDefaultPackages = mkDefault true;
  };
}
