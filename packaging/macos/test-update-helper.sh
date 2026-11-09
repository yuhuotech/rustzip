#!/usr/bin/env bash
set -euo pipefail

root="$(cd "$(dirname "$0")/../.." && pwd)"
work="$(mktemp -d "${TMPDIR:-/tmp}/rustzip-update-helper.XXXXXX")"
helper="$work/RustZIPUpdateHelper"
target="$work/installed/RustZIP.app"
stage="$work/stage/RustZIP.app"
pid_file="$work/smoke.pid"
finished="$work/smoke.finished"
running_pid=""
helper_pid=""
smoke_pid=""
cleanup() {
    [[ -z "$running_pid" ]] || kill "$running_pid" 2>/dev/null || true
    [[ -z "$helper_pid" ]] || kill "$helper_pid" 2>/dev/null || true
    [[ -z "$smoke_pid" ]] || kill "$smoke_pid" 2>/dev/null || true
    rm -rf "$work"
}
trap cleanup EXIT

xcrun swiftc -D RUSTZIP_UPDATER_TESTING -warnings-as-errors -O \
    -o "$helper" "$root/packaging/macos/update-helper.swift"
case "$(uname -m)" in
    arm64) platform="macos-aarch64" ;;
    x86_64) platform="macos-x86_64" ;;
    *) exit 1 ;;
esac
sign_update() {
    swift -e 'import CryptoKit; import Foundation; let k = try Curve25519.Signing.PrivateKey(rawRepresentation: Data(repeating: 7, count: 32)); let m = "rustzip-update-v1\n\(CommandLine.arguments[1])\n\(CommandLine.arguments[2])\n\(CommandLine.arguments[3])"; print(try k.signature(for: Data(m.utf8)).map { String(format:"%02x", $0) }.joined())' "$1" "$2" "$3"
}
cargo build --manifest-path "$root/Cargo.toml" -p rz-updater --features test-fixture --bin rz-update-fixture
cp "${CARGO_TARGET_DIR:-$root/target}/debug/rz-update-fixture" "$work/RustZIPSmokeApp"

make_app() {
    local app="$1" version="$2" startup_failure="${3:-false}"
    mkdir -p "$app/Contents/MacOS" "$app/Contents/Resources"
    python3 -c 'import plistlib,sys; path,version=sys.argv[1:]; plistlib.dump({"CFBundleIdentifier":"com.yuhuotech.rustzip.updater-smoke","CFBundleName":"RustZIP Update Smoke","CFBundlePackageType":"APPL","CFBundleExecutable":"RustZIPSmokeApp","CFBundleShortVersionString":version,"CFBundleVersion":version},open(path,"wb"))' \
        "$app/Contents/Info.plist" "$version"
    cp "$work/RustZIPSmokeApp" "$app/Contents/MacOS/RustZIPSmokeApp"
    python3 -c 'import json,sys; json.dump({"pid_file":sys.argv[2],"finished":sys.argv[3],"sleep_ms":60000,"fail":sys.argv[4]=="true"},open(sys.argv[1],"w"))' \
        "$app/Contents/Resources/smoke-config.json" "$pid_file" "$finished" "$startup_failure"
    ln -s RustZIPSmokeApp "$app/Contents/MacOS/smoke-link"
    codesign --force --sign - "$app/Contents/MacOS/RustZIPSmokeApp"
    codesign --force --sign - "$app"
    codesign --verify --deep --strict "$app"
}

app_version() {
    /usr/libexec/PlistBuddy -c 'Print :CFBundleShortVersionString' "$target/Contents/Info.plist"
}

make_app "$target" 0.1.0
for format in zip dmg; do
    if [[ "$format" == zip ]]; then
        old_version=0.1.0
        new_version=0.1.1
    else
        old_version=0.1.1
        new_version=0.1.2
    fi
    rm -rf "$work/stage"
    make_app "$stage" "$new_version"
    download="$work/download-$format"
    mkdir -p "$download"
    archive="$download/update.$format"
    if [[ "$format" == zip ]]; then
        ditto -c -k --sequesterRsrc --keepParent "$stage" "$archive"
    else
        hdiutil create -quiet -volname RustZIP -srcfolder "$work/stage" -ov -format UDZO "$archive"
    fi
    checksum="$(shasum -a 256 "$archive" | awk '{print $1}')"
    asset_platform="$platform"
    [[ "$format" != zip ]] || asset_platform="$platform-zip"
    signature="$(sign_update "$new_version" "$asset_platform" "$checksum")"
    /bin/sleep 60 &
    running_pid=$!
    "$helper" --app "$target" --archive "$archive" --format "$format" --sha256 "$checksum" \
        --version "$new_version" --language en-US --pid "$running_pid" \
        --platform "$asset_platform" --signature "$signature" &
    helper_pid=$!
    sleep 1
    [[ "$(app_version)" == "$old_version" ]]
    kill -0 "$helper_pid"
    kill "$running_pid"
    wait "$running_pid" 2>/dev/null || true
    running_pid=""

    helper_exited=0
    for attempt in {1..120}; do
        if ! kill -0 "$helper_pid" 2>/dev/null; then
            helper_exited=1
            break
        fi
        sleep 0.25
    done
    if [[ "$helper_exited" != 1 ]]; then
        echo "Update helper did not finish while the relaunched app was running." >&2
        exit 1
    fi
    wait "$helper_pid"
    helper_pid=""
    [[ ! -f "$finished" ]]
    for attempt in {1..40}; do
        [[ -f "$pid_file" ]] && break
        sleep 0.25
    done
    [[ -f "$pid_file" ]]
    smoke_pid="$(< "$pid_file")"
    kill "$smoke_pid" 2>/dev/null || true
    for attempt in {1..40}; do
        kill -0 "$smoke_pid" 2>/dev/null || break
        sleep 0.1
    done
    smoke_pid=""
    rm -f "$pid_file"
    [[ "$(app_version)" == "$new_version" ]]
    [[ -L "$target/Contents/MacOS/smoke-link" ]]
    codesign --verify --deep --strict "$target"
    [[ ! -e "$download" ]]
    [[ -z "$(find "$work/installed" -maxdepth 1 -name '.RustZIP-backup-*.app' -print -quit)" ]]
done

# A file modified after the application's Ed25519 verification must never be extracted.
download="$work/download-tampered"
mkdir -p "$download"
printf 'tampered' > "$download/update.zip"
/bin/sleep 0.1 &
running_pid=$!
wait "$running_pid"
finished_pid="$running_pid"
running_pid=""
if "$helper" --app "$target" --archive "$download/update.zip" --format zip \
    --sha256 0000000000000000000000000000000000000000000000000000000000000000 \
    --version 0.1.3 --language en-US --pid "$finished_pid" \
    --platform "$platform-zip" --signature "$(sign_update 0.1.3 "$platform-zip" 0000000000000000000000000000000000000000000000000000000000000000)"; then
    echo "Tampered update was accepted." >&2
    exit 1
fi
[[ "$(app_version)" == "0.1.2" ]]
codesign --verify --deep --strict "$target"

# A correctly signed replacement that exits before its startup receipt rolls back.
rm -rf "$work/stage"
make_app "$stage" 0.1.3 true
download="$work/download-startup-failure"
mkdir -p "$download"
ditto -c -k --sequesterRsrc --keepParent "$stage" "$download/update.zip"
checksum="$(shasum -a 256 "$download/update.zip" | awk '{print $1}')"
if "$helper" --app "$target" --archive "$download/update.zip" --format zip --sha256 "$checksum" \
    --version 0.1.3 --language en-US --pid 0 --platform "$platform-zip" \
    --signature "$(sign_update 0.1.3 "$platform-zip" "$checksum")"; then
    echo "A replacement without a startup receipt was accepted." >&2; exit 1
fi
[[ "$(app_version)" == 0.1.2 ]]

# Simulate a process loss after publication and recover from the persisted journal.
rm -rf "$work/stage"
make_app "$stage" 0.1.3
download="$work/download-interrupted"
mkdir -p "$download"
ditto -c -k --sequesterRsrc --keepParent "$stage" "$download/update.zip"
checksum="$(shasum -a 256 "$download/update.zip" | awk '{print $1}')"
if "$helper" --app "$target" --archive "$download/update.zip" --format zip --sha256 "$checksum" \
    --version 0.1.3 --language en-US --pid 0 --platform "$platform-zip" --fault-after-swap \
    --signature "$(sign_update 0.1.3 "$platform-zip" "$checksum")"; then
    echo "The interruption injection did not stop the updater." >&2; exit 1
fi
[[ "$(app_version)" == 0.1.3 ]]
"$helper" --recover --app "$target" --pid 0
[[ "$(app_version)" == 0.1.2 ]]
for attempt in {1..40}; do [[ ! -f "$pid_file" ]] || break; sleep 0.1; done
if [[ -f "$pid_file" ]]; then smoke_pid="$(< "$pid_file")"; kill "$smoke_pid" 2>/dev/null || true; smoke_pid=""; rm -f "$pid_file"; fi

# Even a correctly checksummed archive must contain the version in the signed manifest.
rm -rf "$work/stage"
make_app "$stage" 0.1.2
download="$work/download-wrong-version"
mkdir -p "$download"
ditto -c -k --sequesterRsrc --keepParent "$stage" "$download/update.zip"
checksum="$(shasum -a 256 "$download/update.zip" | awk '{print $1}')"
/bin/sleep 0.1 &
running_pid=$!
wait "$running_pid"
finished_pid="$running_pid"
running_pid=""
if "$helper" --app "$target" --archive "$download/update.zip" --format zip \
    --sha256 "$checksum" --version 0.1.3 --language en-US --pid "$finished_pid" \
    --platform "$platform-zip" --signature "$(sign_update 0.1.4 "$platform-zip" "$checksum")"; then
    echo "A signature for a different version was accepted." >&2
    exit 1
fi
mkdir -p "$download"
ditto -c -k --sequesterRsrc --keepParent "$stage" "$download/update.zip"
checksum="$(shasum -a 256 "$download/update.zip" | awk '{print $1}')"
if "$helper" --app "$target" --archive "$download/update.zip" --format zip \
    --sha256 "$checksum" --version 0.1.3 --language en-US --pid "$finished_pid" \
    --platform "$platform-zip" --signature "$(sign_update 0.1.3 "$platform-zip" "$checksum")"; then
    echo "Update with the wrong app version was accepted." >&2
    exit 1
fi
[[ "$(app_version)" == "0.1.2" ]]
codesign --verify --deep --strict "$target"

# Exercise the exact controller protocol used by the GUI, including handoff acknowledgement.
rm -rf "$work/stage"
make_app "$stage" 0.1.3
download="$work/download-controller"
mkdir -p "$download"
ditto -c -k --sequesterRsrc --keepParent "$stage" "$download/update.zip"
checksum="$(shasum -a 256 "$download/update.zip" | awk '{print $1}')"
"$work/RustZIPSmokeApp" --fixture-macos-handoff "$helper" --app "$target" --archive "$download/update.zip" \
    --format zip --sha256 "$checksum" --version 0.1.3 --language en-US --pid 0 \
    --platform "$platform-zip" --signature "$(sign_update 0.1.3 "$platform-zip" "$checksum")"
for attempt in {1..300}; do
    if [[ "$(app_version)" == 0.1.3 && ! -e "$work/installed/.RustZIP.app.rustzip-update.json" && -f "$pid_file" ]]; then break; fi
    sleep 0.1
done
[[ "$(app_version)" == 0.1.3 ]]
[[ ! -e "$work/installed/.RustZIP.app.rustzip-update.json" ]]
smoke_pid="$(< "$pid_file")"
kill "$smoke_pid" 2>/dev/null || true
smoke_pid=""
rm -f "$pid_file"
echo "macOS update helper, controller handoff, startup receipt and recovery passed."
