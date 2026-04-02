{
  perSystem = {
    config,
    pkgs,
    inputs',
    ...
  }: let
    # Use stable toolchain - same as packages.nix
    toolchain = with inputs'.fenix.packages;
      combine [
        stable.rustc
        stable.cargo
        stable.clippy
        stable.rustfmt
        stable.rust-analyzer
      ];
  in {
    devShells.default = with pkgs;
      mkShell {
        packages =
          [
            # Rust toolchain (stable from fenix)
            toolchain
            cmake
            pkg-config
            openssl
            zlib

            # Task runner
            just

            # Utilities
            jq
            fd
            ripgrep

            # Tree formatter
            config.treefmt.build.wrapper
          ]
          ++ lib.optionals stdenv.hostPlatform.isDarwin [
            darwin.apple_sdk.frameworks.Security
            darwin.apple_sdk.frameworks.SystemConfiguration
          ];

        shellHook = ''
          echo "🦀 Dugite - Cardano Node in Rust"
          echo ""
          echo "Rust: $(rustc --version)"
          echo "Cargo: $(cargo --version)"
          echo ""
          echo "Commands:"
          echo "  cargo build --all-targets          # Build everything"
          echo "  cargo test --all                   # Run all tests"
          echo "  cargo clippy --all-targets -- -D warnings  # Lint"
          echo "  cargo fmt --all -- --check         # Check formatting"
          echo ""
          echo "  cargo build --release              # Build release binary"
          echo "  cargo run -p dugite-node -- --help"
          echo "  cargo run -p dugite-cli -- --help"
          echo "  cargo run -p dugite-tui"
        '';
      };
  };
}
