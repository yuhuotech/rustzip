#!/usr/bin/env bash
# Builds RustZIP.app, an installer disk image, and an update ZIP.
#
#   packaging/macos/build-app.sh [output-dir]
#
# Environment:
#   TARGET                 Rust target triple, default: the machine's own
#   MACOS_SIGN_IDENTITY    "Developer ID Application: ..." to sign for distribution.
#                          Without it the app is ad-hoc signed: it runs on this Mac, and
#                          other Macs ask the user to approve it in System Settings.
#   NOTARY_PROFILE         a `xcrun notarytool store-credentials` profile; notarizes the dmg
#   NOTARY_DIRECT          set to 1 to use APPLE_ID, APPLE_TEAM_ID and APPLE_APP_PASSWORD
#   RZ_WITH_RAR            set to 1 for an opt-in RAR build and association
set -euo pipefail
trap 'status=$?; printf "macOS package failed at line %s: %s (exit %s)\n" "${BASH_LINENO[0]:-$LINENO}" "$BASH_COMMAND" "$status" >&2; exit "$status"' ERR

root="$(cd "$(dirname "$0")/../.." && pwd)"
out="${1:-$root/dist}"
target="${TARGET:-}"
build_root="${CARGO_TARGET_DIR:-target}"
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
python3 -c 'import re,sys; rust,swift=(open(p,encoding="utf-8").read() for p in sys.argv[1:]); key="".join(re.findall(r"0x([0-9a-fA-F]{2})",rust)).lower(); keys=re.findall(r"let keyHex = \"([0-9a-f]{64})\"",swift); assert len(keys)==2 and keys[-1]==key, "macOS helper update key differs from the Rust client"' \
    crates/rz-services/src/update_key.in packaging/macos/update-helper.swift
if [ -n "$target" ]; then
    cargo build --release -p rustzip-gui -p rz-pdf-renderer -p rz-video-renderer --target "$target" "$@"
    bin="$build_root/$target/release/rustzip"
    pdf_renderer="$build_root/$target/release/rz-pdf-renderer"
    video_renderer="$build_root/$target/release/rz-video-renderer"
    arch="${target%%-*}"
else
    cargo build --release -p rustzip-gui -p rz-pdf-renderer -p rz-video-renderer "$@"
    bin="$build_root/release/rustzip"
    pdf_renderer="$build_root/release/rz-pdf-renderer"
    video_renderer="$build_root/release/rz-video-renderer"
    arch="$(uname -m)"
fi
[ "$arch" = "arm64" ] && arch="aarch64"

app="$out/RustZIP.app"
rm -rf "$app"
finder_extension="$app/Contents/PlugIns/RustZIPFinderSync.appex"
quicklook_extension="$app/Contents/PlugIns/RustZIPQuickLook.appex"
mkdir -p "$app/Contents/MacOS" "$app/Contents/Resources" "$app/Contents/Frameworks" \
    "$app/Contents/Helpers" \
    "$finder_extension/Contents/MacOS" "$finder_extension/Contents/Resources" \
    "$quicklook_extension/Contents/MacOS" "$quicklook_extension/Contents/Resources"
cp "$bin" "$app/Contents/MacOS/rustzip"
cp "$pdf_renderer" "$app/Contents/MacOS/rz-pdf-renderer"
cp "$video_renderer" "$app/Contents/MacOS/rz-video-renderer"
cp packaging/icons/RustZIP.icns "$app/Contents/Resources/RustZIP.icns"
cp packaging/licenses/libchm.txt "$app/Contents/Resources/libchm-license.txt"
cp packaging/licenses/format-readers.txt "$app/Contents/Resources/format-readers-licenses.txt"
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
cp packaging/macos/update-helper-strings.tsv "$app/Contents/Resources/update-helper-strings.tsv"
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
xcrun swiftc -O -target "$swift_target" -sdk "$sdk" \
    -o "$app/Contents/Helpers/RustZIPUpdateHelper" packaging/macos/update-helper.swift

identity="${MACOS_SIGN_IDENTITY:--}"
bash packaging/macos/bundle-homebrew-libraries.sh "$app" "$identity"
if [ "$identity" = "-" ]; then
    codesign --force --sign - "$app/Contents/Helpers/RustZIPUpdateHelper"
    codesign --force --sign - "$app/Contents/MacOS/rz-pdf-renderer"
    codesign --force --sign - "$app/Contents/MacOS/rz-video-renderer"
    codesign --force --entitlements packaging/macos/finder-sync.entitlements \
        --sign - "$finder_extension"
    codesign --force --entitlements packaging/macos/finder-sync.entitlements \
        --sign - "$quicklook_extension"
    codesign --force --sign - "$app"
else
    codesign --force --options runtime --timestamp \
        --sign "$identity" "$app/Contents/Helpers/RustZIPUpdateHelper"
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
bash packaging/macos/verify-portable-libraries.sh "$app"

dmg="$out/RustZIP-$version-macos-$arch.dmg"
zip="$out/RustZIP-$version-macos-$arch.zip"
stage="$(mktemp -d)"
cp -R "$app" "$stage/"
ln -s /Applications "$stage/Applications"
rm -f "$dmg"
hdiutil create -quiet -volname "RustZIP" -srcfolder "$stage" -ov -format UDZO "$dmg"
rm -rf "$stage"

if [ "$identity" != "-" ] && { [ -n "${NOTARY_PROFILE:-}" ] || [ "${NOTARY_DIRECT:-0}" = "1" ]; }; then
    if [ "${NOTARY_DIRECT:-0}" = "1" ]; then
        : "${APPLE_ID:?}" "${APPLE_TEAM_ID:?}" "${APPLE_APP_PASSWORD:?}"
        xcrun notarytool submit "$dmg" --apple-id "$APPLE_ID" \
            --team-id "$APPLE_TEAM_ID" --password "$APPLE_APP_PASSWORD" --wait
    else
        xcrun notarytool submit "$dmg" --keychain-profile "$NOTARY_PROFILE" --wait
    fi
    stapled=0
    for attempt in 1 2 3 4 5; do
        if xcrun stapler staple "$dmg"; then
            stapled=1
            break
        fi
        if [ "$attempt" -lt 5 ]; then
            echo "Notarization ticket delivery failed (attempt $attempt); retrying." >&2
            sleep 10
        fi
    done
    if [ "$stapled" != 1 ]; then
        echo "Could not attach the notarization ticket after five attempts." >&2
        exit 1
    fi
    validated=0
    for attempt in 1 2 3 4 5; do
        if xcrun stapler validate "$dmg"; then
            validated=1
            break
        fi
        if [ "$attempt" -lt 5 ]; then
            echo "Notarization ticket validation failed (attempt $attempt); retrying." >&2
            sleep 10
        fi
    done
    if [ "$validated" != 1 ]; then
        echo "Could not validate the notarization ticket after five attempts." >&2
        exit 1
    fi

    # The update ZIP contains the app itself, so its ticket must be stapled before archiving.
    stapled=0
    for attempt in 1 2 3 4 5; do
        if xcrun stapler staple "$app"; then
            stapled=1
            break
        fi
        if [ "$attempt" -lt 5 ]; then
            echo "App ticket delivery failed (attempt $attempt); retrying." >&2
            sleep 10
        fi
    done
    if [ "$stapled" != 1 ]; then
        echo "Could not attach the app notarization ticket after five attempts." >&2
        exit 1
    fi
    xcrun stapler validate "$app"
    codesign --verify --deep --strict "$app"
fi

rm -f "$zip"
ditto -c -k --sequesterRsrc --keepParent "$app" "$zip"
zip_check="$(mktemp -d)"
ditto -x -k "$zip" "$zip_check"
codesign --verify --deep --strict "$zip_check/RustZIP.app"
if [ "$identity" != "-" ] && { [ -n "${NOTARY_PROFILE:-}" ] || [ "${NOTARY_DIRECT:-0}" = "1" ]; }; then
    xcrun stapler validate "$zip_check/RustZIP.app"
fi
rm -rf "$zip_check"
echo "$dmg"
