#!/bin/sh
# Orb only: runs the app on the orb Desktop with debug logging; sign-in opens in the Desktop's Chrome.
# Needs a Rust toolchain on the orb; dev/orb-handler runs the binary this builds.
dev=$(cd "$(dirname "$0")" && pwd)
. "$dev/desk-env.sh"
export BROWSER="$dev/orb-browser" AMP_NATIVE_DEBUG=1 GTK_USE_PORTAL=0
export GDK_BACKEND=${GDK_BACKEND:-wayland}
cd "$dev/.." && cargo build -q && exec target/debug/amp-linux-desktop "$@" > /tmp/amp-native.out 2>&1
