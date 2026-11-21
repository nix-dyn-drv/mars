# MARS

Massive Assortment of Rust Stuff

<!-- INSERT ART HERE -->

## How to use
- Not usable yet. Still in the early infancy.

## How to contribute
- Reach out to help with design and to set up the project.
- Provide infrastructure + cache + funding.

- Build a shard `.#SOME_NUMBER` and deal with the errors

### Unexpected crate type
```
unit registry+https://github.com/rust-lang/crates.io-index#solana-program@2.3.0 has unexpected crate types, ["cdylib", "rlib"]
```

### pkg-config
```
Could not run `PKG_CONFIG_ALLOW_SYSTEM_CFLAGS=1 pkg-config --libs --cflags
```

## Idea / Roadmap
- Collect all crate metadata from crates.io.
- Bucket them into non-conflicting shards; no more than 50k members per shard.
- Use a dummy package that has no source, only dependencies.
- Generate `Cargo.toml` for this dummy package and an associated `Cargo.lock`.
- Use `github:obsidiansystems:cargo-dyndrv#buildDynamicCrate` to fetch each shard's depenencies and compile them.
- Rust/cargo will create rlib/staticlib/dylib and other artifacts that can be used for end-user application compilation.
- Stand up a CI/infrastructure to build and cache these artifacts for each shard.
- End users are able to declare their builds with `buildDynamicCrate` and will often be able to leverage the intermediate products via the cache.
- Specific crates require system-deps, overrides, and fixes, which will be managed in this repository and maintained over time.
- The build recipes + cache is updated over time to reflect current crates.io on a regular basis.

## Goals
- Improved evaluation and build times for large build graphs.
- Distribute build recipes, overrides, fixes, and dependency declarations as needed for use in the Nix ecosystem.
- Shared binary cache of a plurality of the crates from crates.io pre-compiled in order to speed up subsequent builds for project depending on them.
- Exercise the relevant infrastructure for dynamic derivation support.
- Validate [grey-havens](https://discourse.nixos.org/t/grey-havens-can-do-10x-the-work-of-old-hydra-and-1-3x-the-work-of-new-hydra/81221) project with a large-scale and public workload.

- Long-term: standardize and upstream the discovered system-deps into crates
- North Star: serve as an example for languge-specific package sets and to showcase the benefits of such specialization

## Notes
- A modern variant of Nix including `nix-command flakes ca-derivation dyanmic-derivations` is asummed .
- Binary cache is currently best-effort: we are interested to provide a proper guarantee depending on adoption and support
- We provide coarse-grained artifacts at the crate-level, not at the individul compilation unit.

### cargo-dyndrv fork
The flake pins [lisanna-dettwyler/cargo-dyndrv](https://github.com/lisanna-dettwyler/cargo-dyndrv) branch `mars`: upstream PR 4 plus the fixes below and the `crates.drv` output behind `exposeCrates` (one derivation per shard whose `<crate>-<version>.drv` outputs expose every library unit). Its overlay provides `buildDynamicCrate`.
- `--cap-lints allow` for every non-path crate, as cargo does, so `#![deny(warnings)]` in a registry crate no longer fails on a newer rustc (the most common failure in the first sweep: `hiding a lifetime that's elided elsewhere is confusing`, `implicit autoref ...`, `use of deprecated ...`)
- targets with several crate types (`["cdylib", "rlib"]`) build as `lib` instead of bailing with "unexpected crate types"
- `CARGO_MANIFEST_LINKS` is set for build scripts of `links` packages (ring's build.rs unwraps it)
- `extern` entries may be keyed by bare crate name (`"openssl-sys"`) as well as the exact package id
- `PROFILE`, `DEBUG` and `NUM_JOBS` are set for build scripts (the `built` and `cmake` crates and many hand-written build.rs files unwrap `PROFILE`)
- `RUSTDOC` and `CARGO_ENCODED_RUSTFLAGS` are set for build scripts (`built` reads `RUSTDOC`, av-scenechange's build.rs unwraps the rustflags)
- an `extern` entry keyed `"*"` applies to every unit that has no more specific entry

`externFor` in `flake.nix` is the default extern map (pkg-config for the common `-sys` crates, plus a `"*"` entry that gives every build script pkg-config, cmake, protoc, python3, make, autotools, nasm and clang on `PATH` and sets `PKG_CONFIG_PATH`, `PROTOC` and `LIBCLANG_PATH`); per-shard `overrides` are merged on top.
