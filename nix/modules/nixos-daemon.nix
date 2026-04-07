# NixOS (Linux/systemd) module for the sonify-health daemon.
# Exported from the flake as nixosModules.daemon.
# See darwin-daemon.nix for the macOS/launchd equivalent.
#
# Minimal usage (defaults to Unix domain socket with socket activation):
#
#   inputs.sonify-health.nixosModules.default
#
#   services.sonify-health = {
#     enable = true;
#     heartbeat.slot = 0;
#     heartbeat.checks = [
#       { name = "local"; command = "/path/to/check-lan"; resultMode = "exit-code"; }
#     ];
#   };
#
# To use TCP instead:
#
#   services.sonify-health = {
#     enable = true;
#     socket = null;
#     port   = 3000;
#   };
#
# To reference the socket from a reverse proxy (e.g. nginx):
#
#   locations."/".proxyPass =
#     "http://unix:${config.services.sonify-health.socket}";
#
# Note: when using socket mode the reverse proxy user must be a member of
# the service group (cfg.group) so it can connect to the socket.
#
# fping works well as a check command — its exit codes map directly to
# healthy (0), degraded (1, some unreachable), and down (2, all unreachable):
#
#   services.sonify-health.heartbeat.checks = [
#     {
#       name = "lan";
#       command = "${pkgs.fping}/bin/fping -q -t 4000 -r 1 10.0.0.1 10.0.0.2";
#     }
#     {
#       name = "wan";
#       command = "${pkgs.fping}/bin/fping -q -t 4000 -r 1 8.8.8.8 1.1.1.1";
#     }
#     {
#       name = "dns";
#       command = "${pkgs.fping}/bin/fping -q -t 4000 -r 1 google.com github.com";
#     }
#   ];
{self}: {
  config,
  lib,
  pkgs,
  ...
}: let
  cfg = config.services.sonify-health;
  tomlFormat = pkgs.formats.toml {};

  # Detect PipeWire so we can add the service user to the pipewire group
  # automatically, giving ALSA clients access to the PipeWire socket.
  pipewireEnabled = config.services.pipewire.enable or false;

  # The TOML config file carries heartbeat/drone/voice settings.
  # Listen address is passed via --listen on the command line so the
  # NixOS module retains structured socket/host/port options.
  configFile = tomlFormat.generate "sonify-health.toml" ({
      log_level = cfg.logLevel;
      log_format = cfg.logFormat;
    }
    // lib.optionalAttrs (cfg.audioDevice != null) {
      audio_device = cfg.audioDevice;
    }
    // {
      heartbeat =
        {
          slot = cfg.heartbeat.slot;
          cycle_duration_secs = cfg.heartbeat.cycleDurationSecs;
          slot_duration_secs = cfg.heartbeat.slotDurationSecs;
        }
        // lib.optionalAttrs (cfg.heartbeat.checks != []) {
          checks =
            map (c: {
              name = c.name;
              command = c.command;
              result_mode = c.resultMode;
            })
            cfg.heartbeat.checks;
        };
    }
    // lib.optionalAttrs (cfg.voice != {}) {
      voice = cfg.voice;
    }
    // lib.optionalAttrs (cfg.drone.metrics != []) {
      drone = {
        poll_interval_secs = cfg.drone.pollIntervalSecs;
        metrics = map (m:
          {
            name = m.name;
            command = m.command;
            result_mode = m.resultMode;
            register = m.register;
          }
          // lib.optionalAttrs (m.texture != null) {
            texture = m.texture;
          })
        cfg.drone.metrics;
      };
    });

  droneMetricSubmodule = lib.types.submodule {
    options = {
      name = lib.mkOption {
        type = lib.types.str;
        description = "Human-readable name for this drone metric.";
      };

      command = lib.mkOption {
        type = lib.types.str;
        description = "Shell command that outputs a 0.0..1.0 metric value.";
      };

      resultMode = lib.mkOption {
        type = lib.types.enum ["exit-code" "stdout"];
        default = "stdout";
        description = ''
          How to read the command result.  "exit-code" maps the exit code
          (0..255) linearly to 0.0..1.0.  "stdout" reads a float from stdout.
        '';
      };

      register = lib.mkOption {
        type = lib.types.enum ["low" "mid" "high"];
        default = "mid";
        description = ''
          Pitch register for the drone voice.  "low" = half base frequency,
          "mid" = base frequency, "high" = double base frequency.
        '';
      };

      texture = lib.mkOption {
        type = lib.types.nullOr (lib.types.enum ["bong" "arpeggio" "thrum" "shimmer" "reactor" "warpcore"]);
        default = null;
        description = ''
          Drone texture.  "bong" = periodic bell strikes, "arpeggio" = cycling
          pentatonic notes, "thrum" = continuous tremolo, "shimmer" = detuned
          beating, "reactor" = deep pulsing power hum, "warpcore" = rhythmic
          spectral sweep.  When null, auto-assigned by metric position.
        '';
      };
    };
  };

  checkSubmodule = lib.types.submodule {
    options = {
      name = lib.mkOption {
        type = lib.types.str;
        description = "Human-readable name for this check.";
      };

      command = lib.mkOption {
        type = lib.types.str;
        description = "Shell command to execute.";
      };

      resultMode = lib.mkOption {
        type = lib.types.enum ["exit-code" "stdout"];
        default = "exit-code";
        description = ''
          How to read the command result.  "exit-code" maps exit codes
          0/1/2 to healthy/degraded/down.  "stdout" reads the severity
          value printed to stdout.
        '';
      };
    };
  };
in {
  options.services.sonify-health = {
    enable = lib.mkEnableOption "sonify-health sonification daemon";

    package = lib.mkOption {
      type = lib.types.package;
      default = self.packages.${pkgs.stdenv.hostPlatform.system}.cli;
      defaultText = lib.literalExpression "self.packages.\${system}.cli";
      description = "Package providing the sonify-health binary.";
    };

    logLevel = lib.mkOption {
      type = lib.types.enum ["trace" "debug" "info" "warn" "error"];
      default = "info";
      description = "Tracing log verbosity level.";
    };

    logFormat = lib.mkOption {
      type = lib.types.enum ["text" "json"];
      default = "json";
      description = ''
        Log output format.  "text" for human-readable local logs, "json"
        for structured logs consumed by a log aggregator.
      '';
    };

    socket = lib.mkOption {
      type = lib.types.nullOr lib.types.path;
      default = "/run/sonify-health/sonify-health.sock";
      description = ''
        Path for the Unix domain socket used by the service.  When set,
        systemd socket activation is used and the host/port options are
        ignored.  Set to null to use TCP instead.

        Other services (e.g. nginx) that proxy to this socket must be
        members of the service group to connect.
      '';
    };

    host = lib.mkOption {
      type = lib.types.str;
      default = "127.0.0.1";
      description = "IP address to bind to.  Ignored when socket is set.";
    };

    port = lib.mkOption {
      type = lib.types.port;
      default = 3000;
      description = "TCP port to listen on.  Ignored when socket is set.";
    };

    openFirewall = lib.mkOption {
      type = lib.types.bool;
      default = false;
      description = ''
        Whether to open the configured port in the NixOS firewall.
        Only effective when socket is null (TCP mode).
      '';
    };

    audioDevice = lib.mkOption {
      type = lib.types.nullOr lib.types.str;
      default = null;
      example = "speakers";
      description = ''
        Audio output device name (case-insensitive substring match).
        When null, the system default output device is used.
      '';
    };

    voice = lib.mkOption {
      type = lib.types.attrsOf lib.types.anything;
      default = {};
      example = {
        base_freq = 523.0;
        sine_ratio = 0.8;
      };
      description = ''
        Hostname-derived voice parameter overrides.  Unspecified fields
        use the hash-derived default.  Available: base_freq, sine_ratio,
        tri_ratio, saw_ratio, attack_ms, release_ms, chirp_ratio,
        stereo_pan, reverb_mix.
      '';
    };

    heartbeat = {
      slot = lib.mkOption {
        type = lib.types.ints.unsigned;
        default = 0;
        description = "Zero-indexed slot in the timing cycle.";
      };

      cycleDurationSecs = lib.mkOption {
        type = lib.types.number;
        default = 14;
        description = "Total cycle duration covering all machines (seconds).";
      };

      slotDurationSecs = lib.mkOption {
        type = lib.types.number;
        default = 2;
        description = "Time budget per machine within the cycle (seconds).";
      };

      checks = lib.mkOption {
        type = lib.types.listOf checkSubmodule;
        default = [];
        example = lib.literalExpression ''
          [
            {
              name = "lan";
              command = "''${pkgs.fping}/bin/fping -q -t 4000 -r 1 10.0.0.1 10.0.0.2";
            }
            {
              name = "wan";
              command = "''${pkgs.fping}/bin/fping -q -t 4000 -r 1 8.8.8.8 1.1.1.1";
            }
            {
              name = "dns";
              command = "''${pkgs.fping}/bin/fping -q -t 4000 -r 1 google.com github.com";
            }
          ]
        '';
        description = ''
          Heartbeat check commands.  Each maps to one boop in the
          heartbeat pattern.  fping is a good fit — its exit codes
          (0/1/2 = all/some/none reachable) map directly to the
          healthy/degraded/down severities.
        '';
      };
    };

    drone = {
      pollIntervalSecs = lib.mkOption {
        type = lib.types.number;
        default = 5;
        description = "How often to run drone metric commands (seconds).";
      };

      metrics = lib.mkOption {
        type = lib.types.listOf droneMetricSubmodule;
        default = [];
        example = lib.literalExpression ''
          [
            {
              name = "gpu";
              command = "/path/to/gpu-load";
              register = "low";
            }
            {
              name = "memory";
              command = "/path/to/mem-pressure";
              register = "mid";
            }
          ]
        '';
        description = ''
          Drone metric commands.  Each metric drives a continuous audio
          stream whose timbre and volume shift with the reported value.
        '';
      };
    };

    oidc = {
      enable = lib.mkEnableOption "OIDC authentication for the web UI and API";

      baseUrl = lib.mkOption {
        type = lib.types.str;
        default = "";
        example = "https://sonify.example.com";
        description = ''
          Public base URL of the service, used to construct the OIDC
          redirect URI (base_url + /auth/callback).
        '';
      };

      issuer = lib.mkOption {
        type = lib.types.str;
        default = "";
        example = "https://sso.example.com/application/o/sonify-health/";
        description = "OIDC issuer URL for provider discovery.";
      };

      clientId = lib.mkOption {
        type = lib.types.str;
        default = "";
        description = "OIDC client ID.";
      };

      clientSecretFile = lib.mkOption {
        type = lib.types.path;
        example = "/run/secrets/sonify-health-oidc";
        description = ''
          Path to a file containing the OIDC client secret.  The file
          is read at daemon startup.
        '';
      };
    };

    user = lib.mkOption {
      type = lib.types.str;
      default = "sonify-health";
      description = "System user the daemon runs as.";
    };

    group = lib.mkOption {
      type = lib.types.str;
      default = "sonify-health";
      description = "System group the daemon runs as.";
    };

    frontendPath = lib.mkOption {
      type = lib.types.str;
      default = "${cfg.package}/share/sonify-health/frontend";
      defaultText = lib.literalExpression ''"''${cfg.package}/share/sonify-health/frontend"'';
      description = ''
        Path to the compiled Elm frontend assets directory.  The default
        points at the Nix store output from the cli package build.
      '';
    };
  };

  config = lib.mkIf cfg.enable {
    users.users.${cfg.user} = {
      isSystemUser = true;
      group = cfg.group;
      description = "sonify-health daemon user";
    };

    users.groups.${cfg.group} = {};

    # Open the TCP port when using host:port mode with openFirewall.
    networking.firewall.allowedTCPPorts =
      lib.mkIf (cfg.openFirewall && cfg.socket == null) [cfg.port];

    # Create the socket directory before the socket unit tries to bind.
    systemd.tmpfiles.rules = lib.mkIf (cfg.socket != null) [
      "d ${dirOf cfg.socket} 0750 ${cfg.user} ${cfg.group} -"
    ];

    # Socket unit: systemd creates and holds the Unix domain socket, then
    # passes the open file descriptor to the service on first activation.
    systemd.sockets.sonify-health = lib.mkIf (cfg.socket != null) {
      description = "sonify-health Unix domain socket";
      wantedBy = ["sockets.target"];
      socketConfig = {
        ListenStream = cfg.socket;
        SocketUser = cfg.user;
        SocketGroup = cfg.group;
        # 0660: accessible to the service user and group only.  Add the
        # reverse proxy user to cfg.group to grant it access.
        SocketMode = "0660";
        Accept = false;
      };
    };

    systemd.services.sonify-health = {
      description = "sonify-health sonification daemon";
      wantedBy = ["multi-user.target"];
      after =
        ["network.target"]
        ++ lib.optional (cfg.socket != null) "sonify-health.socket";
      requires =
        lib.optional (cfg.socket != null) "sonify-health.socket";

      # Check commands are executed via "sh -c".  The default systemd PATH
      # on NixOS does not include /bin, so we must ensure a shell is
      # reachable.
      path = ["/bin" pkgs.bash];

      environment =
        {}
        // lib.optionalAttrs cfg.oidc.enable {
          BASE_URL = cfg.oidc.baseUrl;
          OIDC_ISSUER = cfg.oidc.issuer;
          OIDC_CLIENT_ID = cfg.oidc.clientId;
          OIDC_CLIENT_SECRET_FILE = cfg.oidc.clientSecretFile;
        };

      serviceConfig = {
        Type = "notify";
        NotifyAccess = "main";

        WatchdogSec = lib.mkDefault "30s";

        ExecStart =
          "${cfg.package}/bin/sonify-health"
          + " --config ${configFile}"
          + (
            if cfg.socket != null
            then " --listen sd-listen"
            else " --listen ${cfg.host}:${toString cfg.port}"
          )
          + " --frontend-path ${cfg.frontendPath}"
          + " daemon";

        User = cfg.user;
        Group = cfg.group;
        SupplementaryGroups = lib.mkDefault (
          ["audio"]
          ++ lib.optional pipewireEnabled "pipewire"
        );
        Restart = "on-failure";
        RestartSec = "5s";

        # Hardening.
        NoNewPrivileges = true;
        PrivateTmp = true;
        ProtectSystem = "strict";
        ProtectHome = true;
      };
    };
  };
}
