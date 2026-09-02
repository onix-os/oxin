//! Building the interpreter a 0xin config runs in.
//!
//! `Lua::full()` is the wrong starting point for a compositor. It brings in
//! `os` and `io`, and three entries there are actively dangerous to a running
//! session:
//!
//! - `os.exit` calls `std::process::exit` straight away, so a stray call in a
//!   config kills the session and skips the client teardown in `main`.
//! - `os.execute` shells out and blocks on the child, freezing the compositor
//!   for as long as it runs — and it bypasses `keybindings::reset_signals`,
//!   which every spawn path must go through (see `notes/signals-and-spawning`).
//! - `debug.setlocal` lets a script rewrite its caller's variables.
//!
//! So we start from `core()` and add back only what a config genuinely needs.
//! Anything the config should be able to do, it does through `oxin.*`, where
//! the host decides what it means.

use luna::{Callback, CallbackReturn, Lua, Value};

/// Ceiling on the whole VM's heap. A config that runs away allocating is
/// stopped here; the driver's fuel and deadline handle the ones that spin.
const MEMORY_LIMIT: usize = 64 * 1024 * 1024;

/// Build the interpreter, with the pieces a config may reach and nothing else.
pub(crate) fn build() -> Lua {
    let mut lua = Lua::core();
    // `require` needs `package`, and a config that grows past one file wants
    // it. It cannot load C modules — luna has no `package.cpath` at all.
    lua.load_package();
    lua.set_memory_limit(Some(MEMORY_LIMIT));

    lua.enter(|ctx| {
        // `core()` ships no `print` — it lives in luna's `io` library, which we
        // are not loading — so a config's first `print(...)` would fail with
        // "attempt to call a nil value". Give it one that goes to stderr in the
        // compositor's own log style.
        ctx.set_global(
            "print",
            Callback::from_fn(&ctx, |ctx, _, mut stack| {
                let mut line = String::new();
                for (i, value) in stack.drain(..).enumerate() {
                    if i > 0 {
                        line.push('\t');
                    }
                    match value {
                        // Bare, so `print("hi")` logs `hi` rather than `"hi"`.
                        Value::String(s) => {
                            line.push_str(&String::from_utf8_lossy(s.as_bytes()));
                        }
                        // Unlike Lua's own `print` this does not run a
                        // `__tostring` metamethod — doing so needs a callback
                        // sequence, and a config's log line does not warrant it.
                        other => line.push_str(&other.display().to_string()),
                    }
                }
                let _ = ctx;
                eprintln!("0xin: lua: {line}");
                Ok(CallbackReturn::Return)
            }),
        );

        // Reading and compiling files is the host's job: `init.lua` and the
        // plugin path are loaded by 0xin, with a chunk name and a budget.
        // Leaving these reachable would let a config sidestep both.
        ctx.set_global("dofile", Value::Nil);
        ctx.set_global("loadfile", Value::Nil);
        ctx.set_global("load", Value::Nil);
    });

    lua
}
