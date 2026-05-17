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
    let
      linuxOutputs = (flake-utils.lib.eachSystem [ "x86_64-linux" "aarch64-linux" ] (system:
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

            # GNOME Shell only scans the user extensions dir on session start.
            # `gnome-extensions enable` asks the live shell to enable an
            # extension it has already discovered — on first install it
            # hasn't, so the call fails with "does not exist". Writing the
            # dconf entry directly causes auto-enable on the next session
            # start, which is what we want either way.
            echo "Ensuring $UUID is in enabled-extensions..."
            current=$(gsettings get org.gnome.shell enabled-extensions)
            if echo "$current" | grep -qF "'$UUID'"; then
              echo "  already present."
            elif [ "$current" = "@as []" ]; then
              gsettings set org.gnome.shell enabled-extensions "['$UUID']"
              echo "  added (list was empty)."
            else
              new="''${current%]}, '$UUID']"
              gsettings set org.gnome.shell enabled-extensions "$new"
              echo "  appended."
            fi

            # If the shell happens to already know about the extension
            # (re-install during the same session), poke it to enable now.
            gnome-extensions enable "$UUID" >/dev/null 2>&1 || true

            if [ "''${XDG_SESSION_TYPE:-}" = "wayland" ]; then
              echo
              echo "On Wayland, GNOME Shell only loads newly-installed extensions on session start."
              echo "Log out and log back in — $UUID will auto-enable from dconf."
            else
              echo
              echo "On X11, you can reload GNOME Shell with Alt+F2 then 'r' (then it'll auto-enable from dconf)."
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

            # Disable in running shell (no-op if it isn't loaded).
            gnome-extensions disable "$UUID" >/dev/null 2>&1 || true

            # Remove from dconf so it doesn't auto-enable on next session.
            current=$(gsettings get org.gnome.shell enabled-extensions)
            if echo "$current" | grep -qF "'$UUID'"; then
              echo "Removing $UUID from enabled-extensions..."
              # Three cases handled by the chained sed:
              #   ['X', 'Y']   ->  ['Y']      (drop "'X', ")
              #   ['Y', 'X']   ->  ['Y']      (drop ", 'X'")
              #   ['X']        ->  @as []     (sole entry)
              new=$(printf '%s' "$current" \
                | sed "s/'$UUID', //;s/, '$UUID'//;s/\\['$UUID'\\]/@as []/")
              gsettings set org.gnome.shell enabled-extensions "$new"
            fi

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
        # home-manager modules ship only with the Linux side — there is
        # no macOS equivalent planned.
        homeManagerModules.lofi = import ./nix/hm-module.nix { inherit self; };
        homeManagerModules.default = self.homeManagerModules.lofi;
      };

      # macOS (Apple Silicon) outputs. Separate from the Linux block
      # because the Darwin build will not share the GTK / GNOME inputs.
      # Just a Rust devShell for now — packages/apps come as the macOS
      # port comes together.
      darwinOutputs = flake-utils.lib.eachSystem [ "aarch64-darwin" ] (system:
        let
          pkgs = import nixpkgs {
            inherit system;
            overlays = [ (import rust-overlay) ];
          };
          rustToolchain = pkgs.rust-bin.fromRustupToolchainFile ./rust-toolchain.toml;
        in {
          devShells.default = pkgs.mkShell {
            # `bazelisk` is the Bazel version dispatcher; it reads
            # `.bazelversion` at the repo root and downloads the matching
            # Bazel release on first invocation. Bazel is the macOS
            # build's single front door — `cargo` is included for
            # editor tooling (rust-analyzer, ad-hoc one-offs), not for
            # producing artefacts.
            #
            # Swift itself is not in the shell — Nix on Darwin does not
            # ship a usable Swift toolchain, so that comes from the
            # user's Xcode / Command Line Tools.
            nativeBuildInputs = [ rustToolchain pkgs.bazelisk ];
          };
        });
    in
      # `//` is shallow — both blocks define `devShells` (etc), so a
      # plain merge would have Darwin clobber Linux at the top level.
      nixpkgs.lib.recursiveUpdate linuxOutputs darwinOutputs;
}
