# Stage 7 — Boot-into-VM

**What it is.** The "boot a Linux kernel, then straight into our own
userspace" milestone: a minimal Linux kernel plus initramfs/rootfs that
boots directly into 0xin on `virtio-gpu`, rather than 0xin being launched
from an already-running Linux install.

**Deliverable** (from `KICKOFF.md`): *minimal Linux + initramfs/rootfs boots
straight into 0xin on virtio-gpu. "Boot a Linux kernel, then our
userspace."*

## Status

**Not started.** This is the stage after [Stage 6](stage-6-real-display.md)
in `KICKOFF.md`'s roadmap; real-hardware DRM/KMS work is still ongoing, and
the `cargo vm` runner this stage implies (mirroring `cargo nested`/`cargo
headless`) doesn't exist yet. Notably out of scope even once this lands:
running on the `snert` kernel itself — this project targets Linux only for
now, and isn't being design-constrained for a future snert port.

This chapter will be filled in once the work actually starts, the same way
every earlier stage was — after building it, not before.
