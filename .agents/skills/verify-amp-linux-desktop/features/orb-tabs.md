# Orb Desktop and Terminal tabs

An orb thread shows `Desktop`, `Terminal` (and `Mac` with a Mac satellite) tabs on the right; the Desktop tab streams the orb's screen and the Terminal tab shows its shell. Distribution WebKitGTK has no WebRTC, so the app makes the page fall back to its WebSocket transport and decodes H.264 with gst-libav (commit e9e7dbe).

## Sub-features

- `tab-desktop` the Desktop tab shows the orb's desktop (dock with browser and editor icons at the bottom) instead of "Still working".
- `tab-terminal` the Terminal tab shows the welcome text and a `~/workspace/repo$` prompt.
- `tab-mac` the Mac tab shows a running Mac satellite's screen.

## How to get to it (user POV)

- Open an orb thread and choose `Desktop`, `Terminal` or `Mac`. In a wide window they are tabs at the top of the right pane; at the default 1600×1000 window (and at scale 1.5) there is no right pane and they are in the header's `Views` menu (`ctrl+shift+x` Desktop, `ctrl+shift+t` Terminal).
- Hovering a thread's sidebar icon shows its tooltip, `Orb is running` or `Orb is idle`; that is how to find an awake orb thread.

## Driving it with verify-amp

Preconditions:

- Signed-in launch; `pacman -Q gst-libav` succeeds.
- An orb thread of this project whose orb is already awake, or the task allows waking one (the pane then shows `Orb Is Asleep` with a `Wake Orb` button).

- **Open.** Click the orb thread in the sidebar, then `Desktop`. It shows `Starting desktop` first: opening it starts the orb's desktop. `verify-amp shot RUN desktop-tab` after 15 s. The pane shows the remote desktop, not `Still working` or `Desktop Unavailable`. Take a second screenshot later: a changed frame proves the stream is live.
- **Terminal.** Open `Terminal`, wait 10 s, `verify-amp shot RUN terminal-tab`. The prompt is visible.
- **Close.** Close each pane with its own close button. Check the target in the screenshot first; see the banner gotcha.
- **No WebRTC regression.** The app log records nothing about the transport, so the screenshots are the proof. If the tab hangs at `Still working`, the page's transport code changed and the `NO_WEBRTC` script in `amp_native/scripts.py` no longer matches.

## Gotchas

- Waking an orb and starting a Mac cost money; only with permission. Do not open the Mac view unless the task allows starting a satellite.
- Orb threads can show an "Orb Setup Has Been Slow" banner with an `Investigate in New Orb` button at the top right of the thread, near the pane controls. Clicking it starts a new thread and orb in the owner's account. Never click it.
- Do not type or click inside the remote desktop or terminal unless the task asks: it is a real orb with another thread's work.
- `Desktop Unavailable · Receiving video` means the stream works but decoding fails: gst-libav missing or the NVIDIA decoders not demoted (`GST_PLUGIN_FEATURE_RANK`, set in `amp_native/__main__.py`).
- The Mac tab uses the same viewer but is untested; its codec may differ.
- While the Desktop tab is visible WebKit's web process uses about 150% CPU; that is expected.
