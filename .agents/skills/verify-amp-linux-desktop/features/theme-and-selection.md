# Theme and text selection

With Amp's appearance set to System, the page takes its colours from the current Omarchy theme, and selected text is clearly highlighted with readable text in the transcript, composer and sidebar search.

## Sub-features

- `theme-system` page background, sidebar and accent follow `~/.local/state/omarchy/current/theme/colors.toml` (the app reads `$XDG_STATE_HOME/omarchy/current`; verify runs do not override `XDG_STATE_HOME`, so they use the owner's current theme).
- `select-transcript` a drag selection over a message is visible.
- `select-composer` selected composer text is visible.
- `select-search` selected text in the sidebar search field is visible.

## How to get to it (user POV)

- Amp settings → Appearance → System; change the Omarchy theme.
- Drag over text, or `ctrl+a` in a field.

## Driving it with verify-amp

Preconditions:

- Signed-in launch; the copied settings have `"appearance": "system"` (check `$XDG_RUNTIME_DIR/verify-amp/RUN/config/amp-native/settings.json`).

- **Theme.** `verify-amp shot RUN theme`; compare the background and accent with the hex values in the theme's `colors.toml`.
- **Transcript selection.** Open a thread, `agent-desktop drag S X1 Y X2 Y` across a line of a message, `verify-amp shot RUN select-transcript`. The highlight is clearly distinct from the background and the selected text is readable.
- **Composer selection.** Type a word into the composer, `ctrl+a`, screenshot, then clear it.
- **Other themes.** The app follows the owner's current theme only; other themes are checked without the window: call `amp_common::theme::parse_colors(text, light)` (from a scratch test in `crates/common`) on each `/usr/share/omarchy/themes/*/colors.toml` (`light` is whether `light.mode` exists beside it) and compute WCAG contrast of the text colour on the selection colour and of the selection colour on the background. Readable means text-on-selection ≥ 4.5.

## Gotchas

- Never run `omarchy-theme-set` or edit `~/.config/omarchy`: it changes the owner's whole desktop.
- WebKit darkens opaque selection colours; the app emits `#RRGGBBfe` on purpose, so a screenshot reads one step off (e.g. `#4c8466` for `#4c8566`).
- Never edit `~/.local/state/omarchy`.
