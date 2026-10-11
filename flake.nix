{
  description = "lspkl packages and development environment";

  inputs = {
    nixpkgs.url = "https://flakehub.com/f/NixOS/nixpkgs/0.1"; # unstable Nixpkgs
    fenix = {
      url = "https://flakehub.com/f/nix-community/fenix/0.1";
      inputs.nixpkgs.follows = "nixpkgs";
    };
  };

  outputs =
    { self, ... }@inputs:

    let
      supportedSystems = [
        "x86_64-linux"
        "aarch64-darwin"
        "x86_64-darwin"
        "aarch64-linux"
      ];
      forEachSupportedSystem =
        f:
        inputs.nixpkgs.lib.genAttrs supportedSystems (
          system:
          f {
            inherit system;
            pkgs = import inputs.nixpkgs {
              inherit system;
              overlays = [
                inputs.self.overlays.default
              ];
            };
          }
        );
    in
    {
      overlays.default = final: prev: {
        rustToolchain =
          with inputs.fenix.packages.${prev.stdenv.hostPlatform.system};
          combine (
            with stable;
            [
              clippy
              rustc
              cargo
              rustfmt
              rust-src
            ]
          );
      };

      packages = forEachSupportedSystem (
        { pkgs, ... }:
        let
          rustPlatform = pkgs.makeRustPlatform {
            cargo = pkgs.rustToolchain;
            rustc = pkgs.rustToolchain;
          };
          lspkl = pkgs.callPackage ./nix/package.nix { inherit rustPlatform; };
        in
        {
          inherit lspkl;
          default = lspkl;
        }
      );

      checks = forEachSupportedSystem (
        { system, ... }:
        {
          lspkl = self.packages.${system}.lspkl;
        }
      );

      apps = forEachSupportedSystem (
        { system, ... }:
        let
          lspkl = {
            type = "app";
            program = "${self.packages.${system}.lspkl}/bin/lspkl";
            meta = self.packages.${system}.lspkl.meta;
          };
        in
        {
          inherit lspkl;
          default = lspkl;
        }
      );

      devShells = forEachSupportedSystem (
        { pkgs, system }:
        {
          default = pkgs.mkShell {
            packages = with pkgs; [
              pkl
              rustToolchain
              openssl
              pkg-config
              cargo-deny
              cargo-edit
              cargo-watch
              cargo-insta
              cargo-nextest
              rust-analyzer
              just
              jq
              zip
              unzip
              file
              binutils
              curl
              self.formatter.${system}
            ];

            env = {
              # Required by rust-analyzer
              RUST_SRC_PATH = "${pkgs.rustToolchain}/lib/rustlib/src/rust/library";
            };
          };
        }
      );

      formatter = forEachSupportedSystem ({ pkgs, ... }: pkgs.nixfmt);
    };
}
