{pkgs ? import <nixpkgs> {} }:
  pkgs.clangStdenv.mkDerivation {
    name = "devShell";
    buildInputs = with pkgs; [
      rustc
      cargo

      meson
      cmocka
      pkgconfig
      json_c
      ninja
      libclang.lib
    ];
    hardeningDisable = [ "all" ];
    LIBCLANG_PATH = "${pkgs.libclang.lib}/lib";
  }
