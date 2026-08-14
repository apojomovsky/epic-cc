{
  description = "pic8_compiler — a C compiler for mid-range 8-bit Microchip PIC (PIC14) microcontrollers";

  inputs = {
    nixpkgs.url = "github:NixOS/nixpkgs/nixos-unstable";
    flake-utils.url = "github:numtide/flake-utils";
    rust-overlay = {
      url = "github:oxalica/rust-overlay";
      inputs.nixpkgs.follows = "nixpkgs";
    };
  };

  outputs = { self, nixpkgs, flake-utils, rust-overlay }:
    flake-utils.lib.eachDefaultSystem (system:
      let
        pkgs = import nixpkgs {
          inherit system;
          overlays = [ (import rust-overlay) ];
        };

        # The IR producer. Pinned deliberately and NOT tracking nixpkgs' default:
        # we parse LLVM IR *text*, so the clang version is part of our input format.
        # Bumping this is a migration, not a housekeeping change.
        # See docs/09-build-environment.md.
        llvm = pkgs.llvmPackages_20;

        rustToolchain = pkgs.rust-bin.fromRustupToolchainFile ./rust-toolchain.toml;
      in
      {
        devShells.default = pkgs.mkShell {
          packages = [
            rustToolchain

            # IR front end
            llvm.clang

            # PIC tooling. gpasm is an assembler cross-check oracle, not a
            # build dependency — see docs/05-verification.md.
            pkgs.gputils

            # Compiler testing: random program generation and auto-reduction
            pkgs.cvise
            pkgs.creduce
            pkgs.csmith

            # Reading the vendored reference PDFs (docs/06-environment.md)
            pkgs.poppler-utils

            pkgs.python3
          ];

          env = {
            # Wrapped clang: works out of the box, injects Nix host include paths.
            PIC8_CLANG = "${llvm.clang}/bin/clang";

            # Unwrapped clang: no wrapper-injected host flags. Likely the right
            # choice for `-target msp430 -S -emit-llvm`, but which one we actually
            # drive is a question for the feasibility spike.
            PIC8_CLANG_UNWRAPPED = "${llvm.clang-unwrapped}/bin/clang";

            # Builtin headers (stddef.h, stdint.h) live here. Needed if we drive
            # the unwrapped clang.
            #
            # Two nixpkgs gotchas baked into this path:
            #   1. Clang keys the directory on the MAJOR version only ("20"), so
            #      `lib.versions.major` — not the full "20.1.8".
            #   2. nixpkgs splits the headers into the `.lib` output, so clang's
            #      own `-print-resource-dir` reports a path that DOES NOT EXIST.
            #      Trust this variable, not `-print-resource-dir`.
            PIC8_CLANG_RESOURCE_DIR =
              "${llvm.clang-unwrapped.lib}/lib/clang/${pkgs.lib.versions.major llvm.llvm.version}";

            PIC8_GPASM = "${pkgs.gputils}/bin/gpasm";
            PIC8_VENDOR_DIR = toString ./vendor;
          };

          shellHook = ''
            # XC8 is an optional TEST ORACLE, never a build dependency. The shell
            # and the test suite must work without it. Override with PIC8_XC8_ROOT.
            : "''${PIC8_XC8_ROOT:=/opt/microchip/xc8/v4.00}"
            export PIC8_XC8_ROOT

            echo "pic8_compiler dev shell"
            echo "  rustc   $(rustc --version 2>/dev/null | cut -d' ' -f2)"
            echo "  clang   $(${llvm.clang}/bin/clang --version 2>/dev/null | head -1 | grep -oE '[0-9]+\.[0-9]+\.[0-9]+' | head -1)  (pinned)"
            echo "  gpasm   $(${pkgs.gputils}/bin/gpasm --version 2>&1 | grep -oE '[0-9]+\.[0-9]+\.[0-9]+' | head -1)"
            if [ -x "$PIC8_XC8_ROOT/bin/xc8-cc" ]; then
              echo "  xc8     found at $PIC8_XC8_ROOT  (differential oracle enabled)"
            else
              echo "  xc8     ABSENT  (differential tests will skip; set PIC8_XC8_ROOT)"
            fi
            echo "  gpsim   not packaged in nixpkgs — deferred, see docs/09-build-environment.md"
          '';
        };

        formatter = pkgs.nixpkgs-fmt;
      });
}
