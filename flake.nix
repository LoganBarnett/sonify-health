{
  description = "Infrastructure sonification daemon and CLI";
  inputs = {
    # LLM: Do NOT change this URL unless explicitly directed. This is the
    # correct format for nixpkgs stable (25.11 is correct, not nixos-25.11).
    nixpkgs.url = "github:NixOS/nixpkgs/25.11";
    rust-overlay.url = "github:oxalica/rust-overlay";
    crane.url = "github:ipetkov/crane";
    foundation.url = "github:LoganBarnett/rust-template";
    foundation.inputs.nixpkgs.follows = "nixpkgs";
  };

  outputs = {
    self,
    nixpkgs,
    rust-overlay,
    crane,
    foundation,
  }: let
    # Audio + frontend dependencies that aren't part of foundation's
    # default crane build: alsa headers for cpal on Linux, libiconv for
    # Darwin, and pkg-config to find them.  Both binaries link the
    # audio stack via the lib crate, so this applies to every package.
    audioBuildInputs = system: pkgs:
      pkgs.lib.optionals pkgs.stdenv.isLinux [pkgs.alsa-lib]
      ++ pkgs.lib.optionals pkgs.stdenv.isDarwin (with pkgs.darwin; [
        libiconv
      ]);

    project = foundation.lib.mkRustProject {
      inherit self nixpkgs rust-overlay crane;
      name = "sonify-health";
      crates = {
        cli = {
          name = "sonify-health-cli";
          binary = "sonify-health";
          description = "Infrastructure sonification CLI";
        };
        server = {
          name = "sonify-health-server";
          binary = "sonify-health-server";
          description = "Infrastructure sonification daemon";
        };
      };
      extraBuildInputs = [];
      extraNativeBuildInputs = [];
      extraDevPackages = system: pkgs:
        [
          pkgs.cargo-sweep
          pkgs.jq
          # Unified formatter and per-language helpers.
          pkgs.treefmt
          pkgs.alejandra
          pkgs.prettier
          # Elm frontend toolchain.
          pkgs.elmPackages.elm
          pkgs.elmPackages.elm-format
          pkgs.elm2nix
          # Task runner.
          pkgs.just
          # Native build inputs surfaced into the devShell so
          # `cargo build` in the shell can link cpal.
          pkgs.pkg-config
        ]
        ++ audioBuildInputs system pkgs;
      shellHook = _pkgs: ''
        echo "sonify-health development environment"
        echo ""
        echo "Available Cargo packages (use 'cargo build -p <name>'):"
        cargo metadata --no-deps --format-version 1 2>/dev/null | \
          jq --raw-output '.packages[].name' | \
          sort | \
          sed 's/^/  • /' || echo "  Run 'cargo init' to get started"

        echo ""
        echo "Elm frontend:"
        echo "  Build:       cd frontend && elm make src/Main.elm --output public/elm.js"
        echo "  Regenerate:  cd frontend && elm2nix convert 2>/dev/null > elm-srcs.nix && elm2nix snapshot"
      '';
    };

    # mkRustProject's `extraBuildInputs` parameter takes a flat list
    # (not a system-aware function), which can't carry cpal's
    # platform-specific dependencies (alsa-lib on Linux,
    # libiconv on Darwin).  Until foundation grows a system-aware
    # variant, build a parallel packages set that wires the audio
    # inputs through manually for the per-crate crane builds and
    # merge it over mkRustProject's defaults.
    workspaceCrates = {
      cli = {
        name = "sonify-health-cli";
      };
      server = {
        name = "sonify-health-server";
      };
    };
    overrideAudioInputs = system: let
      pkgs = import nixpkgs {
        inherit system;
        overlays = [(import rust-overlay)];
      };
      craneLib =
        (crane.mkLib pkgs).overrideToolchain
        (p: p.rust-bin.stable.latest.default);
      commonArgs = {
        src = craneLib.cleanCargoSource self;
        buildInputs = audioBuildInputs system pkgs;
        nativeBuildInputs = [pkgs.pkg-config];
        cargoTestExtraArgs = "--lib --bins";
      };
      buildCrate = key: crate: let
        pkgFile = ./. + "/nix/packages/${key}.nix";
      in
        if builtins.pathExists pkgFile
        then import pkgFile {inherit craneLib commonArgs pkgs;}
        else
          craneLib.buildPackage (commonArgs
            // {
              pname = crate.name;
              cargoExtraArgs = "-p ${crate.name}";
            });
    in
      nixpkgs.lib.mapAttrs buildCrate workspaceCrates
      // {
        default = craneLib.buildPackage (commonArgs // {pname = "sonify-health";});
      };

    systems = nixpkgs.lib.systems.flakeExposed;
    audioPackages = nixpkgs.lib.genAttrs systems overrideAudioInputs;
  in
    project
    // {
      packages =
        nixpkgs.lib.genAttrs systems (system:
          project.packages.${system} // audioPackages.${system});

      nixosModules = {
        daemon = import ./nix/modules/nixos-daemon.nix {inherit self;};
        default = self.nixosModules.daemon;
      };

      darwinModules = {
        daemon = import ./nix/modules/darwin-daemon.nix {inherit self;};
        default = self.darwinModules.daemon;
      };
    };
}
