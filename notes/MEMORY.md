# 0xin — memory index

One line per note; open the file for the full fact. Verify before asserting.

- [env-and-toolchain](env-and-toolchain.md) — pinned versions, pkg-config names, include dirs, test clients.
- [running-and-verifying](running-and-verifying.md) — `cargo nested`, the screenshot+log verification recipe, headless caveat.
- [architecture](architecture.md) — what lives in Rust vs the C shim, and why (wlroots types are opaque to Rust).
- [signals-and-spawning](signals-and-spawning.md) — SIG_IGN and the blocked mask survive exec; every spawn must reset child signals (pre_exec) or clients break subtly.
