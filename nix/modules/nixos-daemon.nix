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
#     heartbeats = [
#       {
#         name = "gateway";
#         command = "${pkgs.fping}/bin/fping -q -t 4000 -r 1 10.0.0.1";
#         resultMode = "exit-code";
#         notes = [
#           {
#             transition = {
#               type = "discrete";
#               states = [
#                 { threshold = 0.5; patch = "sine"; }
#                 { threshold = 1.01; patch = "alarm"; }
#               ];
#             };
#           }
#         ];
#       }
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

  # The TOML config file carries heartbeat and patch settings.
  # Listen address is passed via --listen on the command line so the
  # NixOS module retains structured socket/host/port options.
  configFile = tomlFormat.generate "sonify-health.toml" ({
      log_level = cfg.logLevel;
      log_format = cfg.logFormat;
    }
    // lib.optionalAttrs (cfg.audioDevice != null) {
      audio_device = cfg.audioDevice;
    }
    // lib.optionalAttrs (cfg.patches != {}) {
      patches = cfg.patches;
    }
    // lib.optionalAttrs (cfg.sliderRanges != {}) {
      slider_ranges = cfg.sliderRanges;
    }
    // lib.optionalAttrs (cfg.heartbeats != []) {
      heartbeats = map (hb:
        {
          name = hb.name;
          command = hb.command;
          result_mode = hb.resultMode;
          notes = map (n:
            {
              transition = n.transition;
            }
            // lib.optionalAttrs (n.volume != 0.3) {
              volume = n.volume;
            }
            // lib.optionalAttrs (n.offset != 0.0) {
              offset = n.offset;
            })
          hb.notes;
        }
        // {
          playback = hb.playback;
          cycle_offset_secs = hb.cycleOffsetSecs;
          crossfade_ms = hb.crossfadeMs;
        }
        // lib.optionalAttrs (hb.phraseGap != 0.0) {
          phrase_gap = hb.phraseGap;
        }
        // lib.optionalAttrs (hb.repeatRate != 1.0) {
          repeat_rate = hb.repeatRate;
        }
        // lib.optionalAttrs (hb.pollIntervalSecs != 10.0) {
          poll_interval_secs = hb.pollIntervalSecs;
        }
        // lib.optionalAttrs (hb.cycleSecs != 14.0) {
          cycle_secs = hb.cycleSecs;
        })
      cfg.heartbeats;
    });

  noteSubmodule = lib.types.submodule {
    options = {
      transition = lib.mkOption {
        type = lib.types.attrsOf lib.types.anything;
        description = ''
          Transition mapping from probe metric to patches.  Either:
            { type = "discrete"; states = [{ threshold = 0.5; patch = "sine"; } ...]; }
          or:
            { type = "gradient"; patches = ["warm" "sharp" "alarm"];
              segments = [
                { strategy = "ease-in"; intensity = 2.0; }
                { strategy = "linear"; intensity = 2.0; }
              ];
            }
          Each segment controls the interpolation curve between a pair of
          adjacent patches.  Omit segments for all-linear interpolation.
        '';
      };

      volume = lib.mkOption {
        type = lib.types.number;
        default = 0.3;
        description = "Output volume for this note (0.0-1.0).";
      };

      offset = lib.mkOption {
        type = lib.types.number;
        default = 0.0;
        description = "Seconds from heartbeat start when this note plays.";
      };
    };
  };

  heartbeatSubmodule = lib.types.submodule {
    options = {
      name = lib.mkOption {
        type = lib.types.str;
        description = "Human-readable name for this heartbeat.";
      };

      command = lib.mkOption {
        type = lib.types.str;
        description = "Shell command that produces a probe metric.";
      };

      resultMode = lib.mkOption {
        type = lib.types.enum ["exit-code" "stdout"];
        default = "exit-code";
        description = ''
          How to read the command result.  "exit-code" maps exit 0 to 0.0
          and non-zero to 1.0.  "stdout" reads a float from stdout.
        '';
      };

      notes = lib.mkOption {
        type = lib.types.listOf noteSubmodule;
        description = ''
          Notes for this heartbeat.  Each note has its own transition,
          volume, and offset from the heartbeat start.
        '';
      };

      playback = lib.mkOption {
        type = lib.types.enum ["clock" "loop" "continuous"];
        default = "clock";
        description = ''
          Playback mode.  "clock" fires once per cycle, "loop" repeats
          the phrase back-to-back, "continuous" sustains a drone.
        '';
      };

      cycleOffsetSecs = lib.mkOption {
        type = lib.types.number;
        default = 0.0;
        description = "Seconds to offset this heartbeat within the cycle.";
      };

      crossfadeMs = lib.mkOption {
        type = lib.types.number;
        default = 0.0;
        description = "Crossfade duration in milliseconds between patches.";
      };

      phraseGap = lib.mkOption {
        type = lib.types.number;
        default = 0.0;
        description = "Seconds of silence between phrase repetitions (continuous mode).";
      };

      repeatRate = lib.mkOption {
        type = lib.types.number;
        default = 1.0;
        description = "Speed multiplier on phrase repetition.";
      };

      pollIntervalSecs = lib.mkOption {
        type = lib.types.number;
        default = 10.0;
        description = "Seconds between probe command executions.";
      };

      cycleSecs = lib.mkOption {
        type = lib.types.number;
        default = 14.0;
        description = "Seconds between plays for one-shot heartbeats.";
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

    # host and port are separate options (rather than a single "listen"
    # string) so that other Nix expressions can reference them
    # individually — e.g. firewall rules need the port, reverse proxy
    # configs need host:port, and health-check URLs need both.  The
    # module combines them into the --listen flag internally.
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

    patches = lib.mkOption {
      type = lib.types.attrsOf (lib.types.attrsOf lib.types.anything);
      default = {};
      example = {
        gateway-ok = {
          freq = 523.0;
          duration = 0.4;
        };
        gateway-bad = {
          freq = 220.0;
          saw_ratio = 1.0;
          sine_ratio = 0.0;
        };
      };
      description = ''
        Named patch definitions.  Each patch is a set of parameter
        overrides; unspecified fields use Patch::default().  Built-in
        patches (sine, bell, warm, sharp, etc.) are always available
        and can be overridden here.
      '';
    };

    sliderRanges = lib.mkOption {
      type = lib.types.attrsOf (lib.types.submodule {
        options = {
          min = lib.mkOption {
            type = lib.types.number;
            description = "Minimum slider value.";
          };
          max = lib.mkOption {
            type = lib.types.number;
            description = "Maximum slider value.";
          };
          step = lib.mkOption {
            type = lib.types.number;
            description = "Slider step increment.";
          };
        };
      });
      default = {};
      example = {
        cycle_offset = {
          min = 0.0;
          max = 120.0;
          step = 0.1;
        };
      };
      description = ''
        Override slider ranges for the web UI.  Each key is a slider
        name (master_volume, cycle_offset, override_metric,
        note_volume, note_offset, segment_intensity, discrete_threshold,
        step_position)
        and must provide min, max, and step.  Omitted sliders keep
        their built-in defaults.
      '';
    };

    heartbeats = lib.mkOption {
      type = lib.types.listOf heartbeatSubmodule;
      default = [];
      example = lib.literalExpression ''
        [
          {
            name = "lan";
            command = "''${pkgs.fping}/bin/fping -q -t 4000 -r 1 10.0.0.1 10.0.0.2";
            resultMode = "exit-code";
            notes = [
              {
                transition = {
                  type = "discrete";
                  states = [
                    { threshold = 0.5; patch = "sine"; }
                    { threshold = 1.01; patch = "alarm"; }
                  ];
                };
              }
            ];
          }
          {
            name = "cpu";
            command = "sh -c 'uptime | awk ...'";
            resultMode = "stdout";
            playback = "continuous";
            notes = [
              {
                volume = 0.2;
                transition = {
                  type = "gradient";
                  patches = ["warm" "sharp" "alarm"];
                  segments = [
                    { strategy = "ease-in"; intensity = 2.0; }
                    { strategy = "linear"; intensity = 2.0; }
                  ];
                };
              }
            ];
          }
        ]
      '';
      description = ''
        Heartbeat definitions.  Each heartbeat joins a probe command
        with one or more notes, each mapping the probe metric (0.0-1.0)
        to patches from the library.
      '';
    };

    oidc = {
      enable = lib.mkEnableOption "OIDC authentication for the web UI and API";

      baseUrl = lib.mkOption {
        type = lib.types.nullOr lib.types.str;
        default = null;
        example = "https://sonify.example.com";
        description = ''
          Public base URL of the service, used to construct the OIDC
          redirect URI (base_url + /auth/callback).  Set all three OIDC
          options or leave all three null for unauthenticated admin mode.
        '';
      };

      issuer = lib.mkOption {
        type = lib.types.nullOr lib.types.str;
        default = null;
        example = "https://sso.example.com/application/o/sonify-health/";
        description = ''
          OIDC issuer URL for provider discovery.  Set all three OIDC
          options or leave all three null for unauthenticated admin mode.
        '';
      };

      clientId = lib.mkOption {
        type = lib.types.nullOr lib.types.str;
        default = null;
        description = ''
          OIDC client ID.  Set all three OIDC options or leave all three
          null for unauthenticated admin mode.
        '';
      };

      clientSecretFile = lib.mkOption {
        type = lib.types.nullOr lib.types.path;
        default = null;
        example = "/run/secrets/sonify-health-oidc";
        description = ''
          Path to a file containing the OIDC client secret.  The module
          loads this via systemd's LoadCredential, so the service user
          does not need direct read access to the file.  Set all three
          OIDC options or leave all three null for unauthenticated admin
          mode.
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
    assertions = [
      {
        assertion = let
          oidcFields = [cfg.oidc.issuer cfg.oidc.clientId cfg.oidc.clientSecretFile];
          setCount = lib.count (x: x != null) oidcFields;
        in
          !cfg.oidc.enable || setCount == 3;
        message = ''
          services.sonify-health: OIDC is enabled but configuration is
          incomplete.  Set all three of oidc.issuer, oidc.clientId, and
          oidc.clientSecretFile when oidc.enable is true.
        '';
      }
    ];

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

      # Probe commands are executed via "sh -c".  The default systemd
      # PATH on NixOS does not include /bin, so we must ensure a shell
      # is reachable.
      path = ["/bin" pkgs.bash];

      environment =
        {}
        // lib.optionalAttrs cfg.oidc.enable {
          BASE_URL = cfg.oidc.baseUrl;
          OIDC_ISSUER = cfg.oidc.issuer;
          OIDC_CLIENT_ID = cfg.oidc.clientId;
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

        LoadCredential =
          lib.mkIf (cfg.oidc.clientSecretFile != null)
          "oidc-client-secret:${cfg.oidc.clientSecretFile}";

        User = cfg.user;
        Group = cfg.group;
        SupplementaryGroups = lib.mkDefault (
          ["audio"]
          ++ lib.optional pipewireEnabled "pipewire"
        );
        Restart = "on-failure";
        RestartSec = "5s";

        # Allow the cpal audio thread to use real-time scheduling
        # (SCHED_FIFO).  Without this the request fails silently and
        # the callback thread runs at normal priority.
        LimitRTPRIO = "99";

        # Hardening.
        NoNewPrivileges = true;
        PrivateTmp = true;
        ProtectSystem = "strict";
        ProtectHome = true;
      };
    };
  };
}
