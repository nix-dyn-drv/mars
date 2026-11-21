{
  lib,
  stdenv,
  runCommand,
  buildPackages,
  rustPlatform,
  cargo,
  writeExtern,
  cargo-dyndrv,
}:
{
  extern ? { },
  pname ? "",
  version ? "",
  name ? "${pname}-${version}",
  outputs,
  # Also offer every registry crate of the lock file: `<drv>.crate "<name>-<version>"`.
  exposeCrates ? false,
  ...
}@args:
let
  dyndrvAsCargo = runCommand "cargo-dyndrv-cargo" { } ''
    mkdir -p $out/bin
    ln -s ${lib.getExe buildPackages.cargo-dyndrv} $out/bin/cargo
  '';

  baseDerivation = rustPlatform.buildRustPackage (
    (lib.removeAttrs args [ "extern" "exposeCrates" ])
    // {
      name = "cargo-dyndrv-build";

      env = (args.env or { }) // {
        CARGO = lib.getExe buildPackages.cargo;
        EXTERN_PATH = writeExtern extern;
        HOST_CC = lib.getExe buildPackages.stdenv.cc;
        # NIX_BUILD_TOP seems like it would be convenient, but it is also set in dev shells
        CARGO_DYNDRV_IN_DRV = "";
      };
      nativeBuildInputs = (args.nativeBuildInputs or [ ]) ++ [
        dyndrvAsCargo
      ];

      # Each output is a derivation
      outputs = lib.map (name: "${name}.drv") outputs ++ lib.optional exposeCrates "crates.drv";

      doCheck = false;
      dontInstall = false;
      dontCargoInstall = true;
      dontFixup = true;

      requiredSystemFeatures = [ "builder-rpc-v0" ];

      # nixpgks stdenv assumes $out exists, but it does not with builder-rpc-v0
      out = "/nonexistant";

      __contentAddressed = true;
      outputHashMode = "text";
      outputHashAlgo = "sha256";

    }
  );

in
runCommand name
  {
    preferLocalBuild = true;
    passthru = {
      base = baseDerivation;
    }
    // lib.optionalAttrs exposeCrates {
      # One crate of the lock file as a derivation of its own: cargo-dyndrv fills `crates.drv`
      # with a derivation whose `<crate>-<version>.drv` outputs are the units' own derivations,
      # so this builds the crate and its dependencies and nothing else of the lock file.
      crate =
        crateName:
        runCommand crateName
          {
            preferLocalBuild = true;
            __contentAddressed = true;
            outputHashMode = "recursive";
            outputHashAlgo = "sha256";
          }
          ''
            ln -s ${
              builtins.outputOf (builtins.outputOf baseDerivation."crates.drv".outPath "${crateName}.drv") "out"
            } $out
          '';
    };
    inherit outputs;

    out = "/nonexistant";

    __contentAddressed = true;
    outputHashMode = "recursive";
    outputHashAlgo = "sha256";
  }
  (
    lib.concatMapStringsSep "\n" (output: ''
      ln -sL ${
        builtins.outputOf baseDerivation.${"${output}.drv"}.outPath "out"
      } ${builtins.placeholder output}
    '') outputs
  )
