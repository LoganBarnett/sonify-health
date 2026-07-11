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
    changelog-roller.url = "github:LoganBarnett/changelog-roller";
  };

  outputs = {
    self,
    nixpkgs,
    rust-overlay,
    crane,
    org-fmt,
    foundation,
    changelog-roller,
  }: let
    forAllSystems =
      nixpkgs.lib.genAttrs nixpkgs.lib.systems.flakeExposed;

    perSystem = forAllSystems (system: let
      pkgs = import nixpkgs {
        inherit system;
        overlays = [(import rust-overlay)];
        # apple-sdk, consumed below as apple-sdk.src for the macOS cross
        # builds, is unfree and darwin-gated, so evaluating pkgs.apple-sdk.src
        # on the x86_64-linux cross builder requires both acceptances.  The
        # licence consent stays visible here in the project rather than hidden
        # in the foundation library.
        config = {
          allowUnfree = true;
          allowUnsupportedSystem = true;
        };
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
      # On Linux each binary also gets a statically-linked `<name>-musl`
      # variant; empty on other systems.
      #
      # The musl build links everything statically (`-static-pie`), so it needs
      # a static `libasound.a` — which the dynamic `pkgs.alsa-lib` does not
      # provide.  Swap in `pkgsStatic.alsa-lib` (built for the musl target with
      # a static archive and the `hw`/`dmix` core plugins compiled in, so the
      # binary needs no dlopen) for the musl build only; the native and darwin
      # builds keep the dynamic alsa via the unmodified `commonArgs`.
      muslPackages = foundation.lib.mkMuslPackages {
        inherit self pkgs system crates crane;
        commonArgs =
          commonArgs
          // {
            buildInputs = nixpkgs.lib.optionals pkgs.stdenv.isLinux [
              pkgs.pkgsStatic.alsa-lib
            ];
          };
      };
      # On Linux each binary also gets a portable `<name>-gnu` variant: a
      # dynamic glibc build that runs off the Nix store (standard FHS
      # interpreter, glibc 2.17 floor via zig) and links the host's shared
      # libraries at runtime.  Unlike the static musl variant it can use the
      # host's libasound and its dlopen'd PCM plugins, so it is the right
      # download for a desktop that routes audio via PulseAudio or PipeWire.
      # It threads the unmodified commonArgs, so the alsa build inputs reach it
      # the same way.  Empty on non-Linux systems.
      gnuPortablePackages = foundation.lib.mkGnuPortablePackages {
        inherit self pkgs system crates crane commonArgs;
      };
      # The x86_64-linux build cross-compiles the macOS `<key>-<arch>-darwin`
      # variants via zig so a release needs no macOS runner; empty on other
      # systems.  appleSdk supplies the CoreAudio framework headers and link
      # stubs cpal needs — see CONTRIBUTING.org's Release binaries section.
      darwinCrossPackages = foundation.lib.mkDarwinCrossPackages {
        inherit self pkgs system crates crane commonArgs;
        appleSdk = pkgs.apple-sdk.src;
      };
      # Native Windows PE variants (`<key>-{x86_64,aarch64}-windows`),
      # cross-compiled via llvm-mingw for the gnullvm targets — no Microsoft
      # SDK, no Cygwin/MSYS2 runtime; the audio backend is WASAPI (a windows-sys
      # FFI binding, so no alsa), leaving only the OS Universal CRT (Windows
      # 10+).  Unlike the darwin cross build this is host-agnostic (llvm-mingw
      # ships a per-host toolchain), so it builds on the Linux CI runners and on
      # a contributor's Mac alike.  Requires a toolchain ≥ Rust 1.91 for the
      # aarch64 gnullvm std — see CONTRIBUTING.org.  commonArgs is threaded
      # unmodified, exactly as the darwin cross build does: cpal selects its
      # backend by target, so the host alsa/pkg-config inputs are inert here.
      windowsCrossPackages = foundation.lib.mkWindowsCrossPackages {
        inherit self pkgs system crates crane commonArgs;
      };
      # The opt-in MSVC-ABI Windows variant (`<key>-x86_64-windows-msvc`), for a
      # dependency that requires the MSVC ABI rather than the default gnullvm
      # path above.  sonify-health has no such dependency, so it stays off:
      # `windows-msvc` is absent from rust-template.json, `windowsMsvcEnabled`
      # is false, and the helper produces nothing.  The wiring is kept so the
      # flake matches the emitted template and flipping the flag on is a
      # one-line change — enabling it would accept Microsoft's SDK licence in
      # this project's own flake (via foundation.lib.xwinSdk), the visible
      # consent exactly as `appleSdk` surfaces the Apple SDK licence.
      windowsMsvcEnabled =
        (builtins.fromJSON (builtins.readFile ./rust-template.json)).windows-msvc
        or false;
      windowsMsvcCrossPackages = foundation.lib.mkWindowsMsvcCrossPackages {
        inherit self pkgs system crates crane commonArgs;
        xwinSdk =
          if windowsMsvcEnabled
          then foundation.lib.xwinSdk {inherit pkgs;}
          else null;
      };
      packages =
        rustPackages.packages
        // muslPackages
        // gnuPortablePackages
        // darwinCrossPackages
        // windowsCrossPackages
        // windowsMsvcCrossPackages;
      # The arm64 subset of the darwin cross outputs — the only ones
      # mkDarwinCrossPackages re-signs after the release profile's strip would
      # invalidate zig's link-time signature, and so the only ones the signature
      # guard below verifies.  Empty except on x86_64-linux.
      aarch64DarwinPackages =
        nixpkgs.lib.filterAttrs
        (name: _: nixpkgs.lib.hasSuffix "-aarch64-darwin" name)
        darwinCrossPackages;
      # The x86_64 subset of the Windows cross outputs, smoke-tested under wine.
      # These are non-empty on every host (the Windows helper is host-agnostic),
      # so the wine check below is gated on `system == "x86_64-linux"` rather
      # than on emptiness: wine runs a win64 PE reliably only there.
      windowsX86Packages =
        nixpkgs.lib.filterAttrs
        (name: _: nixpkgs.lib.hasSuffix "-x86_64-windows" name)
        windowsCrossPackages;
      # A zero-argument "paste and it works" entry point for Nix users:
      # `nix run .#quickstart` runs the server against the Star Trek preset,
      # the counterpart to the curl installer for anyone who has Nix.  On the
      # first run it copies the preset into the working directory as a writable
      # config.toml, since the bundled preset lives read-only in the Nix store;
      # a re-run keeps any existing config rather than clobbering edits.  The
      # preset's heartbeats shell out to `ip`, `awk`, and `ping`, so put those
      # on PATH for minimal hosts (containers, fresh installs) that lack them.
      # Those tools are Linux-only in nixpkgs, so gate them and let macOS use
      # its own system tools.
      quickstartApp = pkgs.writeShellApplication {
        name = "sonify-health-quickstart";
        runtimeInputs = nixpkgs.lib.optionals pkgs.stdenv.isLinux [
          pkgs.iproute2
          pkgs.gawk
          pkgs.iputils
        ];
        text = ''
          if [ ! -e config.toml ]; then
            cp ${self}/examples/connectivity-and-cpu-star-trek.toml config.toml
            chmod +w config.toml
          fi
          exec ${rustPackages.packages.server}/bin/sonify-health-server \
            --config config.toml
        '';
      };
    in {
      inherit packages;
      apps =
        rustPackages.apps
        // {
          quickstart = {
            type = "app";
            program = "${quickstartApp}/bin/sonify-health-quickstart";
          };
        };
      # Add the darwin ad-hoc signature guard to the workspace's checks on
      # x86_64-linux, where the zig-cross darwin binaries are produced.  An
      # arm64 Mach-O with an invalid signature is SIGKILLed by the kernel with
      # no output, so this check proves the shipped signature survived the
      # strip that mkDarwinCrossPackages re-signs around.  Only the arm64
      # outputs are checked — x86_64 macOS does not enforce signatures.  Empty
      # (and so absent) on every other system.
      checks =
        rustPackages.checks
        // nixpkgs.lib.optionalAttrs (aarch64DarwinPackages != {}) {
          darwinSignatures = foundation.lib.mkDarwinSignatureCheck {
            inherit pkgs;
            darwinPackages = aarch64DarwinPackages;
          };
        }
        # Run the x86_64 Windows cross binaries under wine to prove they
        # execute, not merely link.  Gated to x86_64-linux: wine cannot exec an
        # aarch64 PE and is unreliable on Apple Silicon, so aarch64 Windows is
        # build-verified only.
        // nixpkgs.lib.optionalAttrs (system == "x86_64-linux") {
          windowsSmoke = foundation.lib.mkWindowsSmokeCheck {
            inherit pkgs;
            windowsPackages = windowsX86Packages;
          };
        };
      devShells.default = pkgs.mkShell {
        # The audio crates (cpal → alsa-sys) need pkg-config and, on Linux,
        # alsa-lib to compile.  Those live in commonArgs for the crane build;
        # reuse them here so `nix develop --command cargo build/test` — the path
        # CI's Test and Clippy jobs take — can build the workspace too.  macOS
        # links CoreAudio and needs neither, so this gap only bit Linux CI.
        nativeBuildInputs = commonArgs.nativeBuildInputs;
        buildInputs =
          commonArgs.buildInputs
          ++ [
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
            # Rolls and checks CHANGELOG.org.  Here so a dev can run
            # `changelog-roller check-additions` locally before opening a PR;
            # CI's changelog job uses the `ci` shell's own copy (via
            # mkCiShell), not this one.
            changelog-roller.packages.${system}.default
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
        # Runtime marker identifying the default dev shell; a compliance check
        # reads it back with `nix eval`.  The `ci` shell carries the same
        # marker with the value "ci".
        RUST_TEMPLATE_SHELL = "default";
      };
      # The shell the reusable CI workflow runs every job through
      # (`nix develop .#ci --command ...`).  Its baseline — the rust
      # toolchain, changelog-roller, and cargo-semver-checks — comes from
      # foundation's mkCiShell; we add the audio build inputs so the cpal
      # crates compile under CI's cargo test/clippy, and set the strict-audio
      # opt-out so the device-less runner skips the audio-device tests.  The
      # interactive `default` shell deliberately omits that opt-out, so a dev
      # box with a sound card runs the full audio suite for real.
      devShells.ci = foundation.lib.mkCiShell {
        inherit pkgs system;
        toolchain = rust;
        buildInputs = commonArgs.buildInputs;
        nativeBuildInputs = commonArgs.nativeBuildInputs;
        sonify_health_tests_strict_audio_device = "false";
      };
    });
  in {
    devShells =
      nixpkgs.lib.mapAttrs (_: p: p.devShells) perSystem;
    packages = nixpkgs.lib.mapAttrs (_: p: p.packages) perSystem;
    apps = nixpkgs.lib.mapAttrs (_: p: p.apps) perSystem;
    checks = nixpkgs.lib.mapAttrs (_: p: p.checks) perSystem;

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
