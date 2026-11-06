RustZIP vendors gpui-pre-linux 0.3.9 under Apache-2.0.

The local patch adds an XDND v2-v5 file source for the X11 backend, matching
GPUI's existing native ExternalDragPayload API. It negotiates target acceptance,
serves `text/uri-list`, replies to unsupported selection requests, handles
Escape cancellation, ignores stale target responses, waits for target
acceptance when pointer release races the status response, respects the target's
accepted rectangle and position requests, and bounds a drag session. Completion
is queued after the initiating GPUI input callback returns. Wayland behavior
remains provided by the upstream Wayland backend.
