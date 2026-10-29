#!/usr/bin/env sh
# Installs RustZIP for the current user. Remove with: ./install.sh --uninstall
set -eu
here="$(cd "$(dirname "$0")" && pwd)"
prefix="${PREFIX:-$HOME/.local}"
apps="$prefix/share/applications"
data_home="${XDG_DATA_HOME:-$prefix/share}"
nautilus_scripts="$data_home/nautilus/scripts"
nautilus_binary="$nautilus_scripts/.rustzip/rustzip"
kde6_service_menus="$data_home/kio/servicemenus"
kde5_service_menus="$data_home/kservices5/ServiceMenus"
nemo_actions="$data_home/nemo/actions"
if [ "${1:-}" = "--uninstall" ]; then
    rm -f "$prefix/bin/rustzip" "$apps/rustzip.desktop" \
          "$kde6_service_menus/rustzip-servicemenu.desktop" \
          "$kde5_service_menus/rustzip-servicemenu.desktop" \
          "$prefix/share/kservices5/ServiceMenus/rustzip-servicemenu.desktop" \
          "$nemo_actions/rustzip-compress.nemo_action" \
          "$nemo_actions/rustzip-extract.nemo_action" \
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
    "$kde6_service_menus/rustzip-servicemenu.desktop"
install -Dm755 "$here/rustzip-servicemenu.desktop" \
    "$kde5_service_menus/rustzip-servicemenu.desktop"
install -Dm644 "$here/rustzip-compress.nemo_action" \
    "$nemo_actions/rustzip-compress.nemo_action"
install -Dm644 "$here/rustzip-extract.nemo_action" \
    "$nemo_actions/rustzip-extract.nemo_action"
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
echo "Restart Nautilus, Dolphin, and Nemo to show the RustZIP actions."
