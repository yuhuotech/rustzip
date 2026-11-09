#!/usr/bin/env bash
set -euo pipefail

root="$(cd "$(dirname "$0")/../.." && pwd)"
work="$(mktemp -d "${TMPDIR:-/tmp}/rustzip-update-helper.XXXXXX")"
helper="$work/RustZIPUpdateHelper"
target="$work/installed/RustZIP.app"
stage="$work/dmg-stage/RustZIP.app"
marker="$work/restarted"
pid_file="$work/smoke.pid"
finished="$work/smoke.finished"
download="$work/download"
trap 'rm -rf "$work"' EXIT

xcrun swiftc -O -o "$helper" "$root/packaging/macos/update-helper.swift"
mkdir -p "$target/Contents/MacOS" "$stage/Contents/MacOS" "$work/installed" "$work/dmg-stage" "$download"

python3 -c 'import plistlib,sys; path,version=sys.argv[1:]; plistlib.dump({"CFBundleIdentifier":"com.yuhuotech.rustzip","CFBundleName":"RustZIP","CFBundlePackageType":"APPL","CFBundleExecutable":"RustZIPSmokeApp","CFBundleShortVersionString":version,"CFBundleVersion":version},open(path,"wb"))' \
    "$target/Contents/Info.plist" 0.1.0
python3 -c 'import plistlib,sys; path,version=sys.argv[1:]; plistlib.dump({"CFBundleIdentifier":"com.yuhuotech.rustzip","CFBundleName":"RustZIP","CFBundlePackageType":"APPL","CFBundleExecutable":"RustZIPSmokeApp","CFBundleShortVersionString":version,"CFBundleVersion":version},open(path,"wb"))' \
    "$stage/Contents/Info.plist" 0.1.1
python3 -c 'import json,sys; code="import Foundation\nlet pid = String(ProcessInfo.processInfo.processIdentifier)\ntry? Data(pid.utf8).write(to: URL(fileURLWithPath: " + json.dumps(sys.argv[2]) + "))\nThread.sleep(forTimeInterval: 30)\ntry? Data(\"finished\".utf8).write(to: URL(fileURLWithPath: " + json.dumps(sys.argv[3]) + "))\n"; open(sys.argv[1],"w").write(code)' \
    "$work/smoke-app.swift" "$pid_file" "$finished"
xcrun swiftc -O -o "$work/RustZIPSmokeApp" "$work/smoke-app.swift"
cp "$work/RustZIPSmokeApp" "$target/Contents/MacOS/RustZIPSmokeApp"
cp "$work/RustZIPSmokeApp" "$stage/Contents/MacOS/RustZIPSmokeApp"
for app in "$target" "$stage"; do
    codesign --force --sign - "$app/Contents/MacOS/RustZIPSmokeApp"
    codesign --force --sign - "$app"
    codesign --verify --deep --strict "$app"
done

hdiutil create -quiet -volname RustZIP -srcfolder "$work/dmg-stage" -ov -format UDZO "$download/update.dmg"
/bin/sleep 30 &
running_pid=$!
"$helper" --app "$target" --dmg "$download/update.dmg" --version 0.1.1 --language en-US --pid "$running_pid" &
helper_pid=$!
sleep 1
kill "$running_pid"
wait "$running_pid" 2>/dev/null || true
helper_exited=0
for attempt in {1..40}; do
    if ! kill -0 "$helper_pid" 2>/dev/null; then
        helper_exited=1
        break
    fi
    sleep 0.25
done
if [[ "$helper_exited" != 1 ]]; then
    if [[ -f "$pid_file" ]]; then
        read -r smoke_pid < "$pid_file"
        kill "$smoke_pid" 2>/dev/null || true
    fi
    wait "$helper_pid" 2>/dev/null || true
    echo "Update helper waited for the relaunched app to close." >&2
    exit 1
fi
wait "$helper_pid"
[[ ! -f "$finished" ]]
if [[ -f "$pid_file" ]]; then
    read -r smoke_pid < "$pid_file"
    kill "$smoke_pid" 2>/dev/null || true
fi

version="$(/usr/libexec/PlistBuddy -c 'Print :CFBundleShortVersionString' "$target/Contents/Info.plist")"
[[ "$version" == "0.1.1" ]]
for attempt in {1..40}; do
    [[ -f "$marker" ]] && break
    sleep 0.25
done
[[ -f "$marker" ]]
codesign --verify --deep --strict "$target"
[[ -z "$(find "$work/installed" -maxdepth 1 -name '.RustZIP-backup-*.app' -print -quit)" ]]
