RustZIP vendors gpui-pre-windows 0.3.9 under Apache-2.0.

The local patch adds an OLE shell file drag source matching GPUI's existing
native ExternalDragPayload API. It exposes local temporary files through the
Windows Shell data object, so Explorer and other OLE drop targets can receive
them using the normal `CF_HDROP` file contract. Drag completion is posted to
the source window so it runs after GPUI's initiating input callback returns.
