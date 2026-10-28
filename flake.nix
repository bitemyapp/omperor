{
  description = "Omperor, a native Aizome app for coding agents: its dev shell";

  inputs = {
    nixpkgs.url = "https://flakehub.com/f/NixOS/nixpkgs/0.1";

    fenix = {
      url = "github:nix-community/fenix";
      inputs.nixpkgs.follows = "nixpkgs";
      inputs.rust-analyzer-src.follows = "";
    };
  };

  outputs =
    { nixpkgs, fenix, ... }:
    let
      # The OMP helper's embedded Python is fetched for x86-64 Linux only
      # (crates/py/scripts/fetch-python.sh in vendor/oh-my-pi).
      systems = [ "x86_64-linux" ];
      forAllSystems = f: nixpkgs.lib.genAttrs systems (system: f nixpkgs.legacyPackages.${system});
    in
    {
      packages = forAllSystems (pkgs: {
        aizome-fonts = pkgs.callPackage ./nix/aizome-fonts.nix { };
      });

      devShells = forAllSystems (pkgs: {
        default = pkgs.mkShell {
          packages = [
            # The toolchain, components included, comes from rust-toolchain.toml.
            (fenix.packages.${pkgs.stdenv.hostPlatform.system}.fromToolchainFile {
              file = ./rust-toolchain.toml;
              sha256 = "sha256-lYQxtgBt4zZfd4p7QDOaE26H7qwHwl9Pny1he+Nmn+s=";
            })
            pkgs.just
            # scripts/test-concurrent-rpc.py. An environment rather than
            # python3 itself, whose setup hook (for building nixpkgs
            # packages) sets PYTHONPATH and _PYTHON_SYSCONFIGDATA_NAME for
            # every interpreter, the OMP helper's embedded one included.
            (pkgs.python3.withPackages (_: [ ]))
            pkgs.git
            pkgs.util-linux
            # The embedded Python scripts/build-omp-helper.sh fetches
            # (crates/py/scripts/fetch-python.sh in vendor/oh-my-pi).
            pkgs.curl
            pkgs.zstd
            pkgs.uv
            pkgs.patchelf
            # Native parts of the OMP helper: libopus through pkg-config, and
            # CMake builds, which oh-my-pi's Cargo config runs with Ninja.
            pkgs.pkg-config
            pkgs.cmake
            pkgs.ninja
          ];
          buildInputs = [ pkgs.libopus ];

          # NixOS has no global library path, so binaries find their libraries
          # by RUNPATH. rustc links x86-64 Linux binaries with its bundled LLD,
          # which skips the Nix linker wrapper that adds the RUNPATH entries
          # for linked libraries (the OMP helper's libopus). Link with Nix's
          # wrapped LLD instead, found through -B rather than PATH so LLVM's ar
          # and nm don't replace binutils' for C builds; LLD, not ld.bfd, as
          # the helper's static libpython names its archives in an order only
          # LLD resolves. Sidestep dlopens libfontconfig.so.1 to find fonts,
          # which no wrapper sees; without it the app draws no text.
          env.RUSTFLAGS = toString [
            "-C linker-features=-lld"
            "-C link-arg=-fuse-ld=lld"
            "-C link-arg=-B${pkgs.llvmPackages.bintools}/bin"
            "-C link-arg=-Wl,-rpath,${pkgs.lib.makeLibraryPath [ pkgs.fontconfig ]}"
          ];
          # Keep nix develop's ./outputs/out/lib out of the binary's RUNPATH.
          env.NIX_NO_SELF_RPATH = "1";
          # Aizome's type for tests, screenshots and runs outside the session.
          env.AIZOME_FONT_DIR = "${pkgs.callPackage ./nix/aizome-fonts.nix { }}";
        };
      });

      formatter = forAllSystems (pkgs: pkgs.nixfmt);
    };
}
