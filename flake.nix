{
  description = "MARS - Massive Assortment of Rust Stuff";

  inputs = {
    nixpkgs.url = "github:NixOS/nixpkgs";
    rust-overlay = {
      url = "github:oxalica/rust-overlay";
      inputs.nixpkgs.follows = "nixpkgs";
    };
    cargo-dyndrv = {
      url = "github:obsidiansystems/cargo-dyndrv";
      inputs.nixpkgs.follows = "nixpkgs";
    };
  };

  outputs = { self, nixpkgs, rust-overlay, cargo-dyndrv, ... }:
    let
      eachDefaultSystem = overlays: function:
        nixpkgs.lib.genAttrs [
          "x86_64-linux"
          "aarch64-linux"
          "aarch64-darwin"
        ] (system:
          let
            pkgs = import nixpkgs { inherit system overlays; };
          in function pkgs system);
    in {

      packages = eachDefaultSystem [ (import rust-overlay) (cargo-dyndrv.overlays.default) ] (pkgs: system: 
	  let
	    dir = builtins.readDir ./shards;
	    d = builtins.mapAttrs (n: v: 
	      pkgs.buildDynamicCrate {
	          pname = "shard-${n}";
	          version = "unstable";
		  src = ./shards + "/${n}";
		  cargoLock.lockFile = ./shards + "/${n}/Cargo.lock";
		  outputs = ["shard"];
	      }) dir;
	  in
	    d
	/*
      {
	  pname = "ffmpeg-sys-next";
	} ;
      }
      */
      );

      devShells = eachDefaultSystem [ (import rust-overlay) ] (pkgs: system: {
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
              buildInputs = [ openssl libgit2 libssh2 sqlite zlib ];
            })
            (
              writeShellScriptBin "gen-crates" ''
                set -euo pipefail

                # One package per shard, each with its own Cargo.lock; see tools/index-select.
                # shards/resolved.tsv caches the per-crate resolutions; delete it after updating the index.
                index-select crates.io-index shards
              ''
            )
          ];
        };
      });
    };
}
