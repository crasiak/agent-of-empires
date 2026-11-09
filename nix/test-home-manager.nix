{ pkgs, home-manager, aoeModule }:
let
  fakePackage = version: pkgs.writeShellScriptBin "aoe" "echo ${version}";
  previous = fakePackage "previous";
  candidate = fakePackage "candidate";
  mkHome = package: appDirectory: profiles: home-manager.lib.homeManagerConfiguration {
    inherit pkgs;
    modules = [ aoeModule {
      home.username = "aoe-fixture";
      home.homeDirectory = "/tmp/aoe-native-generation-fixture";
      home.stateVersion = "25.11";
      programs.aoe = { enable = true; inherit package appDirectory profiles; };
    } ];
  };
  old = mkHome previous ".config/agent-of-empires" { work.session.pi_integration = "none"; };
  new = mkHome candidate ".config/agent-of-empires" { work.session.pi_integration = "identity"; };
  packageOnly = mkHome candidate null { };
  target = ".config/agent-of-empires/profiles/work/config.toml";
  valid = home: (builtins.tryEval home.activationPackage.drvPath).success;
  unsafeDirectory = mkHome candidate "../outside" { work.session.pi_integration = "none"; };
  unsafeProfile = mkHome candidate ".config/agent-of-empires" { "../../outside".session.pi_integration = "none"; };
  reservedProfile = mkHome candidate ".config/agent-of-empires" { ALL.session.pi_integration = "none"; };
  longProfile = mkHome candidate ".config/agent-of-empires" { ${pkgs.lib.concatStrings (builtins.genList (_: "a") 65)}.session.pi_integration = "none"; };
in
assert valid old && valid new && valid packageOnly;
assert !(valid unsafeDirectory) && !(valid unsafeProfile);
assert !(valid reservedProfile) && !(valid longProfile);
assert builtins.elem previous old.config.home.packages;
assert builtins.elem candidate new.config.home.packages;
assert old.config.home.file.${target}.source != new.config.home.file.${target}.source;
assert !old.config.home.file.${target}.force && !new.config.home.file.${target}.force;
assert !(builtins.hasAttr target packageOnly.config.home.file);
assert builtins.all (name:
  !(pkgs.lib.hasInfix "sessions.json" name) && !(pkgs.lib.hasInfix ".schema_version" name)
  && !(pkgs.lib.hasInfix "/afk/" name) && !(pkgs.lib.hasInfix "ledger" name)
) (builtins.attrNames old.config.home.file);
pkgs.runCommand "aoe-home-manager-contract" { } ''
  test ${old.config.home.file.${target}.source} != ${new.config.home.file.${target}.source}
  touch "$out"
''
