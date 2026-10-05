# NixOS (Linux/systemd) module for the sonify-health daemon.
# Exported from the flake as nixosModules.server.
# See darwin-server.nix for the macOS/launchd equivalent.
#
# The service itself comes from the service helper in ../lib, and the options
# both platforms share live in ./common.nix.  This file holds what only
# sonify-health needs on NixOS.
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

  # Detect PipeWire so we can add the service user to the pipewire group
  # automatically, giving ALSA clients access to the PipeWire socket.
  pipewireEnabled = config.services.pipewire.enable or false;
in {
  imports = [
    ./common.nix
    (import ../lib/mkNixosService.nix {
      name = "sonify-health";
      inherit self;
    })
  ];

  options.services.sonify-health.openFirewall = lib.mkOption {
    type = lib.types.bool;
    default = false;
    description = ''
      Whether to open the configured port in the NixOS firewall.
      Only effective when socket is null (TCP mode).
    '';
  };

  config = lib.mkIf cfg.enable {
    # Open the TCP port when using host:port mode with openFirewall.
    networking.firewall.allowedTCPPorts =
      lib.mkIf (cfg.openFirewall && cfg.socket == null) [cfg.port];

    systemd.services.sonify-health = {
      # Probe commands are executed via "sh -c".  The default systemd
      # PATH on NixOS does not include /bin, so we must ensure a shell
      # is reachable.
      path = ["/bin" pkgs.bash];

      environment.sonify_health_config = toString cfg._generatedConfigFile;

      serviceConfig = {
        SupplementaryGroups = lib.mkDefault (
          ["audio"]
          ++ lib.optional pipewireEnabled "pipewire"
        );

        # Allow the cpal audio thread to use real-time scheduling
        # (SCHED_FIFO).  Without this the request fails silently and
        # the callback thread runs at normal priority.
        LimitRTPRIO = "99";
      };
    };
  };
}
