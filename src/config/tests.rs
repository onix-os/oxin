//! Config parser tests.

use super::*;

#[test]
fn config_binds_override_only_named_chords() {
    let mut cfg = Config::default();
    cfg.binds = default_binds(cfg.modifier);
    let before = cfg.binds.len();

    // Overrides Mod+J (a default MoveFocus bind) back to the old
    // cyclic focusnext, without touching any other default bind.
    cfg.apply_binds("bind = MOD, J, focusnext\n");
    assert_eq!(
        cfg.binds.len(),
        before,
        "override must not grow the bind table"
    );

    let j = key("J");
    let overridden = cfg
        .binds
        .iter()
        .find(|b| b.mods == cfg.modifier && b.keysym == j)
        .unwrap();
    assert!(matches!(overridden.action, Action::FocusNext));

    // An untouched chord (workspace 3) still resolves to its default.
    let three = key("3");
    let untouched = cfg
        .binds
        .iter()
        .find(|b| b.mods == cfg.modifier && b.keysym == three)
        .unwrap();
    assert!(matches!(untouched.action, Action::Workspace(2)));
}

#[test]
fn config_binds_append_new_chords() {
    let mut cfg = Config::default();
    cfg.binds = default_binds(cfg.modifier);
    let before = cfg.binds.len();

    cfg.apply_binds("bind = , Print, spawn, grim\n");
    assert_eq!(
        cfg.binds.len(),
        before + 1,
        "a new chord must be appended, not replace one"
    );
}

#[test]
fn modifier_free_media_keys_map_to_shared_actions() {
    let mut cfg = Config::default();
    cfg.binds = default_binds(cfg.modifier);
    cfg.apply_binds(
        "bind = , XF86AudioRaiseVolume, spawn, pactl set-sink-volume @DEFAULT_SINK@ +5%\n\
         bind = , XF86AudioLowerVolume, spawn, pactl set-sink-volume @DEFAULT_SINK@ -5%\n",
    );

    let raise = key("XF86AudioRaiseVolume");
    let lower = key("XF86AudioLowerVolume");
    let raise_bind = cfg
        .binds
        .iter()
        .find(|bind| bind.mods == 0 && bind.keysym == raise)
        .unwrap();
    let lower_bind = cfg
        .binds
        .iter()
        .find(|bind| bind.mods == 0 && bind.keysym == lower)
        .unwrap();

    assert!(matches!(
        &raise_bind.action,
        Action::Spawn(command)
            if command == "pactl set-sink-volume @DEFAULT_SINK@ +5%"
    ));
    assert!(matches!(
        &lower_bind.action,
        Action::Spawn(command)
            if command == "pactl set-sink-volume @DEFAULT_SINK@ -5%"
    ));
}

#[test]
fn fullscreen_action_parses_and_has_default_bind() {
    let mut cfg = Config::default();
    cfg.binds = default_binds(cfg.modifier);

    // Default: Mod+F toggles fullscreen.
    let f = key("F");
    let default = cfg
        .binds
        .iter()
        .find(|b| b.mods == cfg.modifier && b.keysym == f)
        .unwrap();
    assert!(matches!(default.action, Action::Fullscreen));

    // Both config spellings parse, with no argument required.
    cfg.apply_binds("bind = MOD SHIFT, F, togglefullscreen\n");
    let msf = cfg
        .binds
        .iter()
        .find(|b| b.mods == (cfg.modifier | MOD_SHIFT) && b.keysym == f)
        .unwrap();
    assert!(matches!(msf.action, Action::Fullscreen));
}

#[test]
fn float_rules_parse_lowercased_and_deduplicated() {
    let mut cfg = Config::default();
    cfg.parse_scalars("float = Zenity\nfloat = pavucontrol\nfloat = zenity\n");
    assert_eq!(cfg.float_rules, vec!["zenity", "pavucontrol"]);
}

#[test]
fn first_split_parses_without_changing_desktop_default() {
    let mut cfg = Config::default();
    assert!(cfg.first_split_vertical);

    cfg.parse_scalars("first_split = horizontal\n");
    assert!(!cfg.first_split_vertical);

    cfg.parse_scalars("first_split = vertical\n");
    assert!(cfg.first_split_vertical);
}

#[test]
fn exec_once_preserves_repeated_commands_in_order() {
    let mut cfg = Config::default();
    cfg.parse_scalars(
        "exec_once = shell-toolkit\n\
         exec_once = terminal\n\
         exec_once = virtual-keyboard --hidden\n",
    );

    assert_eq!(
        cfg.exec_once,
        vec!["shell-toolkit", "terminal", "virtual-keyboard --hidden"]
    );
}

#[test]
fn wallpaper_path_is_optional_profile_policy() {
    let mut cfg = Config::default();
    assert!(cfg.wallpaper.is_none());
    cfg.parse_scalars("wallpaper = ~/Pictures/background.jpg\n");
    assert_eq!(cfg.wallpaper.as_deref(), Some("~/Pictures/background.jpg"));
}

#[test]
fn window_opacity_is_opaque_by_default_and_bounded() {
    let mut cfg = Config::default();
    assert_eq!(cfg.window_opacity, 1.0);

    cfg.parse_scalars("window_opacity = 0.8\n");
    assert_eq!(cfg.window_opacity, 0.8);

    cfg.parse_scalars("window_opacity = 1.1\nwindow_opacity = nope\n");
    assert_eq!(cfg.window_opacity, 0.8);
}

#[test]
fn corner_radius_is_disabled_by_default_and_bounded() {
    let mut cfg = Config::default();
    assert_eq!(cfg.corner_radius, 0);

    cfg.parse_scalars("corner_radius = 12\n");
    assert_eq!(cfg.corner_radius, 12);

    cfg.parse_scalars("corner_radius = 201\ncorner_radius = -1\ncorner_radius = nope\n");
    assert_eq!(cfg.corner_radius, 12);
}

#[test]
fn hold_bind_parses_duration_and_action() {
    let mut cfg = Config::default();
    cfg.apply_hold_binds(
        "hold = , XF86PowerOff, 2000, spawn, session-menu\n\
         hold = MOD, q, 99, quit\n",
    );
    assert_eq!(cfg.hold_binds.len(), 1);
    let binding = &cfg.hold_binds[0];
    assert_eq!(binding.duration_ms, 2000);
    assert!(matches!(&binding.action, Action::Spawn(cmd) if cmd == "session-menu"));
}

#[test]
fn virtual_keyboard_and_gestures_are_opt_in() {
    let mut cfg = Config::default();
    assert!(cfg.virtual_keyboard_show.is_none());
    assert!(cfg.gestures.is_empty());
    cfg.parse_scalars(
        "virtual_keyboard_show = pkill -USR2 -x wvkbd-mobintl\n\
         virtual_keyboard_hide = pkill -USR1 -x wvkbd-mobintl\n\
         virtual_keyboard_height = 280\n\
         gesture_handle = hidden\n",
    );
    cfg.apply_gestures(
        "gesture = bottom-up, keyboardshow\n\
         gesture = bottom-down, keyboardhide\n",
    );
    assert_eq!(
        cfg.virtual_keyboard_show.as_deref(),
        Some("pkill -USR2 -x wvkbd-mobintl")
    );
    assert_eq!(cfg.virtual_keyboard_height, 280);
    assert_eq!(cfg.gestures.len(), 2);
    assert_eq!(cfg.gesture_mask(), 0b11);
    assert!(!cfg.gesture_handle_visible);
    assert!(!cfg.has_keyboard_handle());
}

#[test]
fn gesture_override_and_actions_parse() {
    let mut cfg = Config::default();
    cfg.apply_gestures(
        "gesture = edge-left-in, workspaceprev\n\
         gesture = edge-right-in, workspacenext\n\
         gesture = edge-left-in, keyboardtoggle\n",
    );
    assert_eq!(cfg.gestures.len(), 2);
    assert!(matches!(cfg.gestures[0].action, Action::KeyboardToggle));
    assert!(matches!(cfg.gestures[1].action, Action::WorkspaceNext));
    assert_eq!(cfg.gesture_mask(), 0b1100);
}

#[test]
fn top_edge_gestures_parse_as_independent_triggers() {
    let mut cfg = Config::default();
    cfg.apply_gestures(
        "gesture = top-right, spawn, brightnessctl set +5%\n\
         gesture = top-left, spawn, brightnessctl set 5%-\n\
         gesture = top-down, spawn, pgrep -x fuzzel || fuzzel\n\
         gesture = to-top, spawn, pkill -x fuzzel\n",
    );

    assert_eq!(cfg.gestures.len(), 4);
    assert_eq!(cfg.gesture_mask(), 0b1111_0000);
    assert!(matches!(
        &cfg.gestures[0].action,
        Action::Spawn(command) if command == "brightnessctl set +5%"
    ));
    assert!(matches!(
        &cfg.gestures[1].action,
        Action::Spawn(command) if command == "brightnessctl set 5%-"
    ));
    assert!(matches!(
        &cfg.gestures[2].action,
        Action::Spawn(command) if command == "pgrep -x fuzzel || fuzzel"
    ));
    assert!(matches!(
        &cfg.gestures[3].action,
        Action::Spawn(command) if command == "pkill -x fuzzel"
    ));
}

#[test]
fn solo_gesture_and_action_parse() {
    let mut cfg = Config::default();
    cfg.apply_gestures("gesture = double-tap, solo\n");
    assert_eq!(cfg.gestures.len(), 1);
    assert_eq!(cfg.gesture_mask(), 0x1_0000);
    assert!(matches!(cfg.gestures[0].action, Action::ToggleSolo));

    // Both bind-line spellings parse, with no argument required.
    cfg.apply_binds("bind = MOD, S, solo\nbind = MOD SHIFT, S, togglesolo\n");
    let s = key("S");
    let plain = cfg
        .binds
        .iter()
        .find(|b| b.mods == cfg.modifier && b.keysym == s)
        .unwrap();
    assert!(matches!(plain.action, Action::ToggleSolo));
    let shifted = cfg
        .binds
        .iter()
        .find(|b| b.mods == (cfg.modifier | MOD_SHIFT) && b.keysym == s)
        .unwrap();
    assert!(matches!(shifted.action, Action::ToggleSolo));
}

#[test]
fn multi_finger_window_gestures_parse() {
    let mut cfg = Config::default();
    cfg.apply_gestures(
        "gesture = two-up, movewindow, u\n\
         gesture = two-down, movewindow, d\n\
         gesture = two-left, movewindow, l\n\
         gesture = two-right, movewindow, r\n\
         gesture = three-up, close\n\
         gesture = three-down, close\n\
         gesture = three-left, movetoworkspaceprev\n\
         gesture = three-right, movetoworkspacenext\n",
    );

    assert_eq!(cfg.gestures.len(), 8);
    assert_eq!(cfg.gesture_mask(), 0xff00);
    assert!(matches!(
        cfg.gestures[0].action,
        Action::MoveWindow(Direction::Up)
    ));
    assert!(matches!(cfg.gestures[4].action, Action::Close));
    assert!(matches!(
        cfg.gestures[6].action,
        Action::MoveToWorkspacePrevious
    ));
    assert!(matches!(
        cfg.gestures[7].action,
        Action::MoveToWorkspaceNext
    ));
}

#[test]
fn shared_actions_parse_for_keyboard_binds() {
    let mut cfg = Config::default();
    cfg.binds = default_binds(cfg.modifier);
    cfg.apply_binds(
        "bind = MOD, bracketleft, workspaceprev\n\
         bind = MOD, bracketright, workspacenext\n\
         bind = MOD, K, keyboardtoggle\n",
    );
    let action_for = |name| {
        let keysym = key(name);
        &cfg.binds
            .iter()
            .find(|binding| binding.mods == cfg.modifier && binding.keysym == keysym)
            .unwrap()
            .action
    };
    assert!(matches!(
        action_for("bracketleft"),
        Action::WorkspacePrevious
    ));
    assert!(matches!(action_for("bracketright"), Action::WorkspaceNext));
    assert!(matches!(action_for("K"), Action::KeyboardToggle));
}

#[test]
fn float_size_parses_with_and_without_percent() {
    let mut cfg = Config::default();
    assert_eq!(cfg.float_size, (60, 60), "default must be 60% x 60%");

    cfg.parse_scalars("float_size = 55 x 70%\n");
    assert_eq!(cfg.float_size, (55, 70));

    cfg.parse_scalars("float_size = 80%x40%\n");
    assert_eq!(cfg.float_size, (80, 40));

    // Out-of-range or malformed values warn and leave the setting alone.
    cfg.parse_scalars("float_size = 0 x 60\n");
    cfg.parse_scalars("float_size = 60 x 120\n");
    cfg.parse_scalars("float_size = huge\n");
    assert_eq!(cfg.float_size, (80, 40));
}

#[test]
fn togglefloating_action_parses_and_has_default_bind() {
    let mut cfg = Config::default();
    cfg.binds = default_binds(cfg.modifier);

    // Default: Mod+V toggles floating.
    let v = key("V");
    let default = cfg
        .binds
        .iter()
        .find(|b| b.mods == cfg.modifier && b.keysym == v)
        .unwrap();
    assert!(matches!(default.action, Action::ToggleFloating));

    // Both config spellings parse, with no argument required.
    cfg.apply_binds("bind = MOD SHIFT, V, togglefloating\n");
    let msv = cfg
        .binds
        .iter()
        .find(|b| b.mods == (cfg.modifier | MOD_SHIFT) && b.keysym == v)
        .unwrap();
    assert!(matches!(msv.action, Action::ToggleFloating));
}

#[test]
fn monitor_line_parses_position_and_default_scale() {
    let mut cfg = Config::default();
    cfg.parse_scalars("monitor = HDMI-A-1, 0x-1080\n");
    let m = cfg.monitors.iter().find(|m| m.name == "HDMI-A-1").unwrap();
    assert_eq!((m.x, m.y), (0, -1080));
    assert_eq!(m.scale, 1.0);

    cfg.parse_scalars("monitor = eDP-1, 0x0, 1.5\n");
    let m = cfg.monitors.iter().find(|m| m.name == "eDP-1").unwrap();
    assert_eq!((m.x, m.y), (0, 0));
    assert_eq!(m.scale, 1.5);
}

#[test]
fn monitor_line_overrides_same_name_instead_of_duplicating() {
    let mut cfg = Config::default();
    cfg.parse_scalars("monitor = HDMI-A-1, 0x0, 1.0\nmonitor = HDMI-A-1, 1920x0, 2.0\n");
    assert_eq!(cfg.monitors.len(), 1);
    let m = &cfg.monitors[0];
    assert_eq!((m.x, m.y), (1920, 0));
    assert_eq!(m.scale, 2.0);
}
