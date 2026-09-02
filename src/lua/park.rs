//! Parking a host pointer for the duration of a Lua step.
//!
//! luna callbacks are `'static` closures: they cannot capture `&mut Server`.
//! The driver parks the pointer here for exactly as long as it is driving the
//! VM, and host functions pick it back up. Everything a callback can get wrong
//! — running outside a drive, or taking a second `&mut` while one is live —
//! comes back as a catchable Lua error rather than undefined behaviour.
//!
//! The pointer is type-erased so that tests can park something other than a
//! `Server`; `with_parked` re-asserts the type on the way out, and the debug
//! assertion below catches a mismatched pair.

use std::any::TypeId;
use std::cell::Cell;
use std::ptr;

thread_local! {
    /// The host value the currently-running Lua step may touch. Non-null
    /// exactly while a `park` guard is live on the Rust stack below us.
    static PARKED: Cell<*mut ()> = const { Cell::new(ptr::null_mut()) };
    /// Which type `PARKED` points at, so a wrong-typed pickup is caught.
    static PARKED_TY: Cell<Option<TypeId>> = const { Cell::new(None) };
    /// True while a `&mut T` handed out by `with_parked` is live.
    static BORROWED: Cell<bool> = const { Cell::new(false) };
}

/// Why a host function could not reach the parked value.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ParkError {
    /// No drive is in progress — the callback escaped into a context we never
    /// intended it to run in.
    NotParked,
    /// A host function called back into one that is already holding the
    /// `&mut`. This is a 0xin bug, not a config bug, but it must not be UB.
    Reentered,
}

impl ParkError {
    pub(crate) fn message(self) -> &'static str {
        match self {
            ParkError::NotParked => "oxin: host call outside a compositor callback",
            ParkError::Reentered => {
                "oxin: nested host call (a handler ran inside another host function); \
                 this is a 0xin bug"
            }
        }
    }
}

/// Park `value` for the duration of `f`.
///
/// # Safety
///
/// - `value` must be non-null and valid for all of `f`, and the caller must
///   hold it *uniquely*: nothing inside `f` may touch `*value` through any
///   other reference.
/// - `*value` must not be moved or dropped while `f` runs.
/// - Must be called only from the compositor thread.
pub(crate) unsafe fn park<T: 'static, R>(value: *mut T, f: impl FnOnce() -> R) -> R {
    // Restore on the way out even if `f` unwinds, so a panic in a callback
    // does not leave a dangling pointer parked for the next event.
    struct Restore(*mut (), Option<TypeId>);
    impl Drop for Restore {
        fn drop(&mut self) {
            PARKED.with(|p| p.set(self.0));
            PARKED_TY.with(|t| t.set(self.1));
        }
    }

    let prev = PARKED.with(|p| p.replace(value.cast::<()>()));
    let prev_ty = PARKED_TY.with(|t| t.replace(Some(TypeId::of::<T>())));
    let _restore = Restore(prev, prev_ty);
    f()
}

/// Reach the parked value from inside a Lua callback.
///
/// Returns `Err` rather than panicking or misbehaving for both failure modes,
/// so the caller can turn them into Lua errors the config can see.
pub(crate) fn with_parked<T: 'static, R>(f: impl FnOnce(&mut T) -> R) -> Result<R, ParkError> {
    let p = PARKED.with(|c| c.get());
    if p.is_null() {
        return Err(ParkError::NotParked);
    }
    debug_assert_eq!(
        PARKED_TY.with(|t| t.get()),
        Some(TypeId::of::<T>()),
        "parked type does not match the type being picked up",
    );
    if BORROWED.with(|c| c.replace(true)) {
        return Err(ParkError::Reentered);
    }

    struct Release;
    impl Drop for Release {
        fn drop(&mut self) {
            BORROWED.with(|c| c.set(false));
        }
    }
    let _release = Release;

    // SAFETY: a non-null `PARKED` means a `park` guard is live below us on this
    // thread holding a unique `*mut T`, and `BORROWED` was false, so no other
    // `&mut T` derived from it is currently outstanding.
    Ok(f(unsafe { &mut *p.cast::<T>() }))
}

/// True while a drive is in progress. Used by the deferred-event queue to
/// decide whether it is safe to dispatch now or must wait for the unwind.
pub(crate) fn is_parked() -> bool {
    !PARKED.with(|c| c.get()).is_null()
}
