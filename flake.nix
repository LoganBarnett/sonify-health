{
  description = "Infrastructure sonification daemon and CLI";
  inputs = {
    # LLM: Do NOT change this URL unless explicitly directed. This is the
    # correct format for nixpkgs stable (25.11 is correct, not nixos-25.11).
    nixpkgs.url = "github:NixOS/nixpkgs/25.11";
    rust-overlay.url = "github:oxalica/rust-overlay";
    crane.url = "github:ipetkov/crane";
    org-fmt.url = "github:LoganBarnett/org-fmt";
    org-fmt.inputs.nixpkgs.follows = "nixpkgs";
    org-fmt.inputs.rust-overlay.follows = "rust-overlay";
    org-fmt.inputs.crane.follows = "crane";
    foundation.url = "github:LoganBarnett/rust-template";
    foundation.inputs.nixpkgs.follows = "nixpkgs";
  };

  outputs = {
    self,
    nixpkgs,
    rust-overlay,
    crane,
    org-fmt,
    foundation,
  }: let
    forAllSystems =
      nixpkgs.lib.genAttrs nixpkgs.lib.systems.flakeExposed;

    perSystem = forAllSystems (system: let
      pkgs = import nixpkgs {
        inherit system;
        overlays = [(import rust-overlay)];
      };
      craneLib =
        (crane.mkLib pkgs).overrideToolchain
        (p: p.rust-bin.stable.latest.default);
      rust = pkgs.rust-bin.stable.latest.default.override {
        extensions = [
          "rust-src"
          "rust-analyzer"
          "rustfmt"
        ];
      };
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
      commonArgs = {
        src = craneLib.cleanCargoSource self;
        # Audio + frontend dependencies that aren't part of foundation's
        # default crane build: alsa headers for cpal on Linux, libiconv
        # for Darwin.  Both binaries link the audio stack via the lib
        # crate, so this applies to every package.
        buildInputs =
          nixpkgs.lib.optionals pkgs.stdenv.isLinux [pkgs.alsa-lib]
          ++ nixpkgs.lib.optionals pkgs.stdenv.isDarwin (
            with pkgs.darwin; [libiconv]
          );
        # pkg-config resolves the alsa-lib link flags for cpal at build time.
        nativeBuildInputs = [pkgs.pkg-config];
        # The Nix sandbox has no audio device, so the lib's audio tests
        # (strict by default — see crates/lib/src/audio.rs) need an
        # opt-out at build time.
        env = {sonify_health_tests_strict_audio_device = "false";};
      };
      rustPackages = foundation.lib.mkRustPackages {
        inherit self pkgs craneLib crates commonArgs;
      };
    in {
      inherit (rustPackages) packages apps;
      devShell = pkgs.mkShell {
        buildInputs = [
          # Rust toolchain (compiler, cargo, rustfmt, rust-analyzer).
          rust
          # Prunes stale per-profile artifacts from target/ to reclaim disk.
          pkgs.cargo-sweep
          # JSON parsing for the shellHook's cargo-package listing and ad-hoc
          # scripting in the dev shell.
          pkgs.jq
          # Unified formatter and per-language helpers.
          pkgs.treefmt
          pkgs.alejandra
          pkgs.prettier
          # Formats org-mode documents (treefmt delegates .org files to it).
          org-fmt.packages.${system}.default
          # Elm frontend toolchain.
          pkgs.elmPackages.elm
          pkgs.elmPackages.elm-format
          pkgs.elm2nix
          # Task runner.
          pkgs.just
        ];
        shellHook = ''
          ${foundation.lib.cargoHuskyHookSnippet pkgs}
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
    });
  in {
    devShells =
      nixpkgs.lib.mapAttrs (_: p: {default = p.devShell;}) perSystem;
    packages = nixpkgs.lib.mapAttrs (_: p: p.packages) perSystem;
    apps = nixpkgs.lib.mapAttrs (_: p: p.apps) perSystem;

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
