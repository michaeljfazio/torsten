{inputs, ...}: {
  perSystem = {
    inputs',
    system,
    config,
    lib,
    pkgs,
    ...
  }: let
    # Use stable toolchain for Dugite
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
        description = "Dugite - A Rust implementation of the Cardano node";
        license = lib.licenses.asl20;
      };
    };

    # Build dependencies separately for caching
    cargoArtifacts = craneLib.buildDepsOnly commonArgs;
  in {
    packages = {
      default = config.packages.dugite-node;

      # Dugite node
      dugite-node = craneLib.buildPackage (commonArgs
        // {
          inherit cargoArtifacts;
          pname = "dugite-node";
          inherit version;
          cargoExtraArgs = "-p dugite-node";
          doCheck = true;

          meta = commonArgs.meta // {
            mainProgram = "dugite-node";
          };
        });

      # Dugite CLI
      dugite-cli = craneLib.buildPackage (commonArgs
        // {
          inherit cargoArtifacts;
          pname = "dugite-cli";
          inherit version;
          cargoExtraArgs = "-p dugite-cli";
          doCheck = true;

          meta = commonArgs.meta // {
            mainProgram = "dugite-cli";
          };
        });

      # Dugite TUI
      dugite-tui = craneLib.buildPackage (commonArgs
        // {
          inherit cargoArtifacts;
          pname = "dugite-tui";
          inherit version;
          cargoExtraArgs = "-p dugite-tui";
          doCheck = true;

          meta = commonArgs.meta // {
            mainProgram = "dugite-tui";
          };
        });

      # All binaries in one package
      dugite-all = craneLib.buildPackage (commonArgs
        // {
          inherit cargoArtifacts;
          pname = "dugite";
          inherit version;
          doCheck = true;
        });
    };
  };
}
