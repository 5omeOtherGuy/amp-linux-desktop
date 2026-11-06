#!/bin/sh
# A launch from the desktop has its stderr on /dev/null, which would lose Python callback errors and
# GTK and WebKit warnings. Keep it in a file instead, rotated at 1 MB.
if [ ! -t 2 ]; then
	log=${XDG_CACHE_HOME:-$HOME/.cache}/amp-native/stderr.log
	mkdir -p "${log%/*}"
	[ "$(stat -c %s "$log" 2>/dev/null || echo 0)" -gt 1000000 ] && mv -f "$log" "$log.old"
	exec 2>>"$log"
fi
exec /usr/bin/python3 -m amp_native "$@"
