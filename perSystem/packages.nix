{inputs, ...}: {
  perSystem = {
    inputs',
    system,
    config,
    lib,
    pkgs,
    ...
  }: let
    # Use stable toolchain for Torsten
    toolchain = with inputs'.fenix.packages;
      combine [
        stable.rustc
        stable.cargo
        stable.clippy
        stable.rustfmt
      ];

    craneLib = (inputs.crane.mkLib pkgs).overrideToolchain toolchain;

    src = lib.fileset.toSource {
      root = ./..;
      fileset = lib.fileset.unions [
        ../Cargo.lock
        ../Cargo.toml
        ../crates
        ../tests
      ];
    };

    # Extract pname and version from workspace Cargo.toml
    cargoToml = builtins.fromTOML (builtins.readFile ../Cargo.toml);
    version = cargoToml.workspace.package.version;

    commonArgs = {
      inherit src;
      strictDeps = true;

      nativeBuildInputs = with pkgs; [
        pkg-config
        installShellFiles
      ];

      buildInputs = with pkgs; lib.optionals stdenv.hostPlatform.isDarwin [
        darwin.apple_sdk.frameworks.Security
        darwin.apple_sdk.frameworks.SystemConfiguration
      ];

      meta = {
        description = "Torsten - A Rust implementation of the Cardano node";
        license = lib.licenses.asl20;
      };
    };

    # Build dependencies separately for caching
    cargoArtifacts = craneLib.buildDepsOnly commonArgs;
  in {
    packages = {
      default = config.packages.torsten-node;

      # Torsten node
      torsten-node = craneLib.buildPackage (commonArgs
        // {
          inherit cargoArtifacts;
          pname = "torsten-node";
          inherit version;
          cargoExtraArgs = "-p torsten-node";
          doCheck = true;

          meta = commonArgs.meta // {
            mainProgram = "torsten-node";
          };
        });

      # Torsten CLI
      torsten-cli = craneLib.buildPackage (commonArgs
        // {
          inherit cargoArtifacts;
          pname = "torsten-cli";
          inherit version;
          cargoExtraArgs = "-p torsten-cli";
          doCheck = true;

          meta = commonArgs.meta // {
            mainProgram = "torsten-cli";
          };
        });

      # Torsten TUI
      torsten-tui = craneLib.buildPackage (commonArgs
        // {
          inherit cargoArtifacts;
          pname = "torsten-tui";
          inherit version;
          cargoExtraArgs = "-p torsten-tui";
          doCheck = true;

          meta = commonArgs.meta // {
            mainProgram = "torsten-tui";
          };
        });

      # All binaries in one package
      torsten-all = craneLib.buildPackage (commonArgs
        // {
          inherit cargoArtifacts;
          pname = "torsten";
          inherit version;
          doCheck = true;
        });
    };
  };
}
