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
    set -- --features rar
fi
cargo build --release -p rustzip-gui "$@"

stage="$(mktemp -d)/$name"
mkdir -p "$stage/icons"
cp target/release/rustzip "$stage/"
cp packaging/linux/*.desktop packaging/linux/*.nemo_action packaging/linux/rustzip-nautilus-* "$stage/"
cp packaging/licenses/libchm.txt "$stage/libchm-license.txt"
cp packaging/licenses/pbz2.txt "$stage/pbz2-license.txt"
cp packaging/licenses/pbz2-notice.txt "$stage/pbz2-notice.txt"
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
cat > "$stage/install.sh" <<'EOF'
#!/usr/bin/env sh
# Installs RustZIP for the current user. Remove with: ./install.sh --uninstall
set -eu
here="$(cd "$(dirname "$0")" && pwd)"
prefix="${PREFIX:-$HOME/.local}"
apps="$prefix/share/applications"
data_home="${XDG_DATA_HOME:-$prefix/share}"
nautilus_scripts="$data_home/nautilus/scripts"
nautilus_binary="$nautilus_scripts/.rustzip/rustzip"
if [ "${1:-}" = "--uninstall" ]; then
    rm -f "$prefix/bin/rustzip" "$apps/rustzip.desktop" \
          "$prefix/share/kservices5/ServiceMenus/rustzip-servicemenu.desktop" \
          "$prefix/share/nemo/actions/rustzip-compress.nemo_action" \
          "$prefix/share/nemo/actions/rustzip-extract.nemo_action" \
          "$nautilus_scripts/RustZIP - Compress selected items" \
          "$nautilus_scripts/RustZIP - Extract archives here"
    rm -f "$prefix/share/doc/rustzip/unrar-license.txt"
    rm -f "$prefix/share/doc/rustzip/libchm-license.txt"
    rm -f "$prefix/share/doc/rustzip/pbz2-license.txt" "$prefix/share/doc/rustzip/pbz2-notice.txt"
    rm -f "$nautilus_binary"
    rmdir "$nautilus_scripts/.rustzip" 2>/dev/null || true
    for s in 16 32 48 64 128 256 512; do
        rm -f "$prefix/share/icons/hicolor/${s}x${s}/apps/rustzip.png"
    done
    echo "RustZIP removed."
    exit 0
fi
install -Dm755 "$here/rustzip" "$prefix/bin/rustzip"
install -Dm644 "$here/libchm-license.txt" "$prefix/share/doc/rustzip/libchm-license.txt"
install -Dm644 "$here/pbz2-license.txt" "$prefix/share/doc/rustzip/pbz2-license.txt"
install -Dm644 "$here/pbz2-notice.txt" "$prefix/share/doc/rustzip/pbz2-notice.txt"
if [ -f "$here/unrar-license.txt" ]; then
    install -Dm644 "$here/unrar-license.txt" "$prefix/share/doc/rustzip/unrar-license.txt"
fi
install -Dm644 "$here/rustzip.desktop" "$apps/rustzip.desktop"
install -Dm755 "$here/rustzip-servicemenu.desktop" \
    "$prefix/share/kservices5/ServiceMenus/rustzip-servicemenu.desktop"
install -Dm644 "$here/rustzip-compress.nemo_action" "$prefix/share/nemo/actions/rustzip-compress.nemo_action"
install -Dm644 "$here/rustzip-extract.nemo_action" "$prefix/share/nemo/actions/rustzip-extract.nemo_action"
install -d "$nautilus_scripts/.rustzip"
if [ -e "$nautilus_binary" ] || [ -L "$nautilus_binary" ]; then
    if [ ! -L "$nautilus_binary" ] || [ "$(readlink "$nautilus_binary")" != "$prefix/bin/rustzip" ]; then
        echo "Refusing to replace an existing Nautilus helper at $nautilus_binary" >&2
        exit 1
    fi
else
    ln -s "$prefix/bin/rustzip" "$nautilus_binary"
fi
install -Dm755 "$here/rustzip-nautilus-compress" \
    "$nautilus_scripts/RustZIP - Compress selected items"
install -Dm755 "$here/rustzip-nautilus-extract-here" \
    "$nautilus_scripts/RustZIP - Extract archives here"
for s in 16 32 48 64 128 256 512; do
    install -Dm644 "$here/icons/$s.png" "$prefix/share/icons/hicolor/${s}x${s}/apps/rustzip.png"
done
command -v update-desktop-database >/dev/null 2>&1 && update-desktop-database "$apps" || true
command -v gtk-update-icon-cache >/dev/null 2>&1 && gtk-update-icon-cache -q "$prefix/share/icons/hicolor" || true
echo "RustZIP installed to $prefix. Make sure $prefix/bin is on your PATH."
echo "Restart Nautilus to show the RustZIP actions in its Scripts submenu."
EOF
chmod +x "$stage/install.sh" "$stage/rustzip"

mkdir -p "$out"
tar -C "$(dirname "$stage")" -czf "$out/$name.tar.gz" "$name"
rm -rf "$(dirname "$stage")"
echo "$out/$name.tar.gz"
