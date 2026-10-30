#!/usr/bin/env bash
set -euo pipefail

app="${1:?usage: verify-quicklook-registration.sh RustZIP.app}"
bundle_id="com.yuhuotech.rustzip.quicklook"
extension_point="com.apple.quicklook.preview"
lsregister="/System/Library/Frameworks/CoreServices.framework/Frameworks/LaunchServices.framework/Support/lsregister"

if [ ! -d "$app" ]; then
    echo "RustZIP app is missing" >&2
    exit 1
fi
app="$(cd "$app" && pwd -P)"
appex="$app/Contents/PlugIns/RustZIPQuickLook.appex"
if [ ! -d "$appex" ]; then
    echo "RustZIP Quick Look extension is missing" >&2
    exit 1
fi

cleanup() {
    pluginkit -e default -i "$bundle_id" >/dev/null 2>&1 || true
    pluginkit -r "$appex" >/dev/null 2>&1 || true
    "$lsregister" -u "$app" >/dev/null 2>&1 || true
    qlmanage -r >/dev/null 2>&1 || true
}
trap cleanup EXIT

codesign --verify --deep --strict "$app"
"$lsregister" -f "$app"
pluginkit -a "$appex"
pluginkit -e use -p "$extension_point" -i "$bundle_id"

matches="$(pluginkit -m -v -p "$extension_point" -i "$bundle_id")"
if [[ "$matches" != +* || "$matches" != *"$appex"* ]]; then
    echo "Quick Look provider was not registered as enabled for this app" >&2
    printf '%s\n' "$matches" >&2
    exit 1
fi

printf '%s\n' "$matches"
