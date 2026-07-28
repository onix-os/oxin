# Stage 1 — Wayland Server Up

**What it is.** Turning the wlroots backend from Stage 0 into an actual
Wayland *server* — the thing client applications connect to. This is where
`wl_display`, the event loop, and the first client-facing globals
(`wl_compositor`, `wl_shm`) show up.

**Why it matters.** A compositor's whole job, from a client's point of view,
is being the other end of the Wayland protocol. Until a client can connect
and see globals, nothing else — windows, input, tiling — has anywhere to
attach.

**Deliverable** (from `KICKOFF.md`): *`wl_display`, event loop,
`wl_compositor`, `wl_shm`; advertise the socket (`WAYLAND_DISPLAY`); accept a
client connection. `wayland-info` connects and lists globals.*

## How it went

`wl_display_create` plus `wl_display_get_event_loop` gave the event loop
`wlr_backend_autocreate` needed. `wlr_compositor_create` supplies
`wl_compositor` (surfaces, regions) but *not* `wl_shm` — that comes from
`wlr_renderer_init_wl_display`, called on the renderer created in Stage 0.
That distinction — which call actually advertises which global — is easy to
get backwards and only becomes obvious by checking with a real client.

`wl_display_add_socket_auto` opens the Unix socket (e.g. `wayland-2`) and
`main()` exports it as `WAYLAND_DISPLAY` before spawning any client, so
spawned test programs talk to 0xin and not to whatever nested host they're
running under.

**Verified with:** `cargo nested -- wayland-info` — the client connects and
lists `wl_shm`, `zwp_linux_dmabuf_v1`, `wl_compositor`, `wl_subcompositor`,
`wl_data_device_manager`, interleaved with wlroots' own debug log. See
[Running & Verifying](../running.md) for why a real client's own output,
rather than a screenshot, was the right verification tool at this stage —
there was nothing to see yet.

**Status: done.**
