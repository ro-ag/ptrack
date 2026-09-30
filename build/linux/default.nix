# Wrap a verified release AppImage for NixOS; this never compiles p-track.
{ pkgs ? import <nixpkgs> { }, appimage, version }:
let
  wrapped = pkgs.appimageTools.wrapType2 {
    pname = "ptrack";
    inherit version;
    src = appimage;
    extraPkgs = p: [ p.glib-networking p.wayland ];
  };
  desktop = pkgs.makeDesktopItem {
    name = "p-track";
    desktopName = "p-track";
    comment = "Local project workspace";
    exec = "${wrapped}/bin/ptrack gui";
    icon = "ptrack";
    categories = [ "Development" ];
    startupWMClass = "ptrack";
  };
in pkgs.symlinkJoin {
  name = "ptrack-${version}";
  paths = [ wrapped desktop ];
  postBuild = ''
    install -Dm644 ${../../assets/brand/icon-128.png} $out/share/icons/hicolor/128x128/apps/ptrack.png
  '';
  meta = {
    description = "Local project workspace for plans, tasks, agents, and terminals";
    license = pkgs.lib.licenses.asl20;
    platforms = [ "x86_64-linux" "aarch64-linux" ];
    mainProgram = "ptrack";
  };
}
