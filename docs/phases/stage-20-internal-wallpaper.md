# Stage 20 — Internal Wallpaper Control

**What it is.** 0xin decodes and renders PNG/JPEG wallpapers into owned
wlroots scene buffers, with persistent configuration and a bundled live
control command.

**Gate:** *A configured image appears cover-scaled behind windows on every
output. `0xinctl wallpaper PATH` replaces it without restarting, and
`0xinctl wallpaper clear` restores the solid background.*

## Persistent configuration

```ini
background = 0.04 0.04 0.06
wallpaper = ~/Pictures/wallpaper.jpg
```

The image is independently cover-scaled to each output. This fills the output
without distortion and crops overflow around the center. PNG transparency
reveals the existing solid background beneath it. An unreadable or invalid
image logs an error and retains that fallback rather than preventing startup.

Paths beginning with `~/` expand against the compositor session's `HOME`.
PNG and JPEG support comes from a pure-Rust decoder linked into 0xin; no
wallpaper daemon or system image-decoding library is required.

## Runtime control

```sh
0xinctl wallpaper ~/Pictures/another.png
0xinctl wallpaper clear
```

The compositor creates a Unix socket under `XDG_RUNTIME_DIR`, scoped by its
`WAYLAND_DISPLAY`. `0xinctl` derives the same path from its environment, sends
a line request, and reports success or a decoding/rendering error. This makes
it suitable for Patin, keybindings, file pickers, and ordinary shell scripts.

Runtime changes are intentionally ephemeral: they update the current scene but
do not rewrite `0xin.conf`. The configured image is restored on the next
compositor start.

## Rendering architecture

Rust decodes and cover-resizes each image to RGBA8888. The C shim copies those
pixels into a custom `wlr_buffer` implementing data-pointer access, then adds a
`wlr_scene_buffer` above the output's solid fallback and below every
layer-shell or application surface. Scene-node destruction releases the final
wlroots buffer lock and frees the owned pixels.

Wallpaper changes decode every output before replacing existing nodes, so an
invalid path leaves the current wallpaper intact. Output hotplug creates a
newly scaled node from the active configured/runtime path; output destruction
removes its image buffer.

## Scope and limitations

- Scaling mode is currently `cover`.
- One image applies to all outputs, scaled independently.
- Decoding occurs synchronously on a control request. The Triangle filter keeps
  this bounded enough for the current mobile/debug target; a worker-thread
  handoff can be added if very large images still cause visible latency.
- The control socket currently exposes wallpaper operations only. It is the
  first implemented slice of the broader Stage 11 runtime-control design.
