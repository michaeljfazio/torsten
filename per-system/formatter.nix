{
  perSystem = {pkgs, ...}: {
    treefmt = {
      projectRootFile = "flake.nix";
      programs.rustfmt.enable = true;
      programs.alejandra.enable = true;
    };
  };
}
