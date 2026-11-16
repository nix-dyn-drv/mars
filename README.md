# MARS

Massive Assortment of Rust Stuff

<!-- INSERT ART HERE -->

## How to use
- Not usable yet. Still in the early infancy.

## How to contribute
- Reach out to help with design and to set up the project.
- Provide infrastructure + cache + funding.

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
