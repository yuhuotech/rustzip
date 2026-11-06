#!/usr/bin/env bash
set -euo pipefail

app="${1:?usage: bundle-homebrew-libraries.sh RustZIP.app [signing identity]}"
identity="${2:--}"
if [[ ! -d "$app/Contents" ]]; then
    echo "RustZIP app bundle is missing: $app" >&2
    exit 1
fi

app="$(cd "$app" && pwd -P)"
frameworks="$app/Contents/Frameworks"
resources="$app/Contents/Resources"
mkdir -p "$frameworks" "$resources"

declare -a queue=()
while IFS= read -r -d '' binary; do
    if [[ "$(file -b "$binary")" == *Mach-O* ]]; then
        queue+=("$binary")
    fi
done < <(find "$app/Contents" -type f -print0)

declare -a visited=("$app/__visited_sentinel__")
has_rpath() {
    otool -l "$1" | awk -v wanted="$2" '
        $1 == "cmd" && $2 == "LC_RPATH" { in_rpath = 1; next }
        in_rpath && $1 == "path" {
            if ($2 == wanted) found = 1
            in_rpath = 0
        }
        END { exit found ? 0 : 1 }
    '
}

for ((index = 0; index < ${#queue[@]}; index++)); do
    binary="${queue[$index]}"
    seen=0
    for previous in "${visited[@]}"; do
        if [[ "$previous" == "$binary" ]]; then
            seen=1
            break
        fi
    done
    if [[ "$seen" == "1" ]]; then
        continue
    fi
    visited+=("$binary")

    while IFS= read -r dependency; do
        case "$dependency" in
            /opt/homebrew/*|/usr/local/*) ;;
            *) continue ;;
        esac
        if [[ ! -f "$dependency" ]]; then
            echo "Missing Homebrew dynamic library: $dependency" >&2
            exit 1
        fi

        name="${dependency##*/}"
        case "$name" in
            libb2*.dylib)
                formula_prefix="$(brew --prefix libb2)"
                license_source="$formula_prefix/COPYING"
                license_name="libb2-license.txt"
                ;;
            liblz4*.dylib)
                formula_prefix="$(brew --prefix lz4)"
                license_source="$formula_prefix/LICENSE"
                license_name="lz4-license.txt"
                ;;
            *)
                echo "Unreviewed Homebrew dynamic dependency: $dependency" >&2
                exit 1
                ;;
        esac
        if [[ ! -f "$license_source" ]]; then
            echo "Missing license for bundled dependency: $license_source" >&2
            exit 1
        fi
        if [[ ! -f "$resources/$license_name" ]]; then
            cp "$license_source" "$resources/$license_name"
        fi

        destination="$frameworks/$name"
        if [[ -e "$destination" ]]; then
            if ! cmp -s "$dependency" "$destination"; then
                echo "Two Homebrew libraries have the same install name: $name" >&2
                exit 1
            fi
        else
            cp -pL "$dependency" "$destination"
        fi

        chmod 755 "$destination"
        install_name_tool -id "@rpath/$name" "$destination"
        install_name_tool -change "$dependency" "@rpath/$name" "$binary"
        case "$binary" in
            "$frameworks"/*) runpath="@loader_path" ;;
            *) runpath="@executable_path/../Frameworks" ;;
        esac
        if ! has_rpath "$binary" "$runpath"; then
            install_name_tool -add_rpath "$runpath" "$binary"
        fi
        queue+=("$destination")
    done < <(otool -L "$binary" | awk 'NR > 1 { print $1 }')
done

for dylib in "$frameworks"/*.dylib; do
    [[ -f "$dylib" ]] || continue
    if [[ "$identity" == "-" ]]; then
        codesign --force --sign - "$dylib"
    else
        codesign --force --options runtime --timestamp --sign "$identity" "$dylib"
    fi
done
