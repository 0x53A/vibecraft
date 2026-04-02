{ pkgs ? import <nixpkgs> {} }:

pkgs.mkShell {
  nativeBuildInputs = with pkgs; [
    # Rust linking
    clang
    lld
    pkg-config

    # Node / frontend
    nodejs
    pnpm
  ];

  # Use clang as the C compiler and lld as the linker
  CARGO_TARGET_X86_64_UNKNOWN_LINUX_GNU_LINKER = "${pkgs.clang}/bin/clang";
  RUSTFLAGS = "-C link-arg=-fuse-ld=lld";
}
