# Threads and composer

The user opens a thread from the sidebar, reads it, and types a message in the composer at the bottom; characters appear as they are typed without visible lag.

## Sub-features

- `thread-open` clicking a sidebar entry shows that thread and its title in the header.
- `composer-type` typed text appears in the composer.
- `thread-print` `ctrl+p` in a thread opens the page's Print dialog; its Print… hands `{title, html}` to the app (`ampNativePrint`), which opens the GTK print dialog for that document.
- `composer-latency` keypress-to-screen stays near the measured baseline (about 42 ms median with the GL renderer; 135 ms meant the Vulkan renderer was back).

## How to get to it (user POV)

- Click a thread title in the left sidebar.
- `ctrl+k` / the search icon to find a thread by name.

## Driving it with verify-amp

Preconditions:

- Signed-in launch; the sidebar is visible in the latest screenshot.

- **Open thread.** Click a thread title in the sidebar (pick one of this project's; for typing, an idle one, not a running Lead). `verify-amp shot RUN thread`. The header shows the thread title; the log has `ampNavigationState https://ampcode.com/threads/T-…`.
- **Type.** Click inside the composer (the empty box at the bottom of the thread; it has no placeholder text), `agent-desktop type S "verification draft"`, `verify-amp shot RUN typed`. The text is visible in the composer.
- **Clear.** `agent-desktop key S ctrl+a` then `agent-desktop key S BackSpace`; screenshot shows an empty composer. Never press Enter.
- **Print.** In a thread, `ctrl+p`, screenshot the page's dialog, click Print…; the log has `ampNativePrint` and a `Print` window opens. Choose Print to File and Print: the PDF lands as `output.pdf` in the app's working directory (the checkout's root); move it into the evidence folder and inspect page 1 (`pdftoppm -png -f 1 -l 1`). Cancel closes it with no toast.
- **Latency (only when asked).** The measured method and its scripts are described in commit a555e86's message and thread T-01a1ac79-024d-715f-b3e8-c1d275efb268; reuse them rather than eyeballing.

## Gotchas

- Enter sends the message to the owner's real thread. Never press it in the composer.
- A drafted message can persist per thread; always clear it.
- Opening a thread marks it read for the owner; prefer this project's own threads.
- `doctor`'s renderer line must say `gl` for latency results to mean anything.
