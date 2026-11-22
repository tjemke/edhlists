{
  description = "edh-scout: find well-rounded, fun Commander decklists on EDHREC";

  inputs.nixpkgs.url = "github:NixOS/nixpkgs/nixos-unstable";

  outputs =
    { self, nixpkgs }:
    let
      systems = [
        "x86_64-linux"
        "aarch64-linux"
        "x86_64-darwin"
        "aarch64-darwin"
      ];
      forAllSystems = f: nixpkgs.lib.genAttrs systems (system: f nixpkgs.legacyPackages.${system});
    in
    {
      packages = forAllSystems (pkgs: {
        # Forge's card scripts, for the mana goldfish (crates/edh-data/src/forge.rs): just its
        # res/cardsfolder, from the source of nixpkgs' forge-mtg (GPL-3.0), so `sync` has nothing
        # to download for them.
        forge-cards = pkgs.runCommand "forge-cardsfolder-${pkgs.forge-mtg.version}" { } ''
          cp -r ${pkgs.forge-mtg.src}/forge-gui/res/cardsfolder $out
        '';

        default = pkgs.rustPlatform.buildRustPackage {
          pname = "edh-scout";
          version = (builtins.fromTOML (builtins.readFile ./Cargo.toml)).workspace.package.version;
          src = pkgs.lib.fileset.toSource {
            root = ./.;
            fileset = pkgs.lib.fileset.unions [
              ./Cargo.toml
              ./Cargo.lock
              ./crates
              # Read by a test that keeps the changelog in step with the version.
              ./CHANGELOG.md
            ];
          };
          cargoLock.lockFile = ./Cargo.lock;
          cargoBuildFlags = [ "--package" "edh-scout" ];
          env.EDH_SCOUT_FORGE_CARDS_BUILTIN = "${self.packages.${pkgs.stdenv.hostPlatform.system}.forge-cards}";
          # The whole test suite is offline (fixtures live in crates/*/tests/data).
          cargoTestFlags = [ "--workspace" ];
          meta = {
            description = "Find well-rounded, fun Commander decklists on EDHREC, scored with EDHPowerLevel";
            mainProgram = "edh-scout";
          };
        };
      });

      devShells = forAllSystems (pkgs: {
        default = pkgs.mkShell {
          packages = with pkgs; [
            cargo
            rustc
            clippy
            rustfmt
            rust-analyzer
            nodejs # for crates/edhpl/reference
          ];
        };
      });
    };
}
