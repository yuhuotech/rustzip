#!/usr/bin/env bash
# Builds RustZIP.app and a disk image.
#
#   packaging/macos/build-app.sh [output-dir]
#
# Environment:
#   TARGET                 Rust target triple, default: the machine's own
#   MACOS_SIGN_IDENTITY    "Developer ID Application: ..." to sign for distribution.
#                          Without it the app is ad-hoc signed: it runs on this Mac, and
#                          other Macs ask the user to approve it in System Settings.
#   NOTARY_PROFILE         a `xcrun notarytool store-credentials` profile; notarizes the dmg
#   RZ_WITH_RAR            set to 1 for an opt-in RAR build and association
set -euo pipefail

root="$(cd "$(dirname "$0")/../.." && pwd)"
out="${1:-$root/dist}"
target="${TARGET:-}"
case "${RZ_WITH_RAR:-0}" in
    0|1) ;;
    *) echo "RZ_WITH_RAR must be 0 or 1" >&2; exit 2 ;;
esac
set --
rar_document_type=""
if [ "${RZ_WITH_RAR:-0}" = "1" ]; then
    set -- --features rar
    rar_document_type="<string>rar</string>"
fi
version="$(awk -F'"' '/^version *=/ {print $2; exit}' "$root/Cargo.toml")"

cd "$root"
if [ -n "$target" ]; then
    cargo build --release -p rustzip-gui --target "$target" "$@"
    bin="target/$target/release/rustzip"
    arch="${target%%-*}"
else
    cargo build --release -p rustzip-gui "$@"
    bin="target/release/rustzip"
    arch="$(uname -m)"
fi
[ "$arch" = "arm64" ] && arch="aarch64"

app="$out/RustZIP.app"
rm -rf "$app"
mkdir -p "$app/Contents/MacOS" "$app/Contents/Resources"
cp "$bin" "$app/Contents/MacOS/rustzip"
cp packaging/icons/RustZIP.icns "$app/Contents/Resources/RustZIP.icns"
cp packaging/licenses/libchm.txt "$app/Contents/Resources/libchm-license.txt"
cp packaging/licenses/pbz2.txt "$app/Contents/Resources/pbz2-license.txt"
cp packaging/licenses/pbz2-notice.txt "$app/Contents/Resources/pbz2-notice.txt"
if [ "${RZ_WITH_RAR:-0}" = "1" ]; then
    cp packaging/licenses/unrar.txt "$app/Contents/Resources/unrar-license.txt"
fi
sed -e "s/@VERSION@/$version/g" -e "s|@RAR_DOCUMENT_TYPE@|$rar_document_type|" \
    packaging/macos/Info.plist.in > "$app/Contents/Info.plist"
plutil -lint "$app/Contents/Info.plist"

identity="${MACOS_SIGN_IDENTITY:--}"
if [ "$identity" = "-" ]; then
    codesign --force --deep --sign - "$app"
else
    codesign --force --deep --options runtime --timestamp --sign "$identity" "$app"
fi
codesign --verify --deep --strict "$app"

dmg="$out/RustZIP-$version-macos-$arch.dmg"
stage="$(mktemp -d)"
cp -R "$app" "$stage/"
ln -s /Applications "$stage/Applications"
rm -f "$dmg"
hdiutil create -quiet -volname "RustZIP" -srcfolder "$stage" -ov -format UDZO "$dmg"
rm -rf "$stage"

if [ -n "${NOTARY_PROFILE:-}" ] && [ "$identity" != "-" ]; then
    xcrun notarytool submit "$dmg" --keychain-profile "$NOTARY_PROFILE" --wait
    xcrun stapler staple "$dmg"
fi
echo "$dmg"
