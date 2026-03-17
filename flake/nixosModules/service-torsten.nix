{
  flake.nixosModules.service-torsten = {
    config,
    lib,
    pkgs,
    ...
  }: let
    cfg = config.services.torsten;
  in
    with lib; {
      options.services.torsten = {
        enable = mkEnableOption "torsten Cardano node";

        package = mkOption {
          type = types.package;
          default = pkgs.torsten-node;
          description = "The torsten package to use";
        };

        network = mkOption {
          type = types.enum ["mainnet" "preprod" "preview"];
          default = "mainnet";
          description = "Cardano network to connect to";
        };

        configFile = mkOption {
          type = types.path;
          description = "Path to node configuration JSON file";
        };

        topologyFile = mkOption {
          type = types.path;
          description = "Path to topology JSON file";
        };

        databasePath = mkOption {
          type = types.str;
          default = "/var/lib/torsten";
          description = "Path to blockchain database directory";
        };

        socketPath = mkOption {
          type = types.str;
          default = "/run/torsten/node.sock";
          description = "Path to node socket";
        };

        hostAddr = mkOption {
          type = types.str;
          default = "0.0.0.0";
          description = "Address to bind the node";
        };

        port = mkOption {
          type = types.port;
          default = 3001;
          description = "Port for P2P connections";
        };

        mithrilImport = mkOption {
          type = types.bool;
          default = false;
          description = "Import Mithril snapshot on first run";
        };

        extraArgs = mkOption {
          type = types.listOf types.str;
          default = [];
          description = "Extra command-line arguments to pass to torsten-node";
        };

        user = mkOption {
          type = types.str;
          default = "torsten";
          description = "User to run torsten service as";
        };

        group = mkOption {
          type = types.str;
          default = "torsten";
          description = "Group for torsten service";
        };
      };

      config = mkIf cfg.enable {
        users.users.${cfg.user} = {
          isSystemUser = true;
          group = cfg.group;
          home = cfg.databasePath;
          createHome = true;
        };

        users.groups.${cfg.group} = {};

        systemd.services.torsten = {
          description = "Torsten Cardano Node";
          wantedBy = ["multi-user.target"];
          after = ["network-online.target"];
          wants = ["network-online.target"];

          environment = {
            RUST_LOG = "torsten=info,torsten_network=debug";
          };

          preStart = mkIf cfg.mithrilImport ''
            if [ ! -d "${cfg.databasePath}/immutable" ]; then
              echo "Importing Mithril snapshot..."
              ${cfg.package}/bin/torsten-node mithril-import \
                --network-magic ${
                if cfg.network == "mainnet"
                then "764824073"
                else if cfg.network == "preprod"
                then "1"
                else "2"
              } \
                --database-path ${cfg.databasePath}
            fi
          '';

          serviceConfig = {
            Type = "simple";
            User = cfg.user;
            Group = cfg.group;
            Restart = "always";
            RestartSec = "30s";
            TimeoutStartSec = "600";

            StateDirectory = "torsten";
            RuntimeDirectory = "torsten";

            ExecStart = ''
              ${cfg.package}/bin/torsten-node run \
                --config ${cfg.configFile} \
                --topology ${cfg.topologyFile} \
                --database-path ${cfg.databasePath} \
                --socket-path ${cfg.socketPath} \
                --host-addr ${cfg.hostAddr} \
                --port ${toString cfg.port} \
                ${concatStringsSep " " cfg.extraArgs}
            '';

            # Resource limits
            MemoryMax = "8G";

            # Hardening
            NoNewPrivileges = true;
            PrivateTmp = true;
            ProtectSystem = "strict";
            ProtectHome = true;
            ReadWritePaths = [cfg.databasePath "/run/torsten"];
          };
        };

        # Firewall
        networking.firewall.allowedTCPPorts = [cfg.port];
      };
    };
}
