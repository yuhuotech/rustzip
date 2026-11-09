#!/usr/bin/env sh
# Official installer; the helper records the actual prefix and XDG data path.
set -eu
here="$(cd "$(dirname "$0")" && pwd)"
prefix="${PREFIX:-$HOME/.local}"
data_home="${XDG_DATA_HOME:-$prefix/share}"
case "${1:-}" in
    --uninstall) exec "$here/rz-updater" --install "$here" "$prefix" "$data_home" --uninstall ;;
    "") exec "$here/rz-updater" --install "$here" "$prefix" "$data_home" ;;
    *) echo "Usage: install.sh [--uninstall]" >&2; exit 2 ;;
esac
