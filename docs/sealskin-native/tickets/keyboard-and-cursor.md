# Improve Mac keyboard and cursor behavior

Add optional Command-as-Control and a way to send remote-only chords while retaining a local escape action. Verify keyboard layouts and release of held keys and modifiers on focus loss. Add remote cursor shape and visibility handling using the server’s cursor updates.

The **Input** section in Session Controls exposes Command-as-Control, Remote Cursor, Smooth Trackpad Scrolling, and Use Mac Input Method. Smooth Trackpad Scrolling starts enabled, and the other switches are opt-in. Choices are saved per server. Send Key Combination offers remote Alt+Tab and Ctrl+Alt+Delete.

## Trackpad gestures

Two-finger panning and pinch zoom work with today’s Selkies server. Panning uses precise deltas and an end-of-scroll event on servers that advertise finger scrolling, or wheel steps elsewhere. Pinch uses AppKit magnification events and sends a Control-wheel sequence over the remote picture, as the web client does. Verify horizontal and vertical panning, slow and fast movement, momentum, zoom in both directions, and ordinary mouse wheels. [Selkies zoom and scroll](https://github.com/selkies-project/selkies/blob/715df7cdea9c6c21ae339f93f194f718a879dc41/web/selkies-web-core/lib/input.ts#L4932)

Forwarding individual contacts to Linux is an off-by-default option tracked separately. [Multitouch trackpad forwarding](multitouch-trackpad.md)

## IME and composed text

Before this work, the native session only forwarded key events as X11 keysyms. It could use a single printable character supplied by macOS.

The Selkies web client lets the browser’s local IME compose into the focused session input. It mirrors provisional text in the remote field by comparing successive Unicode strings, backspacing the changed suffix, and typing the replacement. It uses momentary keysyms for short runs and batches eligible multi-character Unicode runs as `co,end,<text>`. It also deduplicates browser commit events and defers shortcuts that arrive during composition until the text settles. The server injects committed text through its Wayland or X11 keyboard path. [Web composition and commit handling](https://github.com/selkies-project/selkies/blob/715df7cdea9c6c21ae339f93f194f718a879dc41/web/selkies-web-core/lib/input.ts#L2837-L3072) · [Server commit handling](https://github.com/selkies-project/selkies/blob/715df7cdea9c6c21ae339f93f194f718a879dc41/crates/selkies/src/input/dispatch.rs#L513-L525)

The native client uses GPUI’s macOS text input handler to keep marked text and candidate selection local, then sends the final text once through `co,end,<text>` on supported servers or as a Unicode keysym for a single character. The provisional text and candidate picker anchor to the last click in the remote picture, the pointer over the picture, or a position inside the picture for keyboard-only input. Selkies does not supply the remote text caret’s position. Keeping provisional text local avoids having to erase it from the remote field if composition is canceled or focus changes. Physical shortcuts use the key-event path. Manual verification still covers dead keys, multiple IMEs, and focus loss during composition. [GPUI input example](https://docs.rs/crate/gpui/0.2.2/source/examples/input.rs)

[Input settings](https://github.com/selkies-project/selkies/blob/715df7cdea9c6c21ae339f93f194f718a879dc41/docs/settings.md#input) · [Web input implementation](https://github.com/selkies-project/selkies/blob/715df7cdea9c6c21ae339f93f194f718a879dc41/web/selkies-web-core/lib/input.ts)

[Selkies web feature research](research.md)
