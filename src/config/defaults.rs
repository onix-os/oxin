//! The built-in bind table, replicating 0xin's original hardcoded behavior.
//! Always the starting point for `Config::binds` — see the module doc
//! comment in `mod.rs` for how a config file's own `bind =` lines layer on
//! top of this instead of replacing it.

use super::parse::key;
use super::types::*;

pub(super) fn default_binds(modifier: u32) -> Vec<Bind> {
    let m = modifier;
    let ms = modifier | MOD_SHIFT;
    let mc = modifier | MOD_CTRL;
    let mut binds = vec![
        Bind {
            mods: m,
            keysym: key("Return"),
            action: Action::Spawn("kitty".into()),
        },
        Bind {
            mods: m,
            keysym: key("Q"),
            action: Action::Close,
        },
        Bind {
            mods: ms,
            keysym: key("Q"),
            action: Action::Quit,
        },
        Bind {
            mods: m,
            keysym: key("H"),
            action: Action::MoveFocus(Direction::Left),
        },
        Bind {
            mods: m,
            keysym: key("J"),
            action: Action::MoveFocus(Direction::Down),
        },
        Bind {
            mods: m,
            keysym: key("K"),
            action: Action::MoveFocus(Direction::Up),
        },
        Bind {
            mods: m,
            keysym: key("L"),
            action: Action::MoveFocus(Direction::Right),
        },
        Bind {
            mods: ms,
            keysym: key("H"),
            action: Action::MoveWindow(Direction::Left),
        },
        Bind {
            mods: ms,
            keysym: key("J"),
            action: Action::MoveWindow(Direction::Down),
        },
        Bind {
            mods: ms,
            keysym: key("K"),
            action: Action::MoveWindow(Direction::Up),
        },
        Bind {
            mods: ms,
            keysym: key("L"),
            action: Action::MoveWindow(Direction::Right),
        },
        Bind {
            mods: mc,
            keysym: key("H"),
            action: Action::ResizeWindow(Direction::Left),
        },
        Bind {
            mods: mc,
            keysym: key("J"),
            action: Action::ResizeWindow(Direction::Down),
        },
        Bind {
            mods: mc,
            keysym: key("K"),
            action: Action::ResizeWindow(Direction::Up),
        },
        Bind {
            mods: mc,
            keysym: key("L"),
            action: Action::ResizeWindow(Direction::Right),
        },
        Bind {
            mods: m,
            keysym: key("F"),
            action: Action::Fullscreen,
        },
        Bind {
            mods: m,
            keysym: key("V"),
            action: Action::ToggleFloating,
        },
    ];
    for i in 0..9u32 {
        let name = (b'1' + i as u8) as char;
        let name = name.to_string();
        binds.push(Bind {
            mods: m,
            keysym: key(&name),
            action: Action::Workspace(i as usize),
        });
        binds.push(Bind {
            mods: ms,
            keysym: key(&name),
            action: Action::MoveToWorkspace(i as usize),
        });
    }
    binds
}
