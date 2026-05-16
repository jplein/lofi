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

        crateInfo = craneLib.crateNameFromCargoToml {
          cargoToml = ./app/gnome/Cargo.toml;
        };

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
          inherit (crateInfo) pname version;
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

        extension = pkgs.buildNpmPackage {
          pname = "lofi-shell-extension";
          version = crateInfo.version;
          src = ./extension/gnome;

          # Hash of the offline npm dependencies tree. Regenerate by
          # setting this to pkgs.lib.fakeHash and running `nix build .#extension`
          # — Nix will print the correct value to paste back here.
          npmDepsHash = "sha256-WIsAmlMt3KxQttps/B03Os1oSJ+V8DhusnOYBEcfn3I=";

          nativeBuildInputs = [ pkgs.zip ];

          # The build script uses `#!/usr/bin/env bash`, which doesn't exist
          # in the Nix sandbox. patchShebangs rewrites it to the absolute
          # store path of bash before we run `npm run build`.
          preBuild = ''
            patchShebangs build.sh
          '';

          # buildNpmPackage's default installPhase wants to install
          # node_modules into the output. We only want the .zip the build
          # script produces.
          installPhase = ''
            runHook preInstall
            mkdir -p $out
            cp lofi-shell@jplein.dev.shell-extension.zip $out/
            runHook postInstall
          '';
        };
      in {
        packages = {
          default = lofi;
          lofi = lofi;
          extension = extension;
        };

        devShells.default = pkgs.mkShell {
          nativeBuildInputs = nativeBuildInputs ++ [ rustToolchain pkgs.nodejs ];
          inherit buildInputs;
        };
      });
}
