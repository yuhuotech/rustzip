#!/usr/bin/env bash
set -euo pipefail

root="$(cd "$(dirname "$0")/../.." && pwd)"
test_dir="$(mktemp -d)"
trap 'rm -rf "$test_dir"' EXIT
arch="$(uname -m)"
if [ "$arch" = "arm64" ]; then
    swift_arch="arm64"
elif [ "$arch" = "x86_64" ]; then
    swift_arch="x86_64"
else
    echo "Unsupported macOS test architecture: $arch" >&2
    exit 2
fi
sdk="$(xcrun --sdk macosx --show-sdk-path)"
python3 "$root/packaging/macos/tests/make-quicklook-fixtures.py" \
    "$test_dir/normal.zip" "$test_dir/zip64.zip" "$test_dir/malformed.zip" \
    "$test_dir/oversized.zip" "$test_dir/multidisk.zip"
xcrun swiftc -parse-as-library -O -module-name RustZIPQuickLookSmoke \
    -target "${swift_arch}-apple-macosx12.0" -sdk "$sdk" \
    -framework AppKit -framework QuickLook -framework QuickLookUI -framework WebKit \
    -o "$test_dir/quicklook-smoke" \
    "$root/packaging/macos/quicklook-preview.swift" \
    "$root/packaging/macos/tests/quicklook-smoke.swift"
"$test_dir/quicklook-smoke" "$test_dir/normal.zip" "$test_dir/zip64.zip" \
    "$test_dir/malformed.zip" "$test_dir/oversized.zip" "$test_dir/multidisk.zip"
