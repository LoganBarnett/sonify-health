# Darwin (macOS/launchd) module for the sonify-health daemon.
# Exported from the flake as darwinModules.daemon.
# See nixos-daemon.nix for the Linux/systemd equivalent.
#
# Minimal usage (defaults to Unix domain socket):
#
#   inputs.sonify-health.darwinModules.default
#
#   services.sonify-health = {
#     enable = true;
#     heartbeats = [
#       {
#         name = "gateway";
#         command = "/path/to/check-lan";
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
# Note on macOS audio: launchd system daemons run outside any user session,
# which can prevent CoreAudio access.  If audio fails, switch to a
# launchd user agent (launchd.user.agents) or grant the daemon user
# access to the audio session.
{self}: {
  config,
  lib,
  pkgs,
  ...
}: let
  cfg = config.services.sonify-health;
  tomlFormat = pkgs.formats.toml {};

  listenArg =
    if cfg.socket != null
    then "--listen unix:${cfg.socket}"
    else "--listen ${cfg.host}:${toString cfg.port}";

  execLine =
    "${cfg.package}/bin/sonify-health"
    + " --config ${configFile}"
    + " ${listenArg}"
    + " --frontend-path ${cfg.frontendPath}"
    + " daemon";

  # The TOML config file carries heartbeat and patch settings.
  # Listen address is passed via --listen on the command line so the
  # module retains structured socket/host/port options.
  configFile = tomlFormat.generate "sonify-health.toml" ({
      log_level = cfg.logLevel;
      log_format = cfg.logFormat;
    }
    // lib.optionalAttrs (cfg.audioDevice != null) {
      audio_device = cfg.audioDevice;
    }
    // lib.optionalAttrs cfg.headless {
      headless = true;
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
      default = "/var/run/sonify-health/sonify-health.sock";
      description = ''
        Path for the Unix domain socket used by the service.  When set,
        the daemon binds its own socket (no launchd socket activation) and
        the host/port options are ignored.  Set to null to use TCP instead.
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

    audioDevice = lib.mkOption {
      type = lib.types.nullOr lib.types.str;
      default = null;
      example = "speakers";
      description = ''
        Audio output device name (case-insensitive substring match).
        When null, the system default output device is used.
      '';
    };

    headless = lib.mkOption {
      type = lib.types.bool;
      default = false;
      example = true;
      description = ''
        Run the daemon without opening an audio device.  Heartbeat
        commands still execute on schedule and the WebSocket / metrics
        endpoints stay live, but no audio is produced and no play
        threads are spawned.

        Intended for servers without speakers whose state will be
        rendered remotely by another sonify-health instance subscribed
        to this one.  Mutually compatible with audioDevice (audioDevice
        is simply ignored when headless = true).
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
          Path to a file containing the OIDC client secret.  Set all
          three OIDC options or leave all three null for unauthenticated
          admin mode.
        '';
      };
    };

    user = lib.mkOption {
      type = lib.types.str;
      default = "_sonify-health";
      description = ''
        System user account the daemon runs as.  The leading underscore
        follows the macOS convention for daemon accounts.
      '';
    };

    group = lib.mkOption {
      type = lib.types.str;
      default = "_sonify-health";
      description = ''
        System group the daemon runs as.  The leading underscore follows
        the macOS convention for daemon groups.
      '';
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

    uid = lib.mkOption {
      type = lib.types.int;
      default = 402;
      description = ''
        UID for the service user.  nix-darwin requires a static UID for
        user creation.  The default (402) sits above macOS Sequoia's
        claimed 300-304 range and below the 501 normal-user boundary.
      '';
    };

    gid = lib.mkOption {
      type = lib.types.int;
      default = 402;
      description = ''
        GID for the service group.  nix-darwin requires a static GID for
        group creation.  The default (402) mirrors the UID choice.
      '';
    };

    healthCheck = {
      enable = lib.mkEnableOption "periodic health-check agent for the daemon";

      url = lib.mkOption {
        type = lib.types.str;
        default = "http://127.0.0.1:${toString cfg.port}/healthz";
        defaultText = lib.literalExpression ''"http://127.0.0.1:''${toString cfg.port}/healthz"'';
        example = "http://127.0.0.1:3000/healthz";
        description = ''
          URL to probe for health.  The agent runs curl against this
          endpoint every 30 seconds and kills the daemon if it fails,
          letting launchd's KeepAlive restart it.
        '';
      };
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
      uid = cfg.uid;
      gid = cfg.gid;
      home = "/var/empty";
      shell = "/usr/bin/false";
      description = "sonify-health daemon user";
      isHidden = true;
    };

    users.groups.${cfg.group} = {
      gid = cfg.gid;
      members = [cfg.user];
    };

    users.knownUsers = [cfg.user];
    users.knownGroups = [cfg.group];

    # Create log and socket directories.  macOS has no tmpfiles equivalent,
    # so we use nix-darwin activation scripts.
    system.activationScripts.postActivation.text = let
      logDir = "/var/log/sonify-health";
      sockDir =
        if cfg.socket != null
        then dirOf cfg.socket
        else null;
    in
      ''
        mkdir -p ${logDir}
        chown ${cfg.user}:${cfg.group} ${logDir}
        chmod 0750 ${logDir}
      ''
      + lib.optionalString (sockDir != null) ''
        mkdir -p ${sockDir}
        chown ${cfg.user}:${cfg.group} ${sockDir}
        chmod 0750 ${sockDir}
      '';

    launchd.daemons.sonify-health = {
      serviceConfig = {
        ProgramArguments = [
          "/bin/sh"
          "-c"
          "/bin/wait4path ${cfg.package} && exec ${execLine}"
        ];
        UserName = cfg.user;
        GroupName = cfg.group;
        RunAtLoad = true;
        KeepAlive = {
          Crashed = true;
          SuccessfulExit = false;
        };
        ThrottleInterval = 30;
        # Interactive prevents macOS from parking the process onto
        # efficiency cores.  Without it the audio thread starves and
        # produces silence, stuttering, or crackling.
        ProcessType = "Interactive";
        EnvironmentVariables =
          {}
          // lib.optionalAttrs cfg.oidc.enable {
            BASE_URL = cfg.oidc.baseUrl;
            OIDC_ISSUER = cfg.oidc.issuer;
            OIDC_CLIENT_ID = cfg.oidc.clientId;
            OIDC_CLIENT_SECRET_FILE = cfg.oidc.clientSecretFile;
          };
        # Darwin has no LoadCredential equivalent, so the secret file
        # path is passed directly via OIDC_CLIENT_SECRET_FILE above.
        StandardOutPath = "/var/log/sonify-health/stdout.log";
        StandardErrorPath = "/var/log/sonify-health/stderr.log";
      };
    };

    # Optional health-check agent.  Probes the daemon's health endpoint
    # every 30 seconds and kills the daemon process on failure, letting
    # launchd's KeepAlive trigger a restart.
    launchd.daemons.sonify-health-healthcheck = lib.mkIf cfg.healthCheck.enable {
      serviceConfig = {
        ProgramArguments = [
          "/bin/sh"
          "-c"
          ''/usr/bin/curl -sf ${cfg.healthCheck.url} || /bin/kill $(/bin/cat /var/run/sonify-health/pid) 2>/dev/null''
        ];
        StartInterval = 30;
        RunAtLoad = false;
        ProcessType = "Background";
        StandardOutPath = "/var/log/sonify-health/healthcheck-stdout.log";
        StandardErrorPath = "/var/log/sonify-health/healthcheck-stderr.log";
      };
    };
  };
}
