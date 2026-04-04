# NixOS (Linux/systemd) module for the sonify-health daemon.
# Exported from the flake as nixosModules.daemon.
# See darwin-daemon.nix for the macOS/launchd equivalent.
#
# Usage:
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

  configFile = tomlFormat.generate "sonify-health.toml" ({
      log_level = cfg.logLevel;
      log_format = cfg.logFormat;
      listen = cfg.listen;
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
      default = "127.0.0.1:3000";
      description = ''
        Address for the web server to bind to (daemon mode).  Exposes
        health check, metrics, and mute control endpoints.
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
      default = "sonify-health";
      description = "System user the daemon runs as.";
    };

    group = lib.mkOption {
      type = lib.types.str;
      default = "sonify-health";
      description = "System group the daemon runs as.";
    };
  };

  config = lib.mkIf cfg.enable {
    users.users.${cfg.user} = {
      isSystemUser = true;
      group = cfg.group;
      description = "sonify-health daemon user";
    };

    users.groups.${cfg.group} = {};

    systemd.services.sonify-health = {
      description = "sonify-health sonification daemon";
      wantedBy = ["multi-user.target"];
      after = ["network.target"];
      # Check commands are executed via "sh -c".  The default systemd PATH
      # on NixOS does not include /bin, so we must ensure a shell is
      # reachable.
      path = ["/bin" pkgs.bash];

      serviceConfig = {
        Type = "notify";
        ExecStart = "${cfg.package}/bin/sonify-health --config ${configFile} daemon";
        User = cfg.user;
        Group = cfg.group;
        SupplementaryGroups = ["audio"];
        WatchdogSec = "30s";
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
