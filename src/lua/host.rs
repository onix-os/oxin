//! What a host function is allowed to touch, and how it reaches it.
//!
//! Lua calls into 0xin from two quite different places. While `init.lua` is
//! loading there is no compositor yet — assignments go to a staged `Config`.
//! Once it is running, the same `oxin.gap = 8` has to reach the live `Server`.
//!
//! Both are parked through the same slot so that a callback can never pick up
//! one thinking it is the other: the parked value says which it is, and the
//! helpers below give a callback the narrowest thing that does the job.

use crate::config::Config;
use crate::state::Server;

use super::park::{with_parked, ParkError};
use super::staging::Staged;

/// The host a running Lua step may reach.
pub(crate) enum HostRef {
    /// `init.lua` is loading; there is no compositor yet.
    Load(*mut Staged),
    /// The compositor is running.
    Live(*mut Server),
}

/// Run `f` against whichever host is parked.
pub(crate) fn with_host<R>(f: impl FnOnce(&mut HostRef) -> R) -> Result<R, ParkError> {
    with_parked::<HostRef, _>(f)
}

/// Run `f` against the config, telling it whether a `MOD+` chord has already
/// been resolved.
///
/// Only the loading case can have bindings pending, so the live case always
/// reports `false`.
pub(crate) fn with_setting_target<R>(
    f: impl FnOnce(&mut Config, bool) -> R,
) -> Result<R, ParkError> {
    with_host(|host| match host {
        // SAFETY: as in `with_config`.
        HostRef::Load(staged) => {
            let staged = unsafe { &mut **staged };
            let used = staged.mod_resolved;
            f(&mut staged.config, used)
        }
        HostRef::Live(server) => f(&mut unsafe { &mut **server }.config, false),
    })
}

/// Run `f` against the staged config, or fail if the compositor is already up.
///
/// Registration — binding a key, adding a handler — only makes sense while the
/// config is loading. Calling one later is a mistake worth naming rather than
/// quietly doing nothing.
pub(crate) fn with_staged<R>(
    what: &str,
    f: impl FnOnce(&mut Staged) -> R,
) -> Result<Result<R, String>, ParkError> {
    with_host(|host| match host {
        // SAFETY: as above.
        HostRef::Load(staged) => Ok(f(unsafe { &mut **staged })),
        HostRef::Live(_) => Err(format!(
            "{what} can only be done while the config is loading, not from a \
             running keybinding or handler"
        )),
    })
}

/// Run `f` against the live compositor, or fail if it is not up yet.
pub(crate) fn with_server<R>(
    what: &str,
    f: impl FnOnce(&mut Server) -> R,
) -> Result<Result<R, String>, ParkError> {
    with_host(|host| match host {
        // SAFETY: as above.
        HostRef::Live(server) => Ok(f(unsafe { &mut **server })),
        HostRef::Load(_) => Err(format!(
            "{what} needs a running compositor; it cannot be called while \
             init.lua is still loading"
        )),
    })
}
