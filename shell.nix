{ pkgs ? import <nixpkgs> { } }:

pkgs.mkShell {
  nativeBuildInputs = with pkgs; [
    cargo
    rustc
    rustfmt
    clippy
    nodejs_24
    python3
    pkg-config
    gnumake
    git
    wrapGAppsHook3
  ];

  buildInputs = with pkgs; [
    gtk3
    webkitgtk_4_1
    libsoup_3
    librsvg
    glib-networking
  ];

  # Cargo runs the binary directly, without the wrapper produced by a Nix
  # package build. Supply the same runtime resources in the development shell.
  shellHook = ''
    export XDG_DATA_DIRS="${pkgs.gtk3}/share/gsettings-schemas/${pkgs.gtk3.name}:${pkgs.gsettings-desktop-schemas}/share/gsettings-schemas/${pkgs.gsettings-desktop-schemas.name}:''${XDG_DATA_DIRS:-}"
    export GIO_EXTRA_MODULES="${pkgs.glib-networking}/lib/gio/modules''${GIO_EXTRA_MODULES:+:$GIO_EXTRA_MODULES}"
  '';
}
