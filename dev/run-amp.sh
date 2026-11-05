#!/bin/sh
# Orb only: runs the app on the orb Desktop with debug logging; sign-in opens in the Desktop's Chrome.
dev=$(cd "$(dirname "$0")" && pwd)
. "$dev/desk-env.sh"
export BROWSER="$dev/orb-browser" AMP_NATIVE_DEBUG=1 GTK_USE_PORTAL=0
export GDK_BACKEND=${GDK_BACKEND:-wayland}
cd "$dev/.." && exec /usr/bin/python3 -m amp_native "$@" > /tmp/amp-native.out 2>&1
