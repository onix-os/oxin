# Stage 21 — Application Window Opacity

**What it is.** 0xin can apply a configurable opacity to ordinary XDG
application windows, allowing its built-in wallpaper or solid background to
show through.

**Gate:** *With `window_opacity = 0.8`, application windows render
translucently while compositor backgrounds and layer-shell UI remain fully
opaque. With no setting, behavior is unchanged.*

## Configuration

```ini
window_opacity = 0.8
```

The accepted range is `0.0` through `1.0`: `0.0` is fully transparent and
`1.0` is fully opaque. The default is `1.0`, so existing desktop and device
profiles do not become translucent unless they opt in. Invalid values are
warned about and ignored.

The FP5 reference profile uses `0.8`. The option itself is device-independent
and works for the same XDG windows on desktop and mobile backends.

## Rendering scope

The compositor walks the buffers below each XDG toplevel scene tree and sets
their wlroots scene-buffer opacity. It reapplies the value as surfaces commit
and map so buffers that are attached after scene-tree creation receive the
same opacity.

This first implementation intentionally excludes layer-shell surfaces. Panels,
Patin, virtual keyboards, and similar shell UI therefore remain fully opaque.
The compositor-owned solid background and wallpaper are also unaffected.

Opacity is currently a single global startup setting. Per-application rules
and live changes can build on this mechanism later.
