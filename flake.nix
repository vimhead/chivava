{
  description = "Chivava terminal typing practice";

  inputs = {
    nixpkgs.url = "github:NixOS/nixpkgs/nixos-26.05";
    home-manager = {
      url = "github:nix-community/home-manager/release-26.05";
      inputs.nixpkgs.follows = "nixpkgs";
    };
  };

  outputs =
    {
      self,
      nixpkgs,
      home-manager,
    }:
    let
      systems = [
        "aarch64-darwin"
        "x86_64-darwin"
        "aarch64-linux"
        "x86_64-linux"
      ];
      forAllSystems = nixpkgs.lib.genAttrs systems;
      pkgsFor = system: import nixpkgs { inherit system; };
    in
    {
      packages = forAllSystems (
        system:
        let
          package = (pkgsFor system).callPackage ./nix/package.nix { };
        in
        {
          chivava = package;
          default = package;
        }
      );

      apps = forAllSystems (system: {
        default = {
          type = "app";
          program = "${self.packages.${system}.default}/bin/chivava";
          meta.description = "Start Chivava typing practice";
        };
      });

      homeManagerModules.default = import ./nix/home-manager.nix;
      homeManagerModules.chivava = self.homeManagerModules.default;

      checks = forAllSystems (
        system:
        let
          pkgs = pkgsFor system;
        in
        {
          package = self.packages.${system}.default;
          home-manager = import ./nix/check-module.nix {
            inherit pkgs home-manager;
            package = self.packages.${system}.default;
            module = self.homeManagerModules.default;
          };
        }
      );

      formatter = forAllSystems (system: (pkgsFor system).nixfmt);
    };
}
