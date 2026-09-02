//! `oxin.action` — the built-in actions a chord or gesture can be bound to.
//!
//! An action is carried as userdata wrapping the real `Action`, not as a
//! string the host re-parses. That means a mistyped action is caught where it
//! is written rather than where it is used, which matters more than it looks:
//! `oxin.keys["MOD+q"] = oxin.action.clsoe` would otherwise evaluate to `nil`
//! and *silently unbind* the key, since `nil` is how a config removes a
//! binding. So `__index` raises on a name that does not exist.

use luna::{Callback, CallbackReturn, Context, Error, IntoValue, Table, UserData, Value};

use crate::config::{Action, Direction};

/// Actions that take no argument, as they are written in a config.
const PLAIN: &[(&str, Action)] = &[
    ("close", Action::Close),
    ("quit", Action::Quit),
    ("fullscreen", Action::Fullscreen),
    ("solo", Action::ToggleSolo),
    ("float", Action::ToggleFloating),
    ("focus_next", Action::FocusNext),
    ("focus_prev", Action::FocusPrev),
    ("workspace_next", Action::WorkspaceNext),
    ("workspace_prev", Action::WorkspacePrevious),
    ("move_to_workspace_next", Action::MoveToWorkspaceNext),
    ("move_to_workspace_prev", Action::MoveToWorkspacePrevious),
    ("keyboard_show", Action::KeyboardShow),
    ("keyboard_hide", Action::KeyboardHide),
    ("keyboard_toggle", Action::KeyboardToggle),
];

/// The ones that take an argument, for the error message.
const WITH_ARG: &[&str] = &[
    "spawn",
    "focus",
    "move",
    "resize",
    "workspace",
    "move_to_workspace",
];

/// Wrap an `Action` as a Lua value.
pub(crate) fn wrap<'gc>(ctx: Context<'gc>, action: Action) -> Value<'gc> {
    UserData::new_static(&ctx, action).into_value(ctx)
}

/// Read an `Action` back out, if that is what this value is.
pub(crate) fn unwrap(value: Value<'_>) -> Option<Action> {
    match value {
        Value::UserData(ud) => ud.downcast_static::<Action>().ok().cloned(),
        _ => None,
    }
}

/// Build the `oxin.action` table.
pub(crate) fn build<'gc>(ctx: Context<'gc>) -> Result<Table<'gc>, Error<'gc>> {
    let action = Table::new(&ctx);

    for (name, act) in PLAIN {
        action.set_raw(&ctx, (*name).into_value(ctx), wrap(ctx, act.clone()))?;
    }

    action.set_field(
        ctx,
        "spawn",
        Callback::from_fn(&ctx, |ctx, _, mut stack| {
            let cmd: luna::String = stack.consume(ctx)?;
            let cmd = String::from_utf8_lossy(cmd.as_bytes()).into_owned();
            stack.replace(ctx, wrap(ctx, Action::Spawn(cmd)));
            Ok(CallbackReturn::Return)
        }),
    );

    directional(ctx, action, "focus", Action::MoveFocus)?;
    directional(ctx, action, "move", Action::MoveWindow)?;
    directional(ctx, action, "resize", Action::ResizeWindow)?;

    workspace_taking(ctx, action, "workspace", Action::Workspace)?;
    workspace_taking(ctx, action, "move_to_workspace", Action::MoveToWorkspace)?;

    // Reading a name that is not here is a mistake, not a `nil`.
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
                        "oxin.action has no {name:?}. Actions taking no argument: {}. \
                         Taking one: {}.",
                        PLAIN.iter().map(|(n, _)| *n).collect::<Vec<_>>().join(", "),
                        WITH_ARG.join(", "),
                    )
                    .as_bytes(),
                )
                .into_value(ctx),
            ))
        }),
    );
    action.set_metatable(ctx, Some(meta));

    Ok(action)
}

/// `oxin.action.focus("left")` and friends.
fn directional<'gc>(
    ctx: Context<'gc>,
    action: Table<'gc>,
    name: &'static str,
    make: fn(Direction) -> Action,
) -> Result<(), Error<'gc>> {
    action.set_field(
        ctx,
        name,
        Callback::from_fn(&ctx, move |ctx, _, mut stack| {
            let dir: luna::String = stack.consume(ctx)?;
            let dir = String::from_utf8_lossy(dir.as_bytes()).into_owned();
            let dir = match direction(&dir) {
                Some(d) => d,
                None => {
                    return Err(Error::from_value(
                        ctx.intern(
                            format!(
                                "oxin.action.{name}: expected \"left\", \"right\", \"up\" or \
                                 \"down\", got {dir:?}"
                            )
                            .as_bytes(),
                        )
                        .into_value(ctx),
                    ))
                }
            };
            stack.replace(ctx, wrap(ctx, make(dir)));
            Ok(CallbackReturn::Return)
        }),
    );
    Ok(())
}

/// `oxin.action.workspace(3)`. Workspaces are 1-based in a config, the way Lua
/// counts and the way the old `.conf` format did; the enum is 0-based.
fn workspace_taking<'gc>(
    ctx: Context<'gc>,
    action: Table<'gc>,
    name: &'static str,
    make: fn(usize) -> Action,
) -> Result<(), Error<'gc>> {
    action.set_field(
        ctx,
        name,
        Callback::from_fn(&ctx, move |ctx, _, mut stack| {
            let n: i64 = stack.consume(ctx)?;
            if !(1..=crate::state::WORKSPACE_COUNT as i64).contains(&n) {
                return Err(Error::from_value(
                    ctx.intern(
                        format!(
                            "oxin.action.{name}: workspaces are numbered 1 to {}, got {n}",
                            crate::state::WORKSPACE_COUNT
                        )
                        .as_bytes(),
                    )
                    .into_value(ctx),
                ));
            }
            stack.replace(ctx, wrap(ctx, make(n as usize - 1)));
            Ok(CallbackReturn::Return)
        }),
    );
    Ok(())
}

/// Accepts the long names and the single letters the `.conf` format used.
fn direction(s: &str) -> Option<Direction> {
    match s.to_ascii_lowercase().as_str() {
        "l" | "left" => Some(Direction::Left),
        "r" | "right" => Some(Direction::Right),
        "u" | "up" => Some(Direction::Up),
        "d" | "down" => Some(Direction::Down),
        _ => None,
    }
}
