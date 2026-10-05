# Options and generated configuration that the NixOS and nix-darwin modules
# for sonify-health share.  Both platform modules import this file next to the
# service helper, so anything declared here merges into each of them.
{
  config,
  lib,
  pkgs,
  ...
}: let
  cfg = config.services.sonify-health;
  tomlFormat = pkgs.formats.toml {};

  # Heartbeats and patches are nested tables, which environment variables
  # cannot carry, so they are rendered to a TOML file.  The listen address and
  # the log settings are absent from it: the service helper passes those
  # itself.
  configFile = tomlFormat.generate "sonify-health.toml" (
    lib.optionalAttrs (cfg.audioDevice != null) {
      audio_device = cfg.audioDevice;
    }
    // lib.optionalAttrs cfg.headless {
      headless = true;
    }
    // lib.optionalAttrs (cfg.sources != []) {
      sources =
        map (s: {
          name = s.name;
          url = s.url;
          playback_enabled = s.playbackEnabled;
        })
        cfg.sources;
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
    }
  );

  sourceSubmodule = lib.types.submodule {
    options = {
      name = lib.mkOption {
        type = lib.types.str;
        example = "prod-db-1";
        description = ''
          Display name and unique identifier for this Remote Source.
          Must be unique across all sources and cannot be `localhost`,
          which is reserved for the Local Source.  Conventionally
          defaults to the URL's hostname when added through the UI;
          a friendlier label (e.g. `prod-db-1`) is preferred when
          managing it through this module.
        '';
      };

      url = lib.mkOption {
        type = lib.types.str;
        example = "wss://db1.internal.example.com/ws";
        description = ''
          WebSocket URL of the remote sonify-health instance.  Use
          `ws://` for plaintext (trusted networks only) or `wss://`
          for TLS.
        '';
      };

      playbackEnabled = lib.mkOption {
        type = lib.types.bool;
        default = false;
        description = ''
          Whether the local renderer plays audio for this remote's
          heartbeats.  Defaults to false: even with the source
          declared, audio stays silent until enabled (the laptop
          versus office switch — listen on the laptop, mute when
          back at the desk so the office speakers don't double up).
        '';
      };
    };
  };

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

    sources = lib.mkOption {
      type = lib.types.listOf sourceSubmodule;
      default = [];
      example = [
        {
          name = "prod-db-1";
          url = "wss://db1.internal.example.com/ws";
          playbackEnabled = false;
        }
      ];
      description = ''
        Remote sonify-health instances whose state this daemon
        subscribes to and (when playbackEnabled = true) plays audio
        for.  Each entry creates an outbound WebSocket connector.

        Names must be unique across the list; `localhost` is reserved
        for the Local Source.
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
          Path to a file containing the OIDC client secret.  On NixOS the
          module loads it through systemd's LoadCredential, so the service
          user needs no direct read access; on nix-darwin the service user
          reads the file itself.  Set all three OIDC options or leave all
          three null for unauthenticated admin mode.
        '';
      };
    };

    _generatedConfigFile = lib.mkOption {
      type = lib.types.path;
      internal = true;
      readOnly = true;
      description = ''
        Path to the generated TOML configuration.  Each platform module
        hands it to the daemon through `sonify_health_config`.
      '';
    };
  };

  config = lib.mkIf cfg.enable (lib.mkMerge [
    {
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

      services.sonify-health._generatedConfigFile = configFile;
    }

    # The service helper declares its OIDC settings as flat options, while
    # adopters of this module set the nested `oidc.*` group.  Forwarding one
    # into the other keeps that surface and lets the helper emit the
    # environment and, on NixOS, the LoadCredential entry.
    (lib.mkIf cfg.oidc.enable {
      services.sonify-health = {
        baseUrl = cfg.oidc.baseUrl;
        oidcIssuer = cfg.oidc.issuer;
        oidcClientId = cfg.oidc.clientId;
        oidcClientSecretFile = cfg.oidc.clientSecretFile;
      };
    })
  ]);
}
