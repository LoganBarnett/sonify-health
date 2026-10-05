# Darwin (macOS/launchd) module for the sonify-health daemon.
# Exported from the flake as darwinModules.server.
# See nixos-server.nix for the Linux/systemd equivalent.
#
# The service itself comes from the service helper in ../lib, and the options
# both platforms share live in ./common.nix.  This file holds what only
# sonify-health needs on macOS.
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
# A health check runs by default and restarts the daemon when /healthz stops
# answering.  To turn it off:
#
#   services.sonify-health = {
#     enable = true;
#     healthCheck.enable = false;
#   };
#
# Note on macOS audio: launchd system daemons run outside any user session,
# which can prevent CoreAudio access.  If audio fails, switch to a
# launchd user agent (launchd.user.agents) or grant the daemon user
# access to the audio session.
{self}: {
  config,
  lib,
  ...
}: let
  cfg = config.services.sonify-health;
in {
  imports = [
    ./common.nix
    (import ../lib/mkDarwinService.nix {
      name = "sonify-health";
      inherit self;
    })
  ];

  config = lib.mkMerge [
    {
      # The service helper gives every service the same default UID and GID
      # (401), so two helper-based services on one host would collide.  402 is
      # also what hosts already running sonify-health created the account
      # with.
      services.sonify-health.uid = lib.mkDefault 402;
      services.sonify-health.gid = lib.mkDefault 402;
    }

    (lib.mkIf cfg.enable {
      launchd.daemons.sonify-health.serviceConfig = {
        # macOS parks a Background process on efficiency cores, which starves
        # the audio thread into silence, stuttering, or crackling.
        # Interactive keeps the daemon off them.
        ProcessType = "Interactive";

        EnvironmentVariables.sonify_health_config =
          toString cfg._generatedConfigFile;
      };
    })
  ];
}
