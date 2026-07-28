# 0xin

**0xin** is a from-scratch tiling Wayland compositor, written in Rust on top
of [wlroots](https://gitlab.freedesktop.org/wlroots/wlroots). It's the
userspace sibling of [`snert`](https://github.com/sn3rt), a from-scratch
kernel project — same working style, one layer up the stack.

Most people who want a tiling Wayland compositor install one. 0xin exists
for a different reason: to understand, concept by concept, what a Wayland
compositor actually *is* — the backend, the Wayland server, the renderer,
`xdg-shell`, input routing, tiling, real display output — by building each
one instead of importing it. It's a learning project first and a daily
driver second, though it's grown into something usable on real hardware.
Design decisions are collected in [Design & ideas](design.md).

## The mental model

A Wayland compositor sits between two things it doesn't own:

```
        clients (foot, firefox, ...)
                    │
                    ▼
   ┌────────────────────────────────┐
   │             0xin               │   ← this project
   │  policy: tiling, workspaces,    │
   │  keybindings, config            │
   └────────────────────────────────┘
                    │
                    ▼
              wlroots (C library)
        DRM/KMS, GLES2 renderer, libinput,
        scene graph, protocol plumbing
                    │
                    ▼
             the Linux kernel
```

**wlroots is the engine; 0xin is the driver.** wlroots does the parts that
are the same for every compositor — talking to the kernel's display and
input subsystems, decoding the Wayland wire protocol, drawing GL buffers.
0xin decides *policy*: which window goes where, what a keypress does, how
workspaces and monitors relate. That split is deliberate and shows up again,
one level down, inside 0xin itself — see [Architecture](architecture.md).

## Why phases

Rather than a progress percentage or a loose TODO list, 0xin's roadmap is a
sequence of **stages**, each ending in a concrete thing you can see or test —
"a nested window shows a solid color," "a terminal appears," "I can type into
it." That's not a documentation choice, it's how the project is actually
built: see [Why phase gates](phases/README.md) for the reasoning, and the
Build Phases chapters for what each stage was and how it went.

## Status

0xin runs nested inside another Wayland session for fast iteration, and as a
real DRM/KMS session on a bare TTY. It tiles windows, switches workspaces,
reads a config file, survives VT switching, and drives multiple monitors with
configurable position and scale. It's being grown into something to
daily-drive, one capability at a time — see the [repository
README](https://github.com/termworks/0xin#readme) for the current feature list,
or jump straight to the [build phases](phases/README.md) for the story of how
it got there.
