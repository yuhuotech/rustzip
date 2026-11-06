#!/usr/bin/env bash
set -euo pipefail

app="${1:?usage: verify-portable-libraries.sh RustZIP.app}"
if [[ ! -d "$app/Contents" ]]; then
    echo "RustZIP app bundle is missing: $app" >&2
    exit 1
fi
app="$(cd "$app" && pwd -P)"
frameworks="$app/Contents/Frameworks"
resources="$app/Contents/Resources"

for required in libb2.1.dylib liblz4.1.dylib; do
    if [[ ! -f "$frameworks/$required" ]]; then
        echo "RustZIP app is missing bundled library: $required" >&2
        exit 1
    fi
done
for license in libb2-license.txt lz4-license.txt; do
    if [[ ! -s "$resources/$license" ]]; then
        echo "RustZIP app is missing bundled library license: $license" >&2
        exit 1
    fi
done

team_id="$(codesign -dv --verbose=4 "$app" 2>&1 | sed -n 's/^TeamIdentifier=//p' | head -n 1)"
if [[ -z "$team_id" ]]; then
    echo "RustZIP app has no code signature" >&2
    exit 1
fi

while IFS= read -r -d '' binary; do
    if [[ "$(file -b "$binary")" != *Mach-O* ]]; then
        continue
    fi
    codesign --verify --strict "$binary"
    if otool -l "$binary" | awk '
        $1 == "cmd" && $2 == "LC_RPATH" { in_rpath = 1; next }
        in_rpath && $1 == "path" {
            if ($2 ~ /^\/opt\/homebrew\// || $2 ~ /^\/usr\/local\//) bad = 1
            in_rpath = 0
        }
        END { exit bad ? 0 : 1 }
    '; then
        echo "Non-portable Homebrew run path in $binary" >&2
        exit 1
    fi
    while IFS= read -r dependency; do
        case "$dependency" in
            /System/Library/*|/usr/lib/*) ;;
            @rpath/*)
                bundled="$frameworks/${dependency#@rpath/}"
                if [[ ! -f "$bundled" ]]; then
                    echo "Bundled dependency is missing: $dependency (from $binary)" >&2
                    exit 1
                fi
                ;;
            @loader_path/*)
                bundled="$(dirname "$binary")/${dependency#@loader_path/}"
                if [[ ! -e "$bundled" ]]; then
                    echo "Loader-relative dependency is missing: $dependency (from $binary)" >&2
                    exit 1
                fi
                ;;
            @executable_path/*)
                bundled="$(dirname "$binary")/${dependency#@executable_path/}"
                if [[ ! -e "$bundled" ]]; then
                    echo "Executable-relative dependency is missing: $dependency (from $binary)" >&2
                    exit 1
                fi
                ;;
            *)
                echo "Non-portable absolute library dependency: $dependency (from $binary)" >&2
                exit 1
                ;;
        esac
    done < <(otool -L "$binary" | awk 'NR > 1 { print $1 }')
done < <(find "$app/Contents" -type f -print0)

for dylib in "$frameworks"/*.dylib; do
    [[ -f "$dylib" ]] || continue
    dylib_team="$(codesign -dv --verbose=4 "$dylib" 2>&1 | sed -n 's/^TeamIdentifier=//p' | head -n 1)"
    if [[ "$dylib_team" != "$team_id" ]]; then
        echo "Bundled library signature team differs from the app: $dylib" >&2
        exit 1
    fi
done

codesign --verify --deep --strict "$app"
echo "RustZIP macOS dependencies are bundled and signed with the app."
