{
  description = "MARS - Massive Assortment of Rust Stuff";

  inputs = {
    nixpkgs.url = "github:NixOS/nixpkgs/nixpkgs-unstable";
    rust-overlay = {
      url = "github:oxalica/rust-overlay";
      inputs.nixpkgs.follows = "nixpkgs";
    };
    cargo-dyndrv = {
      url = "github:obsidiansystems/cargo-dyndrv/refs/pull/4/head";
      inputs.nixpkgs.follows = "nixpkgs";
    };
  };

  outputs =
    {
      self,
      nixpkgs,
      rust-overlay,
      cargo-dyndrv,
      ...
    }:
    let
      # Fixes for cargo-dyndrv (PR 4 head) found while building the shards, carried as a patch until they
      # land upstream: `--cap-lints allow` for registry crates, one crate type per unit,
      # CARGO_MANIFEST_LINKS for build scripts, extern entries matched by bare crate name.
      marsOverlay = final: prev: {
        cargo-dyndrv = prev.cargo-dyndrv.overrideAttrs (o: {
          patches = (o.patches or [ ]) ++ [ ./patches/cargo-dyndrv-mars.patch ];
        });
      };

      # Default extern map: system libraries for the -sys crates seen so far, found through pkg-config
      # inside the sandbox. Keys are bare crate names (extern-by-name fix above), so one entry covers
      # every version in a lock file; per-shard `overrides` (exact package ids) are merged on top.
      externFor =
        pkgs:
        let
          lib = pkgs.lib;
          # nixpkgs puts .pc files in lib/pkgconfig or share/pkgconfig (zlib uses the latter)
          pcDirs = lib.concatMapStringsSep ":" (p: "${lib.getDev p}/lib/pkgconfig:${lib.getDev p}/share/pkgconfig") (
            with pkgs;
            [
              openssl
              dbus
              sqlite
              zlib
              libgit2
              libssh2
              alsa-lib
              udev
              glib
              freetype
              expat
              fontconfig
              curl
              libusb1
              libxml2
              bzip2
              xz
              zstd
            ]
          );
          pc = {
            path = [ "${lib.getBin pkgs.buildPackages.pkg-config}/bin" ];
            env = {
              PKG_CONFIG_PATH = pcDirs;
              PKG_CONFIG = lib.getExe pkgs.buildPackages.pkg-config;
            };
          };
        in
        lib.genAttrs [
          "openssl-sys"
          "libdbus-sys"
          "libsqlite3-sys"
          "libz-sys"
          "libgit2-sys"
          "libssh2-sys"
          "alsa-sys"
          "libudev-sys"
          "glib-sys"
          "gobject-sys"
          "gio-sys"
          "freetype-sys"
          "expat-sys"
          "servo-fontconfig-sys"
          "fontconfig-sys"
          "yeslogic-fontconfig-sys"
          "curl-sys"
          "libusb1-sys"
          "libxml"
          "bzip2-sys"
          "lzma-sys"
          "zstd-sys"
        ] (_: pc)
        // {
          "openssl-sys" = pc // {
            env = pc.env // {
              OPENSSL_NO_VENDOR = "1";
            };
          };
        };

      eachDefaultSystem =
        overlays: function:
        nixpkgs.lib.genAttrs
          [
            "x86_64-linux"
            "aarch64-linux"
            "aarch64-darwin"
          ]
          (
            system:
            let
              pkgs = import nixpkgs { inherit system overlays; };
            in
            function pkgs system
          );
    in
    {
      overrides = {
        "700" = pkgs: {
          extern = {
            "registry+https://github.com/rust-lang/crates.io-index#libssh2-sys@0.3.4" = {
              path = [ "${pkgs.lib.getBin pkgs.buildPackages.pkg-config}/bin" ];
              env = {
                PKG_CONFIG_PATH = "${pkgs.lib.getDev pkgs.libssh2}/lib/pkgconfig";
                PKG_CONFIG = pkgs.lib.getExe pkgs.buildPackages.pkg-config;
		PROFILE = "default";
              };
            };
            "registry+https://github.com/rust-lang/crates.io-index#ring@0.17.14" = {
              env = {
                CARGO_MANIFEST_LINKS = "ring_core_0_17_14_";
              };
            };
            "registry+https://github.com/rust-lang/crates.io-index#openssl-sys@0.9.118" = {
              path = [ "${pkgs.lib.getBin pkgs.buildPackages.pkg-config}/bin" ];
              env = {
                PKG_CONFIG_PATH = "${pkgs.lib.getDev pkgs.openssl}/lib/pkgconfig";
                PKG_CONFIG = pkgs.lib.getExe pkgs.buildPackages.pkg-config;
              };
            };
          };
        };
      };

      overlays.default = nixpkgs.lib.composeManyExtensions [
        cargo-dyndrv.overlays.default
        marsOverlay
      ];

      packages = eachDefaultSystem [ (import rust-overlay) (cargo-dyndrv.overlays.default) marsOverlay ] (
        pkgs: system:
        let
          dir = builtins.readDir ./shards;
          d = builtins.mapAttrs (
            n: v:
            let
              override = (self.overrides."${n}" or (x: { })) pkgs;
            in
            pkgs.buildDynamicCrate (
              {
                pname = "shard-${n}";
                version = "unstable";
                src = builtins.path {
                  name = "src";
                  path = ./shards + "/${n}";
                };
                preBuild = ''
                  cp ${./src}/main.rs ./src
                  sed -i 's#/dev/null#./src#' Cargo.toml
                  cat Cargo.toml
                '';
                # env.RUSTFLAGS = "-D warnings -A mismatched_lifetime_syntaxes";
                cargoLock.lockFile = ./shards + "/${n}/Cargo.lock";
                outputs = [ "dummy" ];
              }
              // override
              // {
                extern = (externFor pkgs) // (override.extern or { });
              }
            )
          ) dir;
        in
        d
      );

      devShells = eachDefaultSystem [ (import rust-overlay) ] (
        pkgs: system: {
          default = pkgs.mkShell.override { stdenv = pkgs.clangStdenv; } {
            packages = with pkgs; [
              openssl
              pkg-config
              parallel
              findutils
              flock
              jq
              rust-bin.stable.latest.default
              (rustPlatform.buildRustPackage {
                pname = "index-select";
                version = "0.1.0";
                src = ./tools/index-select;
                cargoLock.lockFile = ./tools/index-select/Cargo.lock;
                # The cargo crate links these; nixpkgs disallows vendoring them.
                nativeBuildInputs = [ pkg-config ];
                buildInputs = [
                  openssl
                  libgit2
                  libssh2
                  sqlite
                  zlib
                ];
              })
              (writeShellScriptBin "gen-crates" ''
                set -euo pipefail

                # One package per shard, each with its own Cargo.lock; see tools/index-select.
                # shards/resolved.tsv caches the per-crate resolutions; delete it after updating the index.
                index-select crates.io-index shards
              '')
            ];
          };
        }
      );
    };
}
