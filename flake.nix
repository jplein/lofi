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
    (flake-utils.lib.eachSystem [ "x86_64-linux" "aarch64-linux" ] (system:
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

          # The .zip is what `nix run .#install-extension` hands to
          # `gnome-extensions install --force`. The extracted tree under
          # `share/gnome-shell/extensions/...` is what the home-manager
          # module symlinks into the user's profile.
          installPhase = ''
            runHook preInstall

            mkdir -p $out
            cp lofi-shell@jplein.dev.shell-extension.zip $out/

            mkdir -p $out/share/gnome-shell/extensions/lofi-shell@jplein.dev
            cp -r dist/. $out/share/gnome-shell/extensions/lofi-shell@jplein.dev/

            runHook postInstall
          '';
        };

        install-extension = pkgs.writeShellApplication {
          name = "lofi-install-extension";
          text = ''
            if ! command -v gnome-extensions >/dev/null; then
              echo "error: 'gnome-extensions' CLI not found on PATH." >&2
              echo "Ensure GNOME Shell is installed (you're presumably on GNOME)." >&2
              exit 1
            fi

            UUID="lofi-shell@jplein.dev"
            ZIP="${extension}/lofi-shell@jplein.dev.shell-extension.zip"

            echo "Installing $UUID..."
            gnome-extensions install --force "$ZIP"

            echo "Enabling $UUID..."
            gnome-extensions enable "$UUID"

            if [ "''${XDG_SESSION_TYPE:-}" = "wayland" ]; then
              echo
              echo "On Wayland, GNOME Shell only loads newly-installed extensions on session start."
              echo "Log out and log back in to load $UUID."
            else
              echo
              echo "On X11, you can reload GNOME Shell with Alt+F2 then 'r'."
            fi
          '';
        };

        uninstall-extension = pkgs.writeShellApplication {
          name = "lofi-uninstall-extension";
          text = ''
            if ! command -v gnome-extensions >/dev/null; then
              echo "error: 'gnome-extensions' CLI not found on PATH." >&2
              exit 1
            fi

            UUID="lofi-shell@jplein.dev"

            gnome-extensions disable "$UUID" || true
            gnome-extensions uninstall "$UUID" || true
            echo "Removed $UUID."
          '';
        };
      in {
        packages = {
          default = lofi;
          lofi = lofi;
          extension = extension;
        };

        apps = {
          install-extension = {
            type = "app";
            program = "${install-extension}/bin/lofi-install-extension";
          };
          uninstall-extension = {
            type = "app";
            program = "${uninstall-extension}/bin/lofi-uninstall-extension";
          };
        };

        devShells.default = pkgs.mkShell {
          nativeBuildInputs = nativeBuildInputs ++ [ rustToolchain pkgs.nodejs ];
          inherit buildInputs;
        };
      })) // {
        homeManagerModules.lofi = import ./nix/hm-module.nix { inherit self; };
        homeManagerModules.default = self.homeManagerModules.lofi;
      };
}
