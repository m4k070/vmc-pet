{
  description = "vmc-pet: CA/Lenia の場を体として表示するデスクトップ常駐電子ペット";

  inputs = {
    nixpkgs.url = "github:NixOS/nixpkgs/nixos-unstable";
  };

  outputs =
    { self, nixpkgs }:
    let
      # wlr-layer-shell に依存するため Linux 限定
      system = "x86_64-linux";
      pkgs = import nixpkgs { inherit system; };

      # wayland-client クレートが実行時に dlopen する共有ライブラリ群。
      # cargo ビルド成果物には rpath が付かないため LD_LIBRARY_PATH で明示する。
      runtimeLibs = with pkgs; [
        wayland
        libxkbcommon
      ];
    in
    {
      devShells.${system}.default = pkgs.mkShell {
        packages = with pkgs; [
          rustc
          cargo
          rustfmt
          clippy
          rust-analyzer
          pkg-config
        ] ++ runtimeLibs;

        LD_LIBRARY_PATH = pkgs.lib.makeLibraryPath runtimeLibs;

        shellHook = ''
          echo "rustc: $(rustc --version)"
          echo "  run  : cargo run"
          echo "  check: cargo clippy"
        '';
      };
    };
}
