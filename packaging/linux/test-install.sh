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
cp "$root"/packaging/icons/hicolor/*.png "$package/icons/"
cp "$root"/packaging/linux/install.sh "$package/install.sh"
printf 'test executable\n' > "$package/rustzip"
chmod +x "$package/rustzip"

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

PREFIX="$prefix" XDG_DATA_HOME="$xdg" sh "$package/install.sh" --uninstall
for path in "${installed[@]}"; do
    test ! -e "$path" || { printf 'menu remained after uninstall: %s\n' "$path" >&2; exit 1; }
done
test ! -e "$prefix/bin/rustzip"
test ! -e "$xdg/nautilus/scripts/.rustzip/rustzip"
printf 'Linux desktop integration install/uninstall smoke test passed.\n'
