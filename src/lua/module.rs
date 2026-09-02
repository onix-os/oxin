//! The `oxin` table a config assigns settings onto.
//!
//! Settings are *assigned*, not collected into a table the host merges:
//! `oxin.gap = 4`, not `oxin.setup { gap = 4 }`. A setting the config never
//! mentions is left alone, which is what keeps `OXIN_MOD` and the built-in
//! defaults working.
//!
//! The usual cost of that style is that settings share a namespace with the
//! API, so a typo (`oxin.gpa = 4`) is silently ignored — the host cannot tell
//! it from the user stashing a helper on the module. 0xin owns the whole
//! namespace, so it does not have to accept that: every write goes through
//! `__newindex`, which raises on a name it does not know. Helpers go in
//! `local`s, where they belong.
//!
//! `set_intercept_all_writes` is what makes that work for *re*-assignment too;
//! stock Lua fires `__newindex` only for absent keys, so the second
//! `oxin.gap = ...` would slip past the guard.

use luna::{Callback, CallbackReturn, Context, Error, Execution, IntoValue, Table, Value};

use crate::config::parse::{mod_name, parse_mods};
use crate::config::{Config, MOD_LOGO};

use super::park::with_parked;

/// Every setting name the config may assign, in the order they appear in the
/// docs. Used for the guard's "did you mean" and to seed the table.
const SETTINGS: &[&str] = &[
    "modifier",
    "gap",
    "first_split",
    "background",
    "wallpaper",
    "window_opacity",
    "corner_radius",
    "float_size",
    "gesture_handle",
    "keyboard",
];

/// Build the `oxin` module table, seeded from `defaults` so a config can read
/// a setting it has not written (`oxin.gap` before any assignment).
pub(crate) fn build<'gc>(ctx: Context<'gc>, defaults: &Config) -> Result<Table<'gc>, Error<'gc>> {
    let oxin = Table::new(&ctx);

    seed(ctx, oxin, defaults)?;

    let meta = Table::new(&ctx);
    meta.set_field(
        ctx,
        "__newindex",
        Callback::from_fn(&ctx, |ctx, exec, mut stack| {
            let at = position(&exec);
            let (table, key, value): (Table, Value, Value) = stack.consume(ctx)?;
            let name = match key {
                Value::String(s) => String::from_utf8_lossy(s.as_bytes()).into_owned(),
                other => {
                    return Err(err(
                        ctx,
                        &at,
                        format!(
                            "oxin only has named settings, so it cannot be indexed by a {}",
                            other.type_name()
                        ),
                    ))
                }
            };

            // Apply first: if the value is wrong the table keeps the old one,
            // so a failed assignment leaves nothing half-set behind.
            match with_parked::<Config, _>(|cfg| apply(ctx, cfg, &name, value)) {
                Ok(Ok(())) => {}
                Ok(Err(message)) => return Err(err(ctx, &at, message)),
                Err(park) => return Err(err(ctx, &at, park.message().to_owned())),
            }

            table.set_raw(&ctx, key, value)?;
            Ok(CallbackReturn::Return)
        }),
    );
    oxin.set_metatable(ctx, Some(meta));
    // Without this, `__newindex` fires only for keys the table does not have —
    // and every setting is seeded, so the guard would never run at all.
    oxin.set_intercept_all_writes(&ctx, true);

    Ok(oxin)
}

/// Populate the table with the current values, so reads work before writes.
fn seed<'gc>(ctx: Context<'gc>, oxin: Table<'gc>, cfg: &Config) -> Result<(), Error<'gc>> {
    oxin.set_raw(
        &ctx,
        "modifier".into_value(ctx),
        modifier_name(cfg.modifier).into_value(ctx),
    )?;
    oxin.set_raw(
        &ctx,
        "gap".into_value(ctx),
        i64::from(cfg.gap).into_value(ctx),
    )?;
    oxin.set_raw(
        &ctx,
        "first_split".into_value(ctx),
        if cfg.first_split_vertical {
            "vertical"
        } else {
            "horizontal"
        }
        .into_value(ctx),
    )?;
    let bg = Table::new(&ctx);
    bg.set(ctx, 1, f64::from(cfg.background.0))?;
    bg.set(ctx, 2, f64::from(cfg.background.1))?;
    bg.set(ctx, 3, f64::from(cfg.background.2))?;
    oxin.set_raw(&ctx, "background".into_value(ctx), bg.into_value(ctx))?;
    oxin.set_raw(
        &ctx,
        "wallpaper".into_value(ctx),
        match &cfg.wallpaper {
            Some(p) => p.as_str().into_value(ctx),
            None => Value::Nil,
        },
    )?;
    oxin.set_raw(
        &ctx,
        "window_opacity".into_value(ctx),
        f64::from(cfg.window_opacity).into_value(ctx),
    )?;
    oxin.set_raw(
        &ctx,
        "corner_radius".into_value(ctx),
        i64::from(cfg.corner_radius).into_value(ctx),
    )?;
    let fs = Table::new(&ctx);
    fs.set(ctx, 1, i64::from(cfg.float_size.0))?;
    fs.set(ctx, 2, i64::from(cfg.float_size.1))?;
    oxin.set_raw(&ctx, "float_size".into_value(ctx), fs.into_value(ctx))?;
    oxin.set_raw(
        &ctx,
        "gesture_handle".into_value(ctx),
        if cfg.gesture_handle_visible {
            "visible"
        } else {
            "hidden"
        }
        .into_value(ctx),
    )?;
    let kb = Table::new(&ctx);
    if let Some(s) = &cfg.virtual_keyboard_show {
        kb.set(ctx, "show", s.as_str())?;
    }
    if let Some(h) = &cfg.virtual_keyboard_hide {
        kb.set(ctx, "hide", h.as_str())?;
    }
    kb.set(ctx, "height", i64::from(cfg.virtual_keyboard_height))?;
    oxin.set_raw(&ctx, "keyboard".into_value(ctx), kb.into_value(ctx))?;
    Ok(())
}

/// Write one setting into the staged config, or say why it could not be.
///
/// The error strings are the ones a config author reads, so they name the
/// setting and what it wanted rather than quoting a Rust type.
fn apply<'gc>(
    ctx: Context<'gc>,
    cfg: &mut Config,
    name: &str,
    value: Value<'gc>,
) -> Result<(), String> {
    match name {
        "modifier" => {
            let s = want_string(name, value)?;
            cfg.modifier = parse_mods(&s, MOD_LOGO)
                .ok_or_else(|| format!("oxin.modifier: unknown modifier {s:?}"))?;
        }
        "gap" => {
            let g = want_integer(name, value)?;
            if g < 0 {
                return Err("oxin.gap: must not be negative".into());
            }
            cfg.gap = clamp_i32(g);
        }
        "first_split" => {
            cfg.first_split_vertical = match want_string(name, value)?.as_str() {
                "vertical" => true,
                "horizontal" => false,
                other => {
                    return Err(format!(
                        "oxin.first_split: expected \"vertical\" or \"horizontal\", got {other:?}"
                    ))
                }
            };
        }
        "background" => {
            let rgb = want_number_triple(ctx, name, value)?;
            cfg.background = rgb;
        }
        "wallpaper" => {
            cfg.wallpaper = match value {
                Value::Nil => None,
                other => Some(want_string(name, other)?),
            };
        }
        "window_opacity" => {
            let o = want_number(name, value)?;
            if !(0.0..=1.0).contains(&o) {
                return Err("oxin.window_opacity: must be between 0.0 and 1.0".into());
            }
            cfg.window_opacity = o as f32;
        }
        "corner_radius" => {
            let r = want_integer(name, value)?;
            if !(0..=200).contains(&r) {
                return Err("oxin.corner_radius: must be between 0 and 200".into());
            }
            cfg.corner_radius = clamp_i32(r);
        }
        "float_size" => {
            let (w, h) = want_integer_pair(ctx, name, value)?;
            if !(1..=100).contains(&w) || !(1..=100).contains(&h) {
                return Err(
                    "oxin.float_size: both values are percentages, between 1 and 100".into(),
                );
            }
            cfg.float_size = (clamp_i32(w), clamp_i32(h));
        }
        "gesture_handle" => {
            cfg.gesture_handle_visible = match want_string(name, value)?.as_str() {
                "visible" => true,
                "hidden" => false,
                other => {
                    return Err(format!(
                        "oxin.gesture_handle: expected \"visible\" or \"hidden\", got {other:?}"
                    ))
                }
            };
        }
        "keyboard" => {
            let t = want_table(name, value)?;
            // A partial table is fine — an unmentioned member keeps its value,
            // the same rule the top-level settings follow.
            for (k, v) in t.iter(ctx) {
                let k = want_string("keyboard", k)?;
                match k.as_str() {
                    "show" => cfg.virtual_keyboard_show = Some(want_string("keyboard.show", v)?),
                    "hide" => cfg.virtual_keyboard_hide = Some(want_string("keyboard.hide", v)?),
                    "height" => {
                        let h = want_integer("keyboard.height", v)?;
                        if !(80..=1000).contains(&h) {
                            return Err("oxin.keyboard.height: must be between 80 and 1000".into());
                        }
                        cfg.virtual_keyboard_height = clamp_i32(h);
                    }
                    other => {
                        return Err(format!(
                            "oxin.keyboard has no {other:?}; it takes show, hide and height"
                        ))
                    }
                }
            }
        }
        unknown => return Err(unknown_setting(unknown)),
    }
    Ok(())
}

/// The message for a name the module does not have. Naming the nearest known
/// setting turns the common case — a typo — into a one-line fix.
fn unknown_setting(name: &str) -> String {
    match SETTINGS.iter().copied().min_by_key(|k| distance(k, name)) {
        Some(near) if distance(near, name) <= 3 => {
            format!("oxin has no setting {name:?} — did you mean {near:?}?")
        }
        _ => format!(
            "oxin has no setting {name:?}. Settings are: {}. \
             (Helpers belong in a `local`, not on oxin.)",
            SETTINGS.join(", ")
        ),
    }
}

/// Levenshtein distance, only ever run on a config error path.
fn distance(a: &str, b: &str) -> usize {
    let (a, b): (Vec<char>, Vec<char>) = (a.chars().collect(), b.chars().collect());
    let mut prev: Vec<usize> = (0..=b.len()).collect();
    let mut cur = vec![0usize; b.len() + 1];
    for (i, ac) in a.iter().enumerate() {
        cur[0] = i + 1;
        for (j, bc) in b.iter().enumerate() {
            let cost = usize::from(ac != bc);
            cur[j + 1] = (prev[j] + cost).min(prev[j + 1] + 1).min(cur[j] + 1);
        }
        std::mem::swap(&mut prev, &mut cur);
    }
    prev[b.len()]
}

fn clamp_i32(v: i64) -> i32 {
    v.clamp(i64::from(i32::MIN), i64::from(i32::MAX)) as i32
}

/// Where in the config the current native call was made from.
///
/// luna positions errors raised *by Lua* ("init.lua:3: boom") but not ones a
/// native callback returns, and a config error with no line number is the one
/// thing a config author most needs. The calling frame knows both, so we
/// prefix it ourselves and the two kinds read alike.
fn position(exec: &Execution<'_, '_>) -> Option<String> {
    let frame = exec.upper_lua_frame()?;

    // `current_line` is attributed to the instruction about to run. luna steps
    // back one when the previous opcode is a Call, which covers a native
    // invoked as a function — but `__newindex` is triggered by a *store*, and
    // by the time we run, the pc has moved on to the next statement. So a
    // failed `oxin.gpa = 4` on line 3 would be reported as line 4. Look up the
    // store itself instead.
    let proto = frame.closure.prototype();
    let line = frame
        .pc
        .checked_sub(1)
        .and_then(|prev| {
            match proto
                .opcode_line_numbers
                .binary_search_by_key(&prev, |(op, _)| *op)
            {
                Ok(i) => Some(proto.opcode_line_numbers[i].1),
                Err(0) => None,
                Err(i) => Some(proto.opcode_line_numbers[i - 1].1),
            }
        })
        .unwrap_or(frame.current_line);

    Some(format!("{}:{}", frame.chunk_name.display_lossy(), line))
}

fn err<'gc>(ctx: Context<'gc>, at: &Option<String>, message: String) -> Error<'gc> {
    let text = match at {
        Some(at) => format!("{at}: {message}"),
        None => message,
    };
    Error::from_value(ctx.intern(text.as_bytes()).into_value(ctx))
}

fn want_string(name: &str, value: Value<'_>) -> Result<String, String> {
    match value {
        Value::String(s) => Ok(String::from_utf8_lossy(s.as_bytes()).into_owned()),
        other => Err(format!(
            "oxin.{name}: expected a string, got {}",
            other.type_name()
        )),
    }
}

fn want_integer(name: &str, value: Value<'_>) -> Result<i64, String> {
    value.to_integer().ok_or_else(|| {
        format!(
            "oxin.{name}: expected a whole number, got {}",
            value.type_name()
        )
    })
}

fn want_number(name: &str, value: Value<'_>) -> Result<f64, String> {
    value
        .to_number()
        .ok_or_else(|| format!("oxin.{name}: expected a number, got {}", value.type_name()))
}

fn want_table<'gc>(name: &str, value: Value<'gc>) -> Result<Table<'gc>, String> {
    match value {
        Value::Table(t) => Ok(t),
        other => Err(format!(
            "oxin.{name}: expected a table, got {}",
            other.type_name()
        )),
    }
}

fn want_number_triple<'gc>(
    ctx: Context<'gc>,
    name: &str,
    value: Value<'gc>,
) -> Result<(f32, f32, f32), String> {
    let t = want_table(name, value)?;
    let mut out = [0.0f32; 3];
    for (i, slot) in out.iter_mut().enumerate() {
        let v = t.get_value(ctx, i as i64 + 1);
        let n = v.to_number().ok_or_else(|| {
            format!("oxin.{name}: expected three numbers, e.g. {{ 0.0, 0.6, 0.6 }}")
        })?;
        *slot = n as f32;
    }
    Ok((out[0], out[1], out[2]))
}

fn want_integer_pair<'gc>(
    ctx: Context<'gc>,
    name: &str,
    value: Value<'gc>,
) -> Result<(i64, i64), String> {
    let t = want_table(name, value)?;
    let w = t
        .get_value(ctx, 1)
        .to_integer()
        .ok_or_else(|| format!("oxin.{name}: expected two whole numbers, e.g. {{ 60, 60 }}"))?;
    let h = t
        .get_value(ctx, 2)
        .to_integer()
        .ok_or_else(|| format!("oxin.{name}: expected two whole numbers, e.g. {{ 60, 60 }}"))?;
    Ok((w, h))
}

/// The name we round-trip a modifier back to, so `oxin.modifier` reads the way
/// it was written.
fn modifier_name(m: u32) -> &'static str {
    mod_name(m)
}
