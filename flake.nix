{
  description = "Wayflow -- Wayland-native KVM-over-network";

  inputs = {
    nixpkgs.url     = "github:NixOS/nixpkgs/nixos-unstable";
    rust-overlay.url = "github:oxalica/rust-overlay";
    flake-utils.url  = "github:numtide/flake-utils";
  };

  outputs = { self, nixpkgs, rust-overlay, flake-utils }:
    flake-utils.lib.eachDefaultSystem (system:
      let
        overlays = [ (import rust-overlay) ];
        pkgs = import nixpkgs { inherit system overlays; };
        rust = pkgs.rust-bin.stable.latest.default.override {
          extensions = [ "rust-src" "rust-analyzer" "clippy" ];
        };
      in {
        devShells.default = pkgs.mkShell {
          nativeBuildInputs = with pkgs; [
            rust
            pkg-config
            cargo-watch
            cargo-nextest
          ];
          buildInputs = with pkgs; [
            # Wayland / libei for input capture + injection
            libei
            wayland
            wayland-protocols
            wayland-scanner

            # XDG portal (ashpd dep)
            xdg-desktop-portal

            # For nested compositor testing
            weston
          ];
          shellHook = ''
            export RUST_LOG=wayflow=debug
            alias wf-test='weston --socket=wayland-test &'
          '';
        };
      });
}
