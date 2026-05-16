{
  description = "LoFi launcher";

  inputs = {
    nixpkgs.url = "github:NixOS/nixpkgs/nixos-unstable";
    flake-utils.url = "github:numtide/flake-utils";
    rust-overlay = {
      url = "github:oxalica/rust-overlay";
      inputs.nixpkgs.follows = "nixpkgs";
    };
    crane.url = "github:ipetkov/crane";
  };

  outputs = { self, nixpkgs, flake-utils, rust-overlay, crane }:
    flake-utils.lib.eachSystem [ "x86_64-linux" "aarch64-linux" ] (system:
      let
        pkgs = import nixpkgs {
          inherit system;
          overlays = [ (import rust-overlay) ];
        };

        rustToolchain = pkgs.rust-bin.fromRustupToolchainFile ./rust-toolchain.toml;

        craneLib = (crane.mkLib pkgs).overrideToolchain rustToolchain;

        nativeBuildInputs = with pkgs; [
          pkg-config
          wrapGAppsHook4
        ];

        buildInputs = with pkgs; [
          gtk4
          libadwaita
          glib
        ];

        commonArgs = {
          src = craneLib.cleanCargoSource ./app;
          strictDeps = true;
          inherit nativeBuildInputs buildInputs;
        };

        cargoArtifacts = craneLib.buildDepsOnly commonArgs;

        lofi = craneLib.buildPackage (commonArgs // {
          inherit cargoArtifacts;
          pname = "lofi";
          cargoExtraArgs = "--package lofi-gnome";

          meta = with pkgs.lib; {
            description = "A small launcher for GNOME and macOS";
            mainProgram = "lofi";
            platforms = platforms.linux;
          };
        });
      in {
        packages = {
          default = lofi;
          lofi = lofi;
        };

        devShells.default = pkgs.mkShell {
          nativeBuildInputs = nativeBuildInputs ++ [ rustToolchain ];
          inherit buildInputs;
        };
      });
}
