#!/usr/bin/env bash
set -euo pipefail
repo_dir=$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)
version=$(sed -n 's/^version = "\([^"]*\)"/\1/p' "$repo_dir/Cargo.toml" | head -1)
package=${1:-"$repo_dir/dist/monitor-control_${version}_$(dpkg --print-architecture).deb"}
if [[ ! -f "$package" ]]; then
  echo "Package not found: $package" >&2
  echo 'Build it with scripts/build-deb.sh, or download a built .deb package.' >&2
  exit 1
fi
package=$(realpath "$package")
if [[ $EUID -eq 0 ]]; then
  exec apt-get install -y "$package"
elif command -v pkexec >/dev/null 2>&1; then
  exec pkexec /usr/bin/apt-get install -y "$package"
else
  exec sudo apt-get install "$package"
fi
