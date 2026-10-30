#!/usr/bin/env bash
set -euo pipefail

root="$(cd "$(dirname "$0")/../.." && pwd)"
tmp="$(mktemp -d)"
trap 'rm -rf "$tmp"' EXIT
package="$tmp/package"
prefix="$tmp/prefix"
xdg="$tmp/xdg"

mkdir -p "$package/icons"
cp "$root"/packaging/linux/*.desktop \
    "$root"/packaging/linux/*.nemo_action \
    "$root"/packaging/linux/rustzip-nautilus-* "$package/"
cp "$root"/packaging/licenses/libchm.txt "$package/libchm-license.txt"
cp "$root"/packaging/licenses/pbz2.txt "$package/pbz2-license.txt"
cp "$root"/packaging/licenses/pbz2-notice.txt "$package/pbz2-notice.txt"
cp "$root"/packaging/licenses/pdf-renderer.txt "$package/pdf-renderer-licenses.txt"
cp "$root"/packaging/licenses/audio-playback.txt "$package/audio-playback-licenses.txt"
cp "$root"/packaging/licenses/rodio-mit.txt "$package/rodio-mit.txt"
cp "$root"/packaging/licenses/rodio-apache-2.0.txt "$package/rodio-apache-2.0.txt"
cp "$root"/packaging/licenses/cpal-apache-2.0.txt "$package/cpal-apache-2.0.txt"
cp "$root"/packaging/licenses/symphonia-mpl-2.0.txt "$package/symphonia-mpl-2.0.txt"
cp "$root"/packaging/icons/hicolor/*.png "$package/icons/"
cp "$root"/packaging/linux/install.sh "$package/install.sh"
printf 'test executable\n' > "$package/rustzip"
printf 'test PDF renderer\n' > "$package/rz-pdf-renderer"
chmod +x "$package/rustzip" "$package/rz-pdf-renderer"

PREFIX="$prefix" XDG_DATA_HOME="$xdg" sh "$package/install.sh"
installed=(
    "$xdg/kio/servicemenus/rustzip-servicemenu.desktop"
    "$xdg/kservices5/ServiceMenus/rustzip-servicemenu.desktop"
    "$xdg/nemo/actions/rustzip-compress.nemo_action"
    "$xdg/nemo/actions/rustzip-extract.nemo_action"
)
for path in "${installed[@]}"; do
    test -f "$path" || { printf 'missing installed menu: %s\n' "$path" >&2; exit 1; }
done
test -L "$xdg/nautilus/scripts/.rustzip/rustzip"
test -x "$prefix/bin/rz-pdf-renderer"
test -f "$prefix/share/doc/rustzip/pdf-renderer-licenses.txt"
test -f "$prefix/share/doc/rustzip/audio-playback-licenses.txt"
test -f "$prefix/share/doc/rustzip/rodio-mit.txt"
test -f "$prefix/share/doc/rustzip/rodio-apache-2.0.txt"
test -f "$prefix/share/doc/rustzip/cpal-apache-2.0.txt"
test -f "$prefix/share/doc/rustzip/symphonia-mpl-2.0.txt"

PREFIX="$prefix" XDG_DATA_HOME="$xdg" sh "$package/install.sh" --uninstall
for path in "${installed[@]}"; do
    test ! -e "$path" || { printf 'menu remained after uninstall: %s\n' "$path" >&2; exit 1; }
done
test ! -e "$prefix/bin/rustzip"
test ! -e "$prefix/bin/rz-pdf-renderer"
for file in audio-playback-licenses.txt rodio-mit.txt rodio-apache-2.0.txt \
            cpal-apache-2.0.txt symphonia-mpl-2.0.txt; do
    test ! -e "$prefix/share/doc/rustzip/$file"
done
test ! -e "$xdg/nautilus/scripts/.rustzip/rustzip"
printf 'Linux desktop integration install/uninstall smoke test passed.\n'
