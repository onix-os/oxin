//! Running Lua to completion under a budget we control.
//!
//! luna's own `Lua::finish` and `Lua::execute` are unusable here: their fuel
//! constant is a *per-slice* budget refilled on every iteration, not a total,
//! so both spin forever on `while true do end`. A compositor cannot afford
//! that — a config typo would wedge the session with no way back.
//!
//! So the host owns the loop. `Executor::step` is the bounded primitive, and
//! running out of fuel is resumable rather than an error: it returns
//! `Ok(false)` and nothing is lost. That is what lets us stop on our own terms.

use std::time::{Duration, Instant};

use luna::{Fuel, Lua, StashedExecutor};

/// Fuel handed to one `step`. Bounds how long a single call holds the GC
/// arena; luna's own `finish` uses 4096 for the same purpose.
const FUEL_PER_SLICE: i32 = 8_192;

/// Total fuel one synchronous handler may spend. A handler doing real work
/// (walking the workspaces, formatting a string) is orders of magnitude under
/// this; it is a backstop for accidental loops, not a tuned figure.
pub(crate) const FUEL_PER_HANDLER: i32 = 2_000_000;

/// Total fuel the config chunk may spend. Startup is allowed to be slow, and
/// killing a config half-way is far worse than a slow start.
pub(crate) const FUEL_FOR_CONFIG: i32 = 200_000_000;

/// Wall-clock ceiling for one handler. Roughly three frames at 60Hz: a visible
/// stutter, not a hang. This — not the fuel number — is the real guarantee,
/// because fuel counts instructions and a single `string.rep("x", 1e9)` can
/// blow the clock inside one slice.
pub(crate) const HANDLER_DEADLINE: Duration = Duration::from_millis(50);

/// Wall-clock ceiling for the config chunk.
pub(crate) const CONFIG_DEADLINE: Duration = Duration::from_secs(2);

/// How a drive ended.
#[derive(Debug)]
pub(crate) enum Outcome {
    /// Ran to completion. The result is still on the executor for the caller
    /// to take inside its own `try_enter`.
    Finished,
    /// We stopped it: out of fuel, or past the deadline. Whatever it was doing
    /// is half-applied, and the caller is responsible for re-normalising.
    Killed(&'static str),
    /// luna refused to step. This means a host bug (an executor driven from
    /// two places), never a config bug.
    Broken(String),
}

/// Drive `ex` until it finishes, exhausts `budget`, or passes `deadline`.
///
/// The caller must have parked whatever the host callbacks need (see
/// [`super::park`]) before calling this, and must not hold any borrow the
/// callbacks will also want.
pub(crate) fn drive(
    vm: &mut Lua,
    ex: &StashedExecutor,
    budget: i32,
    deadline: Duration,
) -> Outcome {
    let start = Instant::now();
    let mut spent: i64 = 0;

    loop {
        let remaining = (budget as i64 - spent).max(1);
        let slice = FUEL_PER_SLICE.min(remaining as i32);
        let mut fuel = Fuel::with(slice);
        let before = fuel.remaining();

        // `enter` runs a GC slice on the way out, so collection is paced
        // across the drive for free.
        let finished = match vm.enter(|ctx| ctx.fetch(ex).step(ctx, &mut fuel)) {
            Ok(finished) => finished,
            Err(bad) => {
                vm.enter(|ctx| ctx.fetch(ex).stop(&ctx));
                return Outcome::Broken(bad.to_string());
            }
        };

        spent += i64::from((before - fuel.remaining()).max(0));

        if finished {
            return Outcome::Finished;
        }
        if spent >= i64::from(budget) {
            vm.enter(|ctx| ctx.fetch(ex).stop(&ctx));
            return Outcome::Killed("fuel budget exhausted");
        }
        if start.elapsed() > deadline {
            vm.enter(|ctx| ctx.fetch(ex).stop(&ctx));
            return Outcome::Killed("wall-clock deadline exceeded");
        }
    }
}
