#!/usr/bin/env bash
# Builds RustZIP-<version>-linux-<arch>.tar.gz: the program, desktop entry, icons, file
# manager menu entries and an install script that needs no root (installs under ~/.local).
#
#   packaging/linux/build-tarball.sh [output-dir]
#   RZ_WITH_RAR=1 packaging/linux/build-tarball.sh [output-dir]
set -euo pipefail

root="$(cd "$(dirname "$0")/../.." && pwd)"
out="${1:-$root/dist}"
version="$(awk -F'"' '/^version *=/ {print $2; exit}' "$root/Cargo.toml")"
arch="$(uname -m)"
name="RustZIP-$version-linux-$arch"

cd "$root"
case "${RZ_WITH_RAR:-0}" in
    0|1) ;;
    *) echo "RZ_WITH_RAR must be 0 or 1" >&2; exit 2 ;;
esac
set --
if [ "${RZ_WITH_RAR:-0}" = "1" ]; then
    set -- --features rustzip-gui/rar
fi
cargo build --release -p rustzip-gui -p rz-pdf-renderer "$@"

stage="$(mktemp -d)/$name"
mkdir -p "$stage/icons"
cp target/release/rustzip "$stage/"
cp target/release/rz-pdf-renderer "$stage/"
cp packaging/linux/*.desktop packaging/linux/*.nemo_action packaging/linux/rustzip-nautilus-* "$stage/"
cp packaging/linux/install.sh "$stage/install.sh"
cp packaging/licenses/libchm.txt "$stage/libchm-license.txt"
cp packaging/licenses/pbz2.txt "$stage/pbz2-license.txt"
cp packaging/licenses/pbz2-notice.txt "$stage/pbz2-notice.txt"
cp packaging/licenses/pdf-renderer.txt "$stage/pdf-renderer-licenses.txt"
cp packaging/licenses/audio-playback.txt "$stage/audio-playback-licenses.txt"
cp packaging/licenses/rodio-mit.txt "$stage/rodio-mit.txt"
cp packaging/licenses/rodio-apache-2.0.txt "$stage/rodio-apache-2.0.txt"
cp packaging/licenses/cpal-apache-2.0.txt "$stage/cpal-apache-2.0.txt"
cp packaging/licenses/symphonia-mpl-2.0.txt "$stage/symphonia-mpl-2.0.txt"
if [ "${RZ_WITH_RAR:-0}" = "1" ]; then
    sed 's|MimeType=|MimeType=application/vnd.rar;application/x-rar;|' \
        "$stage/rustzip.desktop" > "$stage/rustzip.desktop.tmp"
    mv "$stage/rustzip.desktop.tmp" "$stage/rustzip.desktop"
    sed 's|Mimetypes=|Mimetypes=application/vnd.rar;|' \
        "$stage/rustzip-extract.nemo_action" > "$stage/rustzip-extract.nemo_action.tmp"
    mv "$stage/rustzip-extract.nemo_action.tmp" "$stage/rustzip-extract.nemo_action"
    cp packaging/licenses/unrar.txt "$stage/unrar-license.txt"
fi
cp packaging/icons/hicolor/*.png "$stage/icons/"
chmod +x "$stage/install.sh" "$stage/rustzip" "$stage/rz-pdf-renderer"

mkdir -p "$out"
tar -C "$(dirname "$stage")" -czf "$out/$name.tar.gz" "$name"
rm -rf "$(dirname "$stage")"
echo "$out/$name.tar.gz"
