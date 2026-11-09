#!/usr/bin/env bash
set -euo pipefail
root="$(cd "$(dirname "$0")/../.." && pwd)"
if [[ "$(id -u)" == 0 ]]; then echo "Run updater integration tests as an ordinary user." >&2; exit 2; fi
cargo build --manifest-path "$root/Cargo.toml" -p rz-updater --features test-fixture
bin="${CARGO_TARGET_DIR:-$root/target}/debug"
work="$(mktemp -d)"
trap 'rm -rf "$work"' EXIT
source="$work/source"
mkdir -p "$source/icons"
cp "$bin/rz-update-fixture" "$source/rustzip"
cp "$bin/rz-updater" "$source/rz-updater"
cp "$bin/rz-update-fixture" "$source/rz-pdf-renderer"
cp "$bin/rz-update-fixture" "$source/rz-video-renderer"
cp "$root"/packaging/linux/*.desktop "$root"/packaging/linux/*.nemo_action "$root"/packaging/linux/rustzip-nautilus-* "$root"/packaging/linux/install.sh "$source/"
cp "$root"/packaging/icons/hicolor/*.png "$source/icons/"
cp "$root/packaging/licenses/format-readers.txt" "$source/format-readers-licenses.txt"
version="$(awk -F'"' '/^version *=/ {print $2; exit}' "$root/Cargo.toml")"
printf '{"schema":1,"version":"%s","platform":"linux-%s"}\n' "$version" "$(uname -m)" > "$source/update-layout.json"
for mode in success fail interrupted; do
    prefix="$work/$mode prefix 中文
with newline"
    xdg="$work/$mode data 中文
with newline"
    download="$work/RustZIP-update-$mode"
    mkdir -p "$download"
    PREFIX="$prefix" XDG_DATA_HOME="$xdg" sh "$source/install.sh"
    printf 'user settings' > "$work/settings-$mode"
    result="$work/result-$mode"
    fault=""
    [[ "$mode" != interrupted ]] || fault=published
    RZ_UPDATE_FIXTURE_FAULT="$fault" RZ_UPDATE_FIXTURE_RESULT="$result" RZ_UPDATE_FIXTURE_MODE="$mode" \
        "$prefix/bin/rustzip" --fixture-update "$source" "$download/update.tar.gz"
    if [[ "$mode" == interrupted ]]; then
        for attempt in {1..100}; do
            if [[ -f "$prefix/lib/rustzip/transaction.json" ]] && grep -q Published "$prefix/lib/rustzip/transaction.json"; then break; fi
            sleep 0.1
        done
        sleep 0.2
        [[ ! -e "$result" ]]
        RZ_UPDATE_FIXTURE_RESULT="$result" "$prefix/bin/rustzip"
    fi
    for attempt in {1..600}; do [[ ! -f "$result" ]] || break; sleep 0.1; done
    [[ -f "$result" ]]
    if [[ "$mode" == success ]]; then
        [[ "$(< "$result")" == updated ]]
        grep -q '0.2.0' "$prefix/lib/rustzip/current/update-layout.json"
    else
        [[ "$(< "$result")" == rolled-back ]]
        grep -q "$version" "$prefix/lib/rustzip/current/update-layout.json"
    fi
    [[ "$(< "$work/settings-$mode")" == 'user settings' ]]
    for attempt in {1..100}; do [[ -e "$prefix/lib/rustzip/transaction.json" ]] || break; sleep 0.1; done
    [[ ! -e "$prefix/lib/rustzip/transaction.json" ]]
    test -L "$xdg/nautilus/scripts/RustZIP - Compress selected items"
    PREFIX="$prefix" XDG_DATA_HOME="$work/changed XDG" sh "$source/install.sh" --uninstall
    [[ ! -e "$prefix/bin/rustzip" ]]
    [[ ! -e "$xdg/nautilus/scripts/RustZIP - Compress selected items" ]]
done
mkdir "$work/unmanaged" "$work/RustZIP-update-unmanaged"
cp "$bin/rz-update-fixture" "$work/unmanaged/rustzip"
if "$work/unmanaged/rustzip" --fixture-update "$source" "$work/RustZIP-update-unmanaged/update.tar.gz"; then
    echo "An unmanaged installation was updated." >&2; exit 1
fi
echo "Linux update, restart, rollback and installation-source checks passed."
