//! The keyed registrars: `oxin.keys`, `oxin.hold`, `oxin.gestures`,
//! `oxin.monitors`.
//!
//! Each is a table you assign into, keyed by the thing it is about — a chord,
//! a gesture name, a connector name. Keyed rather than append-only because
//! every one of these has an identity that can be *replaced*: binding `MOD+q`
//! twice should leave one binding, not two. That also makes them safe to run
//! again, which is what lets a plugin or a later file override an earlier one
//! by naming the same key.
//!
//! They are proxies rather than plain tables: the write goes straight into the
//! staged config, so `"MOD+SHIFT+q"` and `"SHIFT+MOD+q"` land on the same
//! entry instead of sitting in the table as two different strings.
//!
//! Assigning `nil` removes a binding — including a built-in default, which the
//! old `.conf` format had no way to express.

use luna::{Callback, CallbackReturn, Context, Error, IntoValue, Table, Value};

use crate::config::names::{keysym_from_name, parse_mods};
use crate::config::{Bind, GestureBind, GestureTrigger, HoldBind, MonitorConfig, MOD_MASK};

use super::action::unwrap as unwrap_action;
use super::host::with_staged;
use super::staging::Staged;

/// Install `keys`, `hold`, `gestures` and `monitors` onto the module.
pub(crate) fn install<'gc>(ctx: Context<'gc>, oxin: Table<'gc>) -> Result<(), Error<'gc>> {
    oxin.set_raw(&ctx, "keys".into_value(ctx), proxy(ctx, Kind::Keys)?)?;
    oxin.set_raw(&ctx, "hold".into_value(ctx), proxy(ctx, Kind::Hold)?)?;
    oxin.set_raw(
        &ctx,
        "gestures".into_value(ctx),
        proxy(ctx, Kind::Gestures)?,
    )?;
    oxin.set_raw(
        &ctx,
        "monitors".into_value(ctx),
        proxy(ctx, Kind::Monitors)?,
    )?;
    Ok(())
}

#[derive(Clone, Copy)]
enum Kind {
    Keys,
    Hold,
    Gestures,
    Monitors,
}

impl Kind {
    fn name(self) -> &'static str {
        match self {
            Kind::Keys => "oxin.keys",
            Kind::Hold => "oxin.hold",
            Kind::Gestures => "oxin.gestures",
            Kind::Monitors => "oxin.monitors",
        }
    }
}

fn proxy<'gc>(ctx: Context<'gc>, kind: Kind) -> Result<Value<'gc>, Error<'gc>> {
    let table = Table::new(&ctx);
    let meta = Table::new(&ctx);
    meta.set_field(
        ctx,
        "__newindex",
        Callback::from_fn(&ctx, move |ctx, exec, mut stack| {
            let at = super::module::position(&exec);
            let (_, key, value): (Table, Value, Value) = stack.consume(ctx)?;
            let name = match key {
                Value::String(s) => String::from_utf8_lossy(s.as_bytes()).into_owned(),
                other => {
                    return Err(super::module::err(
                        ctx,
                        &at,
                        format!(
                            "{} is keyed by name, not by {}",
                            kind.name(),
                            other.type_name()
                        ),
                    ))
                }
            };
            let outcome = with_staged(kind.name(), |staged| {
                assign(ctx, staged, kind, &name, value)
            });
            match outcome {
                Ok(Ok(Ok(()))) => Ok(CallbackReturn::Return),
                Ok(Ok(Err(message))) | Ok(Err(message)) => {
                    Err(super::module::err(ctx, &at, message))
                }
                Err(park) => Err(super::module::err(ctx, &at, park.message().to_owned())),
            }
        }),
    );
    // Everything is stored on the Rust side, so the table itself stays empty
    // and `__newindex` fires for every write, first or not.
    table.set_metatable(ctx, Some(meta));
    table.set_intercept_all_writes(&ctx, true);
    Ok(table.into_value(ctx))
}

fn assign<'gc>(
    ctx: Context<'gc>,
    staged: &mut Staged,
    kind: Kind,
    name: &str,
    value: Value<'gc>,
) -> Result<(), String> {
    match kind {
        Kind::Keys => {
            let chord = chord(staged, name)?;
            match value {
                Value::Nil => staged.clear_bind(chord),
                other => {
                    let action = action_from(ctx, staged, "oxin.keys", other)?;
                    staged.set_bind(Bind {
                        mods: chord.0,
                        keysym: chord.1,
                        action,
                    });
                }
            }
        }
        Kind::Hold => {
            let chord = chord(staged, name)?;
            match value {
                Value::Nil => {
                    staged
                        .config
                        .hold_binds
                        .retain(|h| (h.mods, h.keysym) != chord);
                }
                Value::Table(t) => {
                    let ms = t.get_value(ctx, "ms").to_integer().ok_or_else(|| {
                        "oxin.hold takes { ms = <milliseconds>, action = ... }".to_owned()
                    })?;
                    if !(1..=60_000).contains(&ms) {
                        return Err("oxin.hold: ms must be between 1 and 60000".into());
                    }
                    let action = action_from(ctx, staged, "oxin.hold", t.get_value(ctx, "action"))?;
                    let entry = HoldBind {
                        mods: chord.0,
                        keysym: chord.1,
                        duration_ms: ms as i32,
                        action,
                    };
                    match staged
                        .config
                        .hold_binds
                        .iter_mut()
                        .find(|h| (h.mods, h.keysym) == chord)
                    {
                        Some(existing) => *existing = entry,
                        None => staged.config.hold_binds.push(entry),
                    }
                }
                other => {
                    return Err(format!(
                        "oxin.hold takes {{ ms = ..., action = ... }}, got {}",
                        other.type_name()
                    ))
                }
            }
        }
        Kind::Gestures => {
            let trigger = gesture_trigger(name)?;
            match value {
                Value::Nil => staged.config.gestures.retain(|g| g.trigger != trigger),
                other => {
                    let action = action_from(ctx, staged, "oxin.gestures", other)?;
                    let entry = GestureBind { trigger, action };
                    match staged
                        .config
                        .gestures
                        .iter_mut()
                        .find(|g| g.trigger == trigger)
                    {
                        Some(existing) => *existing = entry,
                        None => staged.config.gestures.push(entry),
                    }
                }
            }
        }
        Kind::Monitors => match value {
            Value::Nil => staged.config.monitors.retain(|m| m.name != name),
            Value::Table(t) => {
                let x = t.get_value(ctx, "x").to_integer().unwrap_or(0);
                let y = t.get_value(ctx, "y").to_integer().unwrap_or(0);
                let scale = t.get_value(ctx, "scale").to_number().unwrap_or(1.0);
                if scale <= 0.0 {
                    return Err("oxin.monitors: scale must be greater than 0".into());
                }
                let entry = MonitorConfig {
                    name: name.to_owned(),
                    x: x as i32,
                    y: y as i32,
                    scale: scale as f32,
                };
                match staged.config.monitors.iter_mut().find(|m| m.name == name) {
                    Some(existing) => *existing = entry,
                    None => staged.config.monitors.push(entry),
                }
            }
            other => {
                return Err(format!(
                    "oxin.monitors takes {{ x = ..., y = ..., scale = ... }}, got {}",
                    other.type_name()
                ))
            }
        },
    }
    Ok(())
}

/// Turn the right-hand side of a binding into an `Action`.
///
/// Either a built-in from `oxin.action`, or a Lua function — which is stored
/// on the side and referred to by id, so `Action` never holds an interpreter
/// type.
fn action_from<'gc>(
    ctx: Context<'gc>,
    staged: &mut Staged,
    what: &str,
    value: Value<'gc>,
) -> Result<crate::config::Action, String> {
    if let Some(action) = unwrap_action(value) {
        return Ok(action);
    }
    match value {
        Value::Function(f) => {
            let id = staged.add_function(ctx.stash(f));
            Ok(crate::config::Action::Lua(id))
        }
        Value::Nil => Err(format!("{what}: no action given")),
        other => Err(format!(
            "{what}: expected an oxin.action or a function, got {}",
            other.type_name()
        )),
    }
}

/// Resolve `"MOD+SHIFT+q"` to the pair the keyboard handler matches on.
fn chord(staged: &mut Staged, spec: &str) -> Result<(u32, u32), String> {
    let spec = spec.trim();
    if spec.is_empty() {
        return Err("a binding needs a key name".into());
    }

    // The key is the last `+`-separated piece; everything before it is
    // modifiers. Split from the right so a key that is itself `+` still works.
    let (mods_part, key_part) = match spec.rsplit_once('+') {
        Some((m, k)) if !k.trim().is_empty() => (m, k.trim()),
        // Trailing `+`: the key *is* plus, e.g. "MOD++".
        Some((m, _)) => (m, "+"),
        None => ("", spec),
    };

    let mods = if mods_part.is_empty() {
        0
    } else {
        let bits = parse_mods(mods_part, staged.config.modifier).ok_or_else(|| {
            format!(
                "{spec:?}: unknown modifier in {mods_part:?} (use MOD, SUPER, CTRL, ALT, SHIFT)"
            )
        })?;
        if mods_part.to_ascii_uppercase().contains("MOD") {
            staged.mod_resolved = true;
        }
        bits
    };

    let keysym = keysym_from_name(key_part)
        .ok_or_else(|| format!("{spec:?}: xkb does not know a key called {key_part:?}"))?;

    Ok((mods & MOD_MASK, keysym))
}

fn gesture_trigger(name: &str) -> Result<GestureTrigger, String> {
    Ok(match name.to_ascii_lowercase().as_str() {
        "bottom-up" => GestureTrigger::BottomUp,
        "bottom-down" => GestureTrigger::BottomDown,
        "edge-left-in" => GestureTrigger::EdgeLeftIn,
        "edge-right-in" => GestureTrigger::EdgeRightIn,
        "edge-left-up" => GestureTrigger::EdgeLeftUp,
        "edge-left-down" => GestureTrigger::EdgeLeftDown,
        "edge-right-up" => GestureTrigger::EdgeRightUp,
        "edge-right-down" => GestureTrigger::EdgeRightDown,
        "top-right" => GestureTrigger::TopRight,
        "top-left" => GestureTrigger::TopLeft,
        "top-down" => GestureTrigger::TopDown,
        "to-top" => GestureTrigger::ToTop,
        "to-left" => GestureTrigger::ToLeft,
        "to-right" => GestureTrigger::ToRight,
        "two-up" => GestureTrigger::TwoUp,
        "two-down" => GestureTrigger::TwoDown,
        "two-left" => GestureTrigger::TwoLeft,
        "two-right" => GestureTrigger::TwoRight,
        "three-up" => GestureTrigger::ThreeUp,
        "three-down" => GestureTrigger::ThreeDown,
        "three-left" => GestureTrigger::ThreeLeft,
        "three-right" => GestureTrigger::ThreeRight,
        "double-tap" => GestureTrigger::DoubleTap,
        "pad-three-left" => GestureTrigger::PadThreeLeft,
        "pad-three-right" => GestureTrigger::PadThreeRight,
        "pad-three-up" => GestureTrigger::PadThreeUp,
        "pad-three-down" => GestureTrigger::PadThreeDown,
        "pad-pinch-in" => GestureTrigger::PadPinchIn,
        "pad-pinch-out" => GestureTrigger::PadPinchOut,
        other => {
            return Err(format!(
                "oxin.gestures has no trigger {other:?}. Touchscreen triggers are \
                 edge swipes (bottom-up, edge-left-in, top-down, …), multi-finger \
                 swipes (two-left, three-up, …) and double-tap. Touchpad triggers \
                 are pad-three-left/right/up/down and pad-pinch-in/out."
            ))
        }
    })
}
