#!/usr/bin/env bash
set -euo pipefail
repo_dir=$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)
cd "$repo_dir"
cargo build --locked --release
version=$(sed -n 's/^version = "\([^"]*\)"/\1/p' Cargo.toml | head -1)
architecture=$(dpkg --print-architecture)
stage=$(mktemp -d)
trap 'rm -rf "$stage"' EXIT
chmod 755 "$stage"
install -Dm755 target/release/monitor-control "$stage/usr/bin/monitor-control"
install -Dm644 data/io.github.monitorcontrol.MonitorControl.desktop "$stage/usr/share/applications/io.github.monitorcontrol.MonitorControl.desktop"
install -Dm644 data/autostart.desktop "$stage/etc/xdg/autostart/io.github.monitorcontrol.MonitorControl.desktop"
install -Dm644 data/60-monitor-control.rules "$stage/usr/lib/udev/rules.d/60-monitor-control.rules"
install -Dm644 data/monitor-control.conf "$stage/usr/lib/modules-load.d/monitor-control.conf"
install -Dm644 README.md "$stage/usr/share/doc/monitor-control/README.md"
mkdir -p "$stage/DEBIAN" dist
cat > "$stage/DEBIAN/control" <<CONTROL
Package: monitor-control
Version: $version
Section: utils
Priority: optional
Architecture: $architecture
Maintainer: Monitor Control contributors
Depends: libgtk-4-1 (>= 4.12), libadwaita-1-0 (>= 1.5), libddcutil5 (>= 2.2), libc6 (>= 2.39), libgcc-s1, udev, kmod, xdg-desktop-portal, xdg-desktop-portal-gnome
Description: Native external monitor brightness control
 Rust GTK application with a tray menu, global keyboard shortcuts,
 event-driven monitor discovery, and direct DDC/CI hardware control.
CONTROL
cat > "$stage/DEBIAN/postinst" <<'POSTINST'
#!/bin/sh
set -e
if [ "$1" = configure ]; then
    modprobe i2c-dev || true
    if [ -d /run/udev ]; then
        udevadm control --reload-rules
        udevadm trigger --subsystem-match=i2c-dev --action=add
    fi
fi
POSTINST
chmod 755 "$stage/DEBIAN/postinst"
dpkg-deb --root-owner-group --build "$stage" "dist/monitor-control_${version}_${architecture}.deb"
