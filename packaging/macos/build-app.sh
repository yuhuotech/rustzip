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
trap 'status=$?; printf "macOS package failed at line %s: %s (exit %s)\n" "${BASH_LINENO[0]:-$LINENO}" "$BASH_COMMAND" "$status" >&2; exit "$status"' ERR

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
    set -- --features rustzip-gui/rar
    rar_document_type="<string>rar</string>"
fi
version="$(awk -F'"' '/^version *=/ {print $2; exit}' "$root/Cargo.toml")"

cd "$root"
if [ -n "$target" ]; then
    cargo build --release -p rustzip-gui -p rz-pdf-renderer -p rz-video-renderer --target "$target" "$@"
    bin="target/$target/release/rustzip"
    pdf_renderer="target/$target/release/rz-pdf-renderer"
    video_renderer="target/$target/release/rz-video-renderer"
    arch="${target%%-*}"
else
    cargo build --release -p rustzip-gui -p rz-pdf-renderer -p rz-video-renderer "$@"
    bin="target/release/rustzip"
    pdf_renderer="target/release/rz-pdf-renderer"
    video_renderer="target/release/rz-video-renderer"
    arch="$(uname -m)"
fi
[ "$arch" = "arm64" ] && arch="aarch64"

app="$out/RustZIP.app"
rm -rf "$app"
finder_extension="$app/Contents/PlugIns/RustZIPFinderSync.appex"
quicklook_extension="$app/Contents/PlugIns/RustZIPQuickLook.appex"
mkdir -p "$app/Contents/MacOS" "$app/Contents/Resources" \
    "$finder_extension/Contents/MacOS" "$finder_extension/Contents/Resources" \
    "$quicklook_extension/Contents/MacOS" "$quicklook_extension/Contents/Resources"
cp "$bin" "$app/Contents/MacOS/rustzip"
cp "$pdf_renderer" "$app/Contents/MacOS/rz-pdf-renderer"
cp "$video_renderer" "$app/Contents/MacOS/rz-video-renderer"
cp packaging/icons/RustZIP.icns "$app/Contents/Resources/RustZIP.icns"
cp packaging/licenses/libchm.txt "$app/Contents/Resources/libchm-license.txt"
cp packaging/licenses/pbz2.txt "$app/Contents/Resources/pbz2-license.txt"
cp packaging/licenses/pbz2-notice.txt "$app/Contents/Resources/pbz2-notice.txt"
cp packaging/licenses/pdf-renderer.txt "$app/Contents/Resources/pdf-renderer-licenses.txt"
cp packaging/licenses/audio-playback.txt "$app/Contents/Resources/audio-playback-licenses.txt"
cp packaging/licenses/rodio-mit.txt "$app/Contents/Resources/rodio-mit.txt"
cp packaging/licenses/rodio-apache-2.0.txt "$app/Contents/Resources/rodio-apache-2.0.txt"
cp packaging/licenses/cpal-apache-2.0.txt "$app/Contents/Resources/cpal-apache-2.0.txt"
cp packaging/licenses/symphonia-mpl-2.0.txt "$app/Contents/Resources/symphonia-mpl-2.0.txt"
cp packaging/licenses/video-preview.txt "$app/Contents/Resources/video-preview-licenses.txt"
cp packaging/licenses/reflink-copy-mit.txt "$app/Contents/Resources/reflink-copy-mit.txt"
cp packaging/licenses/mp4-mit.txt "$app/Contents/Resources/mp4-mit.txt"
cp packaging/licenses/rust-h264-mit.txt "$app/Contents/Resources/rust-h264-mit.txt"
cp packaging/licenses/rust-h264-apache-2.0.txt "$app/Contents/Resources/rust-h264-apache-2.0.txt"
if [ "${RZ_WITH_RAR:-0}" = "1" ]; then
    cp packaging/licenses/unrar.txt "$app/Contents/Resources/unrar-license.txt"
fi
sed -e "s/@VERSION@/$version/g" -e "s|@RAR_DOCUMENT_TYPE@|$rar_document_type|" \
    packaging/macos/Info.plist.in > "$app/Contents/Info.plist"
plutil -lint "$app/Contents/Info.plist"
sed -e "s/@VERSION@/$version/g" \
    packaging/macos/finder-sync-Info.plist.in > "$finder_extension/Contents/Info.plist"
plutil -lint "$finder_extension/Contents/Info.plist"
cp -R packaging/macos/finder-sync-locales/. "$finder_extension/Contents/Resources/"
swift_arch="${target%%-*}"
if [ -z "$target" ]; then swift_arch="$(uname -m)"; fi
[ "$swift_arch" = "aarch64" ] && swift_arch="arm64"
swift_target="${swift_arch}-apple-macosx12.0"
sdk="$(xcrun --sdk macosx --show-sdk-path)"
set --
if [ "${RZ_WITH_RAR:-0}" = "1" ]; then
    set -- -D RUSTZIP_WITH_RAR
fi
xcrun swiftc -application-extension -parse-as-library -O \
    -module-name RustZIPFinderSync -target "$swift_target" -sdk "$sdk" \
    -framework AppKit -framework FinderSync "$@" \
    -Xlinker -e -Xlinker _NSExtensionMain \
    -o "$finder_extension/Contents/MacOS/RustZIPFinderSync" \
    packaging/macos/finder-sync.swift
sed -e "s/@VERSION@/$version/g" \
    packaging/macos/quicklook-Info.plist.in > "$quicklook_extension/Contents/Info.plist"
plutil -lint "$quicklook_extension/Contents/Info.plist"
cp -R packaging/macos/quicklook-locales/. "$quicklook_extension/Contents/Resources/"
xcrun swiftc -application-extension -parse-as-library -O \
    -module-name RustZIPQuickLook -target "$swift_target" -sdk "$sdk" \
    -framework AppKit -framework QuickLook -framework QuickLookUI \
    -Xlinker -e -Xlinker _NSExtensionMain \
    -o "$quicklook_extension/Contents/MacOS/RustZIPQuickLook" \
    packaging/macos/quicklook-preview.swift

identity="${MACOS_SIGN_IDENTITY:--}"
if [ "$identity" = "-" ]; then
    codesign --force --sign - "$app/Contents/MacOS/rz-pdf-renderer"
    codesign --force --sign - "$app/Contents/MacOS/rz-video-renderer"
    codesign --force --entitlements packaging/macos/finder-sync.entitlements \
        --sign - "$finder_extension"
    codesign --force --entitlements packaging/macos/finder-sync.entitlements \
        --sign - "$quicklook_extension"
    codesign --force --sign - "$app"
else
    codesign --force --options runtime --timestamp \
        --sign "$identity" "$app/Contents/MacOS/rz-pdf-renderer"
    codesign --force --options runtime --timestamp \
        --sign "$identity" "$app/Contents/MacOS/rz-video-renderer"
    codesign --force --options runtime --timestamp \
        --entitlements packaging/macos/finder-sync.entitlements \
        --sign "$identity" "$finder_extension"
    codesign --force --options runtime --timestamp \
        --entitlements packaging/macos/finder-sync.entitlements \
        --sign "$identity" "$quicklook_extension"
    codesign --force --options runtime --timestamp --sign "$identity" "$app"
fi
codesign --verify --deep --strict "$finder_extension"
codesign --verify --deep --strict "$quicklook_extension"
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
