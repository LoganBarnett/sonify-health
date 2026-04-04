# Darwin (macOS/launchd) module for the sonify-health daemon.
# Exported from the flake as darwinModules.daemon.
# See nixos-daemon.nix for the Linux/systemd equivalent.
#
# Usage:
#
#   inputs.sonify-health.darwinModules.default
#
#   services.sonify-health = {
#     enable = true;
#     heartbeat.slot = 0;
#     heartbeat.checks = [
#       { name = "local"; command = "/path/to/check-lan"; resultMode = "exit-code"; }
#     ];
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

  configFile = tomlFormat.generate "sonify-health.toml" ({
      log_level = cfg.logLevel;
      log_format = cfg.logFormat;
      listen = cfg.listen;
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
        metrics =
          map (m: {
            name = m.name;
            command = m.command;
            result_mode = m.resultMode;
            register = m.register;
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

    listen = lib.mkOption {
      type = lib.types.str;
      default = "/var/run/sonify-health/sonify-health.sock";
      description = ''
        Address for the web server to bind to (daemon mode).  Exposes
        health check, metrics, and mute control endpoints.  Can be a
        Unix socket path or a host:port string.
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
        tri_ratio, saw_ratio, attack_ms, release_ms, boop1_ratio,
        boop2_ratio.
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
          Heartbeat check commands (max 3).  Each maps to one boop in
          the heartbeat pattern.  fping is a good fit — its exit codes
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
        default = "http://127.0.0.1:3000/health";
        example = "http://127.0.0.1:3000/health";
        description = ''
          URL to probe for health.  The agent runs curl against this
          endpoint every 30 seconds and kills the daemon if it fails,
          letting launchd's KeepAlive restart it.
        '';
      };
    };
  };

  config = lib.mkIf cfg.enable {
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
      # Detect whether listen looks like a socket path (starts with /).
      sockDir =
        if lib.hasPrefix "/" cfg.listen
        then dirOf cfg.listen
        else null;
    in
      ''
        ${pkgs.coreutils}/bin/mkdir --parents ${logDir}
        chown ${cfg.user}:${cfg.group} ${logDir}
        chmod 0750 ${logDir}
      ''
      + lib.optionalString (sockDir != null) ''
        ${pkgs.coreutils}/bin/mkdir --parents ${sockDir}
        chown ${cfg.user}:${cfg.group} ${sockDir}
        chmod 0755 ${sockDir}
      '';

    launchd.daemons.sonify-health = {
      serviceConfig = {
        ProgramArguments = [
          "/bin/sh"
          "-c"
          "/bin/wait4path ${cfg.package} && exec ${cfg.package}/bin/sonify-health --config ${configFile} daemon"
        ];
        UserName = cfg.user;
        GroupName = cfg.group;
        RunAtLoad = true;
        KeepAlive = {
          Crashed = true;
          SuccessfulExit = false;
        };
        ThrottleInterval = 30;
        ProcessType = "Background";
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
