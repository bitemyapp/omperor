# The font files Aizome draws with, in one folder, as the kit looks for them
# in $AIZOME_FONT_DIR: Archivo (from Google Fonts), IBM Plex Mono and Noto
# Sans Symbols 2, the files Aizome's session installs. The dev shell points
# the variable here, so tests and screenshots set the type the desktop
# shows. A copy of Sekka's desktop/aizome/nix/{fonts,aizome-fonts}.nix: Nix
# can't read the vendor/sekka submodule from this flake's source.
{
  lib,
  fetchurl,
  runCommand,
  ibm-plex,
  noto-fonts,
}:
let
  rev = "95f4904fc8bcf26d3420fe315560c96417c6dec7";
  base = "https://raw.githubusercontent.com/google/fonts/${rev}/ofl/archivo";
  archivo = fetchurl {
    name = "archivo-vf.ttf";
    url = "${base}/Archivo%5Bwdth,wght%5D.ttf";
    hash = "sha256-DglKfTx8TCXPExDEswAU8drpMyIgscLIj0+plvCwUFM=";
  };
  archivoItalic = fetchurl {
    name = "archivo-italic-vf.ttf";
    url = "${base}/Archivo-Italic%5Bwdth,wght%5D.ttf";
    hash = "sha256-MF0TtOuA5i99UX9418BFJQ2XdVKkIPLb7ZBvMU52EwU=";
  };
  archivoLicense = fetchurl {
    name = "archivo-OFL.txt";
    url = "${base}/OFL.txt";
    hash = "sha256-EItOV8nHltPTjQQoyn7jneR62TGHMCcY2bLYhkubcWs=";
  };
in
runCommand "aizome-fonts"
  {
    meta = {
      description = "Archivo, IBM Plex Mono and Noto Sans Symbols 2, as Aizome's session installs them";
      license = lib.licenses.ofl;
      platforms = lib.platforms.all;
    };
  }
  ''
    mkdir -p $out
    ln -s ${archivo} $out/Archivo-VF.ttf
    ln -s ${archivoItalic} $out/Archivo-Italic-VF.ttf
    ln -s ${archivoLicense} $out/OFL-Archivo.txt
    for face in Regular Medium SemiBold Bold Italic; do
      ln -s ${ibm-plex}/share/fonts/truetype/IBMPlexMono-$face.ttf $out/
    done
    ln -s ${noto-fonts}/share/fonts/noto/NotoSansSymbols2-Regular.otf $out/
  ''
