{
  description = "Rust devshell";

  inputs = {
    nixpkgs.url = "github:NixOS/nixpkgs/nixos-unstable";
    rust-overlay.url = "github:oxalica/rust-overlay";
    flake-utils.url = "github:numtide/flake-utils";
  };

  outputs =
    {
      nixpkgs,
      rust-overlay,
      flake-utils,
      ...
    }:
    flake-utils.lib.eachDefaultSystem (
      system:
      let
        overlays = [ (import rust-overlay) ];
        pkgs = import nixpkgs {
          inherit system overlays;
          config.allowUnfree = true;
        };
        toolchain = pkgs.rust-bin.fromRustupToolchainFile ./rust-toolchain.toml;
        beads-latest = (pkgs.beads.override {
          buildGoModule = pkgs.buildGoModule.override { go = pkgs.go_1_26; };
        }).overrideAttrs (old: rec {
          version = "0.60.0";
          src = pkgs.fetchFromGitHub {
            owner = "steveyegge";
            repo = "beads";
            rev = "v${version}";
            hash = "sha256-z3EDtaBHB3ltPRT7vuBFURD7UwgAJBXAPozRnkjejeU=";
          };
          vendorHash = "sha256-1BJsEPP5SYZFGCWHLn532IUKlzcGDg5nhrqGWylEHgY=";
          doCheck = false;
        });
        playwrightDeps = with pkgs; [
          glib
          nss
          nspr
          atk
          at-spi2-atk
          cups.lib
          dbus
          libdrm
          expat
          libxkbcommon
          xorg.libX11
          xorg.libXcomposite
          xorg.libXdamage
          xorg.libXext
          xorg.libXfixes
          xorg.libXrandr
          xorg.libxcb
          mesa
          pango
          cairo
          alsa-lib
          gtk3
          systemd
          libgbm
        ];
      in
      {
        devShells.default =
          with pkgs;
          mkShell {
            packages = [
              nil
              just
              cargo-expand
              bacon
              claude-code
              dolt
              beads-latest
              tailwindcss
              esbuild
              nodejs
              cargo-dist
            ];

            buildInputs = [
              openssl
              pkg-config
              toolchain
            ];

            shellHook = ''
              export LD_LIBRARY_PATH="${lib.makeLibraryPath playwrightDeps}''${LD_LIBRARY_PATH:+:$LD_LIBRARY_PATH}"
              export PLAYWRIGHT_SKIP_VALIDATE_HOST_REQUIREMENTS=true
            '';
          };
      }
    );
}
