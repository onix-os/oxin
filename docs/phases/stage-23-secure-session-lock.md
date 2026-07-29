# Stage 23 — Secure Session Lock

**What it is.** 0xin implements `ext-session-lock-v1`, allowing an external
authentication client such as `patin-lock` to lock the session without trusting
a normal application or layer-shell overlay as a security boundary.

**Gate:** *Patin acquires the lock, its surface covers every output, its touch
keyboard authenticates through PAM, and killing it leaves an opaque fail-closed
screen.*

## FP5 policy

```ini
bind = , XF86PowerOff, spawn, pgrep -x patin-lock >/dev/null || patin-lock
hold = , XF86PowerOff, 2000, spawn, ~/.local/bin/0xin-session-menu
```

The protocol and short/hold mapping are general 0xin capabilities. The FP5
profile uses Patin as a replaceable external client, not a compositor component.
Releasing before two seconds launches the lock; holding through the threshold
opens the session menu instead. `pgrep` makes repeat presses idempotent.

## Security model

As soon as a lock request is accepted, 0xin:

- raises a compositor-owned opaque fallback above applications, fullscreen
  windows, Patin, wvkbd, and every other layer;
- clears application keyboard focus;
- cancels active touch sequences and disables compositor gestures;
- rejects ordinary compositor bindings and runtime-control requests;
- creates, positions, and configures one lock surface for each output;
- routes pointer, touch, physical-keyboard, and virtual-keyboard input to the
  mapped lock surface.

Only the protocol's authenticated unlock request removes the fallback,
restores gesture policy, and focuses the previous workspace. A second lock
client is rejected while one is active.

## Failure and output handling

If the lock client disconnects without unlocking, 0xin remains locked behind
the opaque fallback. A replacement lock client may connect so the user can
recover without restarting the compositor. New outputs receive fallback
covers while locked; output removal destroys its corresponding cover.

Authentication remains the lock client's responsibility. 0xin does not read
password databases or implement PAM itself.

## Mobile input boundary

The existing wvkbd is an ordinary layer-shell client. Keeping it below the
session-lock tree is necessary: allowing arbitrary shell surfaces above a lock
would let another client cover or imitate the authentication UI. A phone-ready
locker therefore needs an integrated touch PIN/password UI, which `patin-lock`
now supplies. Swaylock remains suitable for protocol testing and for docked use
with a physical keyboard.

## Verification

Verified on the FP5 reference target on 30 July 2026: `patin-lock` acquired the
0xin session lock, rendered its integrated touch keyboard, authenticated the
user through PAM, and unlocked normally.

After enabling the short-power mapping, repository validation passed:

```text
$ cargo test --all-targets
34 tests passed

$ mdbook build
INFO HTML book written to `/home/vdzee/proj/0xin/book`

$ git diff --check
(no output, exit 0)
```
