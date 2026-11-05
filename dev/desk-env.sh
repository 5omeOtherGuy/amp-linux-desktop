# Orb only: exports the orb Desktop's display and session bus (labwc) to this shell.
P=$(pgrep -x labwc | head -1)
eval "$(sudo cat /proc/$P/environ | tr '\0' '\n' | grep -E '^(XDG_RUNTIME_DIR|WAYLAND_DISPLAY|DISPLAY|DBUS_SESSION_BUS_ADDRESS)=' | sed 's/^/export /')"
export WAYLAND_DISPLAY=wayland-0
