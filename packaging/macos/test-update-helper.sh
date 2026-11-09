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
python3 -c 'import json,sys; code="import Foundation\nlet pid = String(ProcessInfo.processInfo.processIdentifier)\ntry? Data(pid.utf8).write(to: URL(fileURLWithPath: " + json.dumps(sys.argv[2]) + "))\nThread.sleep(forTimeInterval: 60)\ntry? Data(\"finished\".utf8).write(to: URL(fileURLWithPath: " + json.dumps(sys.argv[3]) + "))\n"; open(sys.argv[1],"w").write(code)' \
    "$work/smoke-app.swift" "$pid_file" "$finished"
xcrun swiftc -O -o "$work/RustZIPSmokeApp" "$work/smoke-app.swift"

make_app() {
    local app="$1" version="$2"
    mkdir -p "$app/Contents/MacOS" "$app/Contents/Resources"
    python3 -c 'import plistlib,sys; path,version=sys.argv[1:]; plistlib.dump({"CFBundleIdentifier":"com.yuhuotech.rustzip.updater-smoke","CFBundleName":"RustZIP Update Smoke","CFBundlePackageType":"APPL","CFBundleExecutable":"RustZIPSmokeApp","CFBundleShortVersionString":version,"CFBundleVersion":version},open(path,"wb"))' \
        "$app/Contents/Info.plist" "$version"
    cp "$work/RustZIPSmokeApp" "$app/Contents/MacOS/RustZIPSmokeApp"
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
    /bin/sleep 60 &
    running_pid=$!
    "$helper" --app "$target" --archive "$archive" --format "$format" --sha256 "$checksum" \
        --version "$new_version" --language en-US --pid "$running_pid" &
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
    --version 0.1.3 --language en-US --pid "$finished_pid"; then
    echo "Tampered update was accepted." >&2
    exit 1
fi
[[ "$(app_version)" == "0.1.2" ]]
codesign --verify --deep --strict "$target"

# Even a correctly checksummed archive must contain the version in the signed manifest.
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
    --sha256 "$checksum" --version 0.1.3 --language en-US --pid "$finished_pid"; then
    echo "Update with the wrong app version was accepted." >&2
    exit 1
fi
[[ "$(app_version)" == "0.1.2" ]]
codesign --verify --deep --strict "$target"
