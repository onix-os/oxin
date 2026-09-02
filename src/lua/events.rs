//! `oxin.on` — behaviour the config registers, run when something happens.
//!
//! Registration repeats: `oxin.on.window(f)` may be called as often as the
//! config likes, and the handlers run in the order they were registered. That
//! is the whole reason this is a call rather than a field to assign — if a
//! hook were one slot, everything a config does would have to pile into a
//! single function.
//!
//! Handlers get their own error boundary. One that raises is reported and the
//! rest still run: a mistake in the third has nothing to do with the fourth.
//!
//! Return contract, uniform across the three:
//!
//! - `on.window` *influences*: `nil` means "not mine, carry on", a table means
//!   "here is what to do instead".
//! - `on.startup` and `on.workspace` are side effects; their return is ignored.

use luna::{Callback, CallbackReturn, Context, Error, IntoValue, Table, Value};

use super::host::with_staged;

/// The events a config can register for.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub(crate) enum Event {
    Startup,
    Window,
    Workspace,
}

impl Event {
    pub(crate) const ALL: [Event; 3] = [Event::Startup, Event::Window, Event::Workspace];

    pub(crate) fn name(self) -> &'static str {
        match self {
            Event::Startup => "startup",
            Event::Window => "window",
            Event::Workspace => "workspace",
        }
    }

    pub(crate) fn index(self) -> usize {
        match self {
            Event::Startup => 0,
            Event::Window => 1,
            Event::Workspace => 2,
        }
    }
}

/// Build `oxin.on`.
pub(crate) fn build<'gc>(ctx: Context<'gc>) -> Result<Table<'gc>, Error<'gc>> {
    let on = Table::new(&ctx);
    for event in Event::ALL {
        on.set_field(
            ctx,
            event.name(),
            Callback::from_fn(&ctx, move |ctx, exec, mut stack| {
                let at = super::module::position(&exec);
                let f: Value = stack.consume(ctx)?;
                let f = match f {
                    Value::Function(f) => f,
                    other => {
                        return Err(super::module::err(
                            ctx,
                            &at,
                            format!(
                                "oxin.on.{} takes a function, got {}",
                                event.name(),
                                other.type_name()
                            ),
                        ))
                    }
                };
                let stashed = ctx.stash(f);
                let outcome = with_staged("oxin.on", move |staged| {
                    staged.handlers[event.index()].push(stashed);
                });
                match outcome {
                    Ok(Ok(())) => Ok(CallbackReturn::Return),
                    Ok(Err(message)) => Err(super::module::err(ctx, &at, message)),
                    Err(park) => Err(super::module::err(ctx, &at, park.message().to_owned())),
                }
            }),
        );
    }

    // A name that is not an event is a typo, not a nil.
    let meta = Table::new(&ctx);
    meta.set_field(
        ctx,
        "__index",
        Callback::from_fn(&ctx, |ctx, _, mut stack| {
            let (_, key): (Table, Value) = stack.consume(ctx)?;
            let name = match key {
                Value::String(s) => String::from_utf8_lossy(s.as_bytes()).into_owned(),
                other => other.type_name().to_owned(),
            };
            Err(Error::from_value(
                ctx.intern(
                    format!(
                        "oxin.on has no event {name:?}. Events are: {}.",
                        Event::ALL
                            .iter()
                            .map(|e| e.name())
                            .collect::<Vec<_>>()
                            .join(", ")
                    )
                    .as_bytes(),
                )
                .into_value(ctx),
            ))
        }),
    );
    on.set_metatable(ctx, Some(meta));

    Ok(on)
}

/// What `on.window` is told about a window that just mapped.
#[derive(Default, Debug)]
pub(crate) struct WindowRecord {
    pub(crate) app_id: Option<String>,
    pub(crate) title: Option<String>,
    /// 1-based, the way a config counts.
    pub(crate) workspace: i64,
    pub(crate) floating: bool,
}

impl WindowRecord {
    pub(crate) fn to_table<'gc>(&self, ctx: Context<'gc>) -> Result<Table<'gc>, Error<'gc>> {
        let t = Table::new(&ctx);
        t.set(ctx, "app_id", self.app_id.as_deref())?;
        t.set(ctx, "title", self.title.as_deref())?;
        t.set(ctx, "workspace", self.workspace)?;
        t.set(ctx, "floating", self.floating)?;
        Ok(t)
    }
}

/// What `on.window` may ask for instead of the default placement.
#[derive(Default, Debug, PartialEq)]
pub(crate) struct WindowRule {
    pub(crate) float: Option<bool>,
    /// 1-based, as written in the config.
    pub(crate) workspace: Option<i64>,
}

/// Read a handler's return value.
///
/// `nil` is "not mine"; a table is a rule. Anything else is a mistake worth
/// naming, since silently ignoring it would make a typo invisible.
pub(crate) fn read_rule<'gc>(
    ctx: Context<'gc>,
    value: Value<'gc>,
) -> Result<Option<WindowRule>, String> {
    match value {
        Value::Nil => Ok(None),
        Value::Table(t) => {
            let float = match t.get_value(ctx, "float") {
                Value::Nil => None,
                other => Some(other.to_bool()),
            };
            let workspace = match t.get_value(ctx, "workspace") {
                Value::Nil => None,
                other => Some(
                    other
                        .to_integer()
                        .ok_or_else(|| "on.window: `workspace` must be a number".to_owned())?,
                ),
            };
            Ok(Some(WindowRule { float, workspace }))
        }
        other => Err(format!(
            "on.window must return nil or a table like {{ float = true }}, got {}",
            other.type_name()
        )),
    }
}
