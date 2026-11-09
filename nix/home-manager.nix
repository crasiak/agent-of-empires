{ aoePackageFor }:
{ config, lib, pkgs, ... }:
let
  cfg = config.programs.aoe;
  toml = pkgs.formats.toml { };
  safeRelativeDirectory = value:
    value != null && value != "" && !(lib.hasPrefix "/" value)
    && lib.all (part: part != "" && part != "." && part != "..") (lib.splitString "/" value);
in {
  options.programs.aoe = {
    enable = lib.mkEnableOption "the selected native AoE package";
    package = lib.mkOption {
      type = lib.types.package;
      default = aoePackageFor pkgs.stdenv.hostPlatform.system;
      description = "Native AoE package selected from the same flake revision on each host.";
    };
    appDirectory = lib.mkOption {
      type = lib.types.nullOr lib.types.str;
      default = null;
      description = "Existing AoE app directory relative to HOME. Required only for explicitly managed profiles; select the actual Linux/XDG or macOS layout.";
    };
    profiles = lib.mkOption {
      type = lib.types.attrsOf toml.type;
      default = { };
      description = "Profiles whose config.toml is exclusively managed with the package generation. Empty by default; no existing configuration is adopted.";
    };
  };

  config = lib.mkIf cfg.enable {
    assertions = [
      {
        assertion = cfg.profiles == { } || safeRelativeDirectory cfg.appDirectory;
        message = "programs.aoe.appDirectory must be an explicit HOME-relative directory without traversal when managing profiles.";
      }
      {
        assertion = lib.all (name:
          builtins.match "[A-Za-z0-9_-]+" name != null
          && builtins.stringLength name <= 64 && lib.toLower name != "all"
        ) (builtins.attrNames cfg.profiles);
        message = "programs.aoe.profiles names must use 1-64 letters, digits, underscores or hyphens; 'all' is reserved.";
      }
    ];
    home.packages = [ cfg.package ];
    home.file = lib.mapAttrs' (name: settings: lib.nameValuePair
      "${if cfg.appDirectory == null then "invalid-aoe-directory" else cfg.appDirectory}/profiles/${name}/config.toml" {
        source = toml.generate "aoe-${name}-config.toml" settings;
        force = false;
      }) cfg.profiles;
  };
}
