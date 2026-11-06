#!/usr/bin/env bash
set -euo pipefail

root="$(cd "$(dirname "$0")/../.." && pwd)"
tmp="$(mktemp -d)"
trap 'rm -rf "$tmp"' EXIT

app="$tmp/RustZIPProbe.app"
mkdir -p "$app/Contents/MacOS" "$app/Contents/Resources"
cp "$root/packaging/macos/tests/bundle-probe-Info.plist" "$app/Contents/Info.plist"

libb2_prefix="$(brew --prefix libb2)"
lz4_prefix="$(brew --prefix lz4)"
cc -O2 -o "$app/Contents/MacOS/probe" \
    "$root/packaging/macos/tests/bundle-probe.c" \
    -I"$libb2_prefix/include" -I"$lz4_prefix/include" \
    "$libb2_prefix/lib/libb2.dylib" "$lz4_prefix/lib/liblz4.dylib"

bash "$root/packaging/macos/bundle-homebrew-libraries.sh" "$app" -
codesign --force --sign - "$app/Contents/MacOS/probe"
codesign --force --sign - "$app"
bash "$root/packaging/macos/verify-portable-libraries.sh" "$app"
"$app/Contents/MacOS/probe"
