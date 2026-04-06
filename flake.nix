{
  description = "Dugite - A Rust implementation of the Cardano node";

  inputs = {
    nixpkgs.url = "github:NixOS/nixpkgs/nixos-25.11";
    flake-parts.url = "github:hercules-ci/flake-parts";
    crane.url = "github:ipetkov/crane";
    fenix.url = "github:nix-community/fenix";
    fenix.inputs.nixpkgs.follows = "nixpkgs";
    rust-overlay.url = "github:oxalica/rust-overlay";
    rust-overlay.inputs.nixpkgs.follows = "nixpkgs";
    treefmt-nix.url = "github:numtide/treefmt-nix";
    treefmt-nix.inputs.nixpkgs.follows = "nixpkgs";
  };

  outputs = {
    self,
    flake-parts,
    nixpkgs,
    ...
  } @ inputs: let
    inherit ((import ./nix/lib.nix {inherit inputs;}).flake.lib) recursiveImports;
  in
    flake-parts.lib.mkFlake {inherit inputs;} {
      imports =
        recursiveImports [
          ./nix
        ]
        ++ [
          inputs.treefmt-nix.flakeModule
        ];
      systems = [
        "x86_64-linux"
        "aarch64-linux"
        "x86_64-darwin"
        "aarch64-darwin"
      ];
      perSystem = {system, ...}: {
        _module.args.pkgs = import inputs.nixpkgs {
          inherit system;
          overlays = [inputs.rust-overlay.overlays.default];
        };
      };
    }
    // {
      inherit inputs;
    };
}
