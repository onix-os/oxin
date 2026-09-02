//! Phase 0 spike: prove the assumptions the whole Lua design rests on.
//!
//! The load-bearing one is the last test in this file. If a runaway config
//! cannot be stopped, none of the rest of the design is worth building.

use std::time::{Duration, Instant};

use luna::{Callback, CallbackReturn, Closure, Executor, Lua, Table};

use std::path::Path;

use super::drive::{drive, Outcome};
use super::load::start_chunk;
use super::park::{park, with_parked, ParkError};

/// Stand-in for `Server`. The real one needs a live wlroots session, and none
/// of what we are proving here depends on what the host struct actually is.
struct FakeServer {
    gap: i32,
    spawned: Vec<String>,
}

/// Load `source` as a named chunk and hand back a started executor.
fn start(vm: &mut Lua, name: &str, source: &str) -> luna::StashedExecutor {
    vm.try_enter(|ctx| {
        let closure = Closure::load(ctx, Some(name), source.as_bytes())?;
        Ok(ctx.stash(Executor::start(ctx, closure.into(), ())))
    })
    .expect("chunk should compile")
}

// ---------------------------------------------------------------------------
// The assumption everything else rests on
// ---------------------------------------------------------------------------

#[test]
fn a_runaway_loop_is_killed_rather_than_hanging_the_compositor() {
    let mut vm = Lua::core();
    let ex = start(&mut vm, "runaway", "while true do end");

    let began = Instant::now();
    let outcome = drive(&mut vm, &ex, 2_000_000, Duration::from_millis(50));
    let took = began.elapsed();

    assert!(
        matches!(outcome, Outcome::Killed(_)),
        "expected the drive to stop it, got {outcome:?}",
    );
    // The deadline is 50ms; allow generous slack for a loaded CI machine while
    // still failing loudly if we actually hung.
    assert!(
        took < Duration::from_secs(2),
        "runaway loop took {took:?} to stop",
    );
}

#[test]
fn a_killed_executor_can_be_reused_for_the_next_keypress() {
    let mut vm = Lua::core();
    let ex = start(&mut vm, "runaway", "while true do end");
    assert!(matches!(
        drive(&mut vm, &ex, 100_000, Duration::from_millis(50)),
        Outcome::Killed(_)
    ));

    // `stop` reset the executor, so the same one takes the next handler.
    vm.try_enter(|ctx| {
        let closure = Closure::load(ctx, Some("after"), b"return 1 + 1")?;
        ctx.fetch(&ex).restart(ctx, closure.into(), ());
        Ok(())
    })
    .expect("restart after a kill should work");

    assert!(matches!(
        drive(&mut vm, &ex, 1_000_000, Duration::from_secs(1)),
        Outcome::Finished
    ));
    let got: i64 = vm
        .try_enter(|ctx| ctx.fetch(&ex).take_result::<i64>(ctx)?)
        .expect("result");
    assert_eq!(got, 2);
}

#[test]
fn fuel_exhaustion_alone_stops_a_loop_that_beats_the_clock() {
    // A tiny fuel budget with a long deadline: proves the fuel arm works on
    // its own, independent of wall-clock timing.
    let mut vm = Lua::core();
    let ex = start(&mut vm, "runaway", "while true do end");
    let outcome = drive(&mut vm, &ex, 50_000, Duration::from_secs(30));
    match outcome {
        Outcome::Killed(why) => assert_eq!(why, "fuel budget exhausted"),
        other => panic!("expected a fuel kill, got {other:?}"),
    }
}

// ---------------------------------------------------------------------------
// Reaching the host from a callback
// ---------------------------------------------------------------------------

#[test]
fn a_callback_reads_and_mutates_the_parked_host() {
    let mut vm = Lua::core();
    let mut server = FakeServer {
        gap: 4,
        spawned: Vec::new(),
    };

    vm.enter(|ctx| {
        let oxin = Table::new(&ctx);
        oxin.set_field(
            ctx,
            "gap",
            Callback::from_fn(&ctx, |ctx, _, mut stack| {
                let got = with_parked::<FakeServer, _>(|s| s.gap).map_err(|e| {
                    luna::Error::from_value(ctx.intern(e.message().as_bytes()).into())
                })?;
                stack.replace(ctx, i64::from(got));
                Ok(CallbackReturn::Return)
            }),
        );
        oxin.set_field(
            ctx,
            "spawn",
            Callback::from_fn(&ctx, |ctx, _, mut stack| {
                let cmd: luna::String = stack.consume(ctx)?;
                let cmd = String::from_utf8_lossy(cmd.as_bytes()).into_owned();
                with_parked::<FakeServer, _>(move |s| s.spawned.push(cmd)).map_err(|e| {
                    luna::Error::from_value(ctx.intern(e.message().as_bytes()).into())
                })?;
                Ok(CallbackReturn::Return)
            }),
        );
        ctx.set_global("oxin", oxin);
    });

    let ex = start(
        &mut vm,
        "init.lua",
        r#"
            oxin.spawn("kitty")
            oxin.spawn("gap is " .. oxin.gap())
        "#,
    );

    // SAFETY: `server` is a live local, uniquely borrowed for the drive, and
    // nothing below touches it through another reference.
    let outcome = unsafe {
        park(&mut server as *mut FakeServer, || {
            drive(&mut vm, &ex, 1_000_000, Duration::from_secs(5))
        })
    };
    assert!(matches!(outcome, Outcome::Finished), "{outcome:?}");
    assert_eq!(server.spawned, vec!["kitty", "gap is 4"]);
}

#[test]
fn a_host_call_outside_a_drive_is_an_error_not_a_crash() {
    assert_eq!(
        with_parked::<FakeServer, _>(|s| s.gap).unwrap_err(),
        ParkError::NotParked,
    );
}

#[test]
fn a_nested_host_borrow_is_refused() {
    let mut server = FakeServer {
        gap: 4,
        spawned: Vec::new(),
    };
    // SAFETY: `server` is live and uniquely borrowed for the closure.
    let inner = unsafe {
        park(&mut server as *mut FakeServer, || {
            with_parked::<FakeServer, _>(|_outer| {
                // Simulates a host fn that re-enters another host fn while it
                // still holds the `&mut`. Must be refused, not aliased.
                with_parked::<FakeServer, _>(|_nested| ()).unwrap_err()
            })
            .unwrap()
        })
    };
    assert_eq!(inner, ParkError::Reentered);
}

#[test]
fn parking_is_restored_after_the_drive_ends() {
    let mut server = FakeServer {
        gap: 1,
        spawned: Vec::new(),
    };
    // SAFETY: as above.
    unsafe { park(&mut server as *mut FakeServer, || {}) };
    assert_eq!(
        with_parked::<FakeServer, _>(|s| s.gap).unwrap_err(),
        ParkError::NotParked,
        "the pointer must not stay parked after the drive returns",
    );
}

// ---------------------------------------------------------------------------
// Load-time error reporting
// ---------------------------------------------------------------------------

#[test]
fn a_named_chunk_reports_file_and_line() {
    let mut vm = Lua::core();
    let err = vm
        .try_enter(|ctx| {
            let closure = Closure::load(
                ctx,
                Some("init.lua"),
                b"local a = 1\nlocal b = 2\nerror('boom')\n",
            )?;
            Ok(ctx.stash(Executor::start(ctx, closure.into(), ())))
        })
        .and_then(|ex| {
            assert!(matches!(
                drive(&mut vm, &ex, 1_000_000, Duration::from_secs(5)),
                Outcome::Finished
            ));
            // `Value<'gc>` cannot escape `try_enter` — it is invariant in
            // 'gc — so take the result for its error alone.
            vm.try_enter(|ctx| ctx.fetch(&ex).take_result::<()>(ctx)?)
        })
        .expect_err("the chunk raises");

    let text = err.to_string();
    assert!(text.contains("init.lua"), "no chunk name in {text:?}");
    assert!(text.contains(':'), "no line number in {text:?}");
    assert!(text.contains("boom"), "no message in {text:?}");
}

#[test]
fn a_syntax_error_names_the_file_only_because_the_loader_adds_it() {
    // luna attaches the chunk name to runtime errors but not to compile ones:
    // a ParseError stringifies as "parse error at line N: ..." with no file.
    // Raw `Closure::load` therefore loses the filename...
    let mut vm = Lua::core();
    let raw = vm
        .try_enter(|ctx| {
            let closure = Closure::load(ctx, Some("init.lua"), b"oxin.gap = = 4\n")?;
            Ok(ctx.stash(Executor::start(ctx, closure.into(), ())))
        })
        .expect_err("should not compile");
    assert!(
        !raw.to_string().contains("init.lua"),
        "luna started naming the file in compile errors; start_chunk can stop \
         prepending it: {raw}",
    );

    // ...and `start_chunk` is what puts it back.
    let err = start_chunk(
        &mut vm,
        Path::new("/etc/0xin/init.lua"),
        b"oxin.gap = = 4\n",
    )
    .expect_err("should not compile");
    assert!(err.contains("/etc/0xin/init.lua"), "{err}");
    assert!(err.contains("line 1"), "{err}");
}

// ---------------------------------------------------------------------------
// Stdlib shape: what `core()` actually gives a config
// ---------------------------------------------------------------------------

#[test]
fn core_has_no_print_so_we_must_supply_one() {
    // Documents why the module builder installs its own `print`: without it a
    // config's first `print(...)` fails with "attempt to call a nil value".
    let mut vm = Lua::core();
    let missing = vm.enter(|ctx| ctx.get_global_value("print").is_nil());
    assert!(
        missing,
        "core() gained a print; the module builder can stop adding one"
    );
}

#[test]
fn core_has_no_os_or_io() {
    // `os.exit` would kill the compositor mid-session and `os.execute` would
    // freeze it while bypassing reset_signals, so neither may be reachable.
    let mut vm = Lua::core();
    vm.enter(|ctx| {
        assert!(
            ctx.get_global_value("os").is_nil(),
            "os must not be reachable"
        );
        assert!(
            ctx.get_global_value("io").is_nil(),
            "io must not be reachable"
        );
    });
}

#[test]
fn a_module_parked_in_package_loaded_is_returned_by_require() {
    let mut vm = Lua::core();
    vm.load_package();
    vm.try_enter(|ctx| {
        let oxin = Table::new(&ctx);
        oxin.set_field(ctx, "gap", 7);
        let package: Table = ctx.get_global("package")?;
        let loaded: Table = package.get(ctx, "loaded")?;
        loaded.set(ctx, "oxin", oxin)?;
        Ok(())
    })
    .expect("park the module");

    let ex = start(
        &mut vm,
        "init.lua",
        "local oxin = require('oxin')\nreturn oxin.gap\n",
    );
    assert!(matches!(
        drive(&mut vm, &ex, 1_000_000, Duration::from_secs(5)),
        Outcome::Finished
    ));
    let got: i64 = vm
        .try_enter(|ctx| ctx.fetch(&ex).take_result::<i64>(ctx)?)
        .expect("require should find the parked module without touching disk");
    assert_eq!(got, 7);
}

// ---------------------------------------------------------------------------
// Settings: assignment, the typo guard, and all-or-nothing
// ---------------------------------------------------------------------------

mod settings {
    use std::path::Path;

    use crate::config::{Config, MOD_ALT};
    use crate::lua::{config, vm};

    fn load(source: &str) -> Result<Config, String> {
        let mut lua = vm::build();
        config::load_all(
            &mut lua,
            Some((Path::new("init.lua"), source.as_bytes().to_vec())),
            &[],
        )
        .map(|f| f.config)
    }

    #[test]
    fn settings_are_assigned_onto_the_module() {
        let cfg = load(
            r#"
                local oxin = require("oxin")
                oxin.modifier       = "alt"
                oxin.gap            = 7
                oxin.first_split    = "horizontal"
                oxin.background     = { 0.04, 0.04, 0.06 }
                oxin.wallpaper      = "~/pics/286257.jpg"
                oxin.window_opacity = 0.95
                oxin.corner_radius  = 40
                oxin.float_size     = { 70, 55 }
                oxin.gesture_handle = "hidden"
                oxin.keyboard = {
                  show   = "pkill -USR2 -x patin-osk",
                  hide   = "pkill -USR1 -x patin-osk",
                  height = 262,
                }
            "#,
        )
        .expect("config should load");

        assert_eq!(cfg.modifier, MOD_ALT);
        assert_eq!(cfg.gap, 7);
        assert!(!cfg.first_split_vertical);
        assert_eq!(cfg.background, (0.04, 0.04, 0.06));
        assert_eq!(cfg.wallpaper.as_deref(), Some("~/pics/286257.jpg"));
        assert_eq!(cfg.window_opacity, 0.95);
        assert_eq!(cfg.corner_radius, 40);
        assert_eq!(cfg.float_size, (70, 55));
        assert!(!cfg.gesture_handle_visible);
        assert_eq!(
            cfg.virtual_keyboard_show.as_deref(),
            Some("pkill -USR2 -x patin-osk")
        );
        assert_eq!(cfg.virtual_keyboard_height, 262);
    }

    #[test]
    fn a_setting_the_config_never_mentions_keeps_its_default() {
        let defaults = Config::default();
        let cfg = load("oxin.gap = 9").expect("loads");
        assert_eq!(cfg.gap, 9);
        assert_eq!(cfg.modifier, defaults.modifier);
        assert_eq!(cfg.background, defaults.background);
        assert_eq!(cfg.corner_radius, defaults.corner_radius);
    }

    #[test]
    fn a_typo_is_refused_and_names_the_setting_it_meant() {
        let err = load("oxin.gpa = 4").expect_err("a typo must not be silently ignored");
        assert!(err.contains("gpa"), "{err}");
        assert!(err.contains("gap"), "should suggest the near miss: {err}");
    }

    #[test]
    fn an_unrecognisable_name_lists_the_settings() {
        let err = load("oxin.frobnicate = 1").expect_err("unknown setting");
        assert!(err.contains("corner_radius"), "should list settings: {err}");
    }

    #[test]
    fn a_typo_reports_the_line_it_is_on() {
        let err = load("local a = 1\nlocal b = 2\noxin.gpa = 4\n").expect_err("a typo");
        assert!(err.contains("init.lua"), "{err}");
        assert!(err.contains(":3"), "should point at line 3: {err}");
    }

    #[test]
    fn a_wrong_type_says_what_it_wanted() {
        let err = load("oxin.gap = 'wide'").expect_err("gap is a number");
        assert!(err.contains("whole number"), "{err}");
    }

    #[test]
    fn out_of_range_values_are_refused() {
        assert!(load("oxin.window_opacity = 1.5").is_err());
        assert!(load("oxin.corner_radius = 9000").is_err());
        assert!(load("oxin.gap = -1").is_err());
        assert!(load("oxin.float_size = { 0, 60 }").is_err());
    }

    #[test]
    fn reassignment_is_guarded_too() {
        // `set_intercept_all_writes` is what makes this work: stock Lua fires
        // __newindex only for absent keys, so the second write would slip past.
        let err = load("oxin.gap = 4\noxin.gap = 'wide'\n").expect_err("second write is checked");
        assert!(err.contains("whole number"), "{err}");
    }

    #[test]
    fn a_setting_can_be_read_back_before_it_is_written() {
        let cfg = load("oxin.gap = oxin.gap + 10").expect("reads the default then adds");
        assert_eq!(cfg.gap, Config::default().gap + 10);
    }

    #[test]
    fn a_config_that_raises_applies_nothing_at_all() {
        let err = load("oxin.gap = 40\noxin.corner_radius = 12\nerror('nope')\n")
            .expect_err("the chunk raises");
        assert!(err.contains("nope"), "{err}");
        // The caller falls back to defaults; nothing partial escapes, which is
        // the whole point of staging.
    }

    #[test]
    fn a_config_may_branch_and_loop() {
        // The reason the file returns nothing: it is a list of statements, so
        // it can compute. A returned table cannot.
        let cfg = load(
            r#"
                local total = 0
                for i = 1, 4 do total = total + i end
                oxin.gap = total
                if oxin.gap > 5 then oxin.corner_radius = 20 end
            "#,
        )
        .expect("loads");
        assert_eq!(cfg.gap, 10);
        assert_eq!(cfg.corner_radius, 20);
    }

    #[test]
    fn a_runaway_config_is_stopped_and_reported() {
        let err = load("while true do end").expect_err("must not hang startup");
        assert!(err.contains("did not finish"), "{err}");
    }

    #[test]
    fn the_config_cannot_reach_the_filesystem_or_kill_the_session() {
        // os.exit would take the compositor down mid-session and os.execute
        // would freeze it while bypassing reset_signals, so neither is loaded;
        // dofile/loadfile/load are removed so chunk loading stays the host's.
        for probe in [
            "os.exit(1)",
            "os.execute('true')",
            "io.open('/etc/passwd')",
            "dofile('/etc/passwd')",
            "loadfile('/etc/passwd')",
            "load('return 1')",
        ] {
            let err = load(probe).unwrap_err();
            assert!(
                err.contains("nil value") || err.contains("nil"),
                "{probe} should not be reachable, got: {err}",
            );
        }
    }
}

// ---------------------------------------------------------------------------
// Session startup: what the compositor actually calls
// ---------------------------------------------------------------------------

/// These share one test because they drive process-wide environment
/// variables, which cannot be set independently from parallel tests.
#[test]
fn session_startup_loads_a_config_and_survives_a_broken_one() {
    use crate::config::Config;
    use crate::lua::session;

    let dir = std::env::temp_dir().join(format!("0xin-lua-test-{}", std::process::id()));
    std::fs::create_dir_all(&dir).expect("temp dir");
    let path = dir.join("init.lua");

    // A config that loads.
    std::fs::write(&path, b"oxin.gap = 11\n").unwrap();
    std::env::set_var("OXIN_CONFIG", &path);
    assert_eq!(session::start().config.gap, 11);

    // A config that raises: defaults, and the reason is kept for 0xinctl.
    std::fs::write(&path, b"oxin.gap = 11\noxin.nonsense = 1\n").unwrap();
    let broken = session::start();
    assert_eq!(
        broken.config.gap,
        Config::default().gap,
        "a broken config must not leave the earlier assignments applied",
    );
    let reported = broken
        .registry
        .config_error
        .as_deref()
        .expect("the failure must be recorded, not just printed");
    assert!(reported.contains("nonsense"), "{reported}");

    // A config that is not there at all: defaults, and no error.
    std::fs::remove_file(&path).unwrap();
    let missing = session::start();
    assert_eq!(missing.config.gap, Config::default().gap);
    assert!(missing.registry.config_error.is_none());

    std::env::remove_var("OXIN_CONFIG");
    let _ = std::fs::remove_dir_all(&dir);
}

// ---------------------------------------------------------------------------
// Registrars: keys, hold, gestures, monitors
// ---------------------------------------------------------------------------

mod registrars {
    use std::path::Path;

    use crate::config::names::keysym_from_name;
    use crate::config::{Action, Config, Direction, GestureTrigger, MOD_ALT, MOD_LOGO, MOD_SHIFT};
    use crate::lua::{config, vm};

    fn load(source: &str) -> Result<Config, String> {
        let mut lua = vm::build();
        config::load_all(
            &mut lua,
            Some((Path::new("init.lua"), source.as_bytes().to_vec())),
            &[],
        )
        .map(|f| f.config)
    }

    fn key(name: &str) -> u32 {
        keysym_from_name(name).expect("xkb should know this key")
    }

    fn bind_for<'a>(cfg: &'a Config, mods: u32, name: &str) -> Option<&'a Action> {
        let sym = key(name);
        cfg.binds
            .iter()
            .find(|b| b.mods == mods && b.keysym == sym)
            .map(|b| &b.action)
    }

    #[test]
    fn a_chord_binds_a_built_in_action() {
        let cfg = load(r#"oxin.keys["MOD+Return"] = oxin.action.spawn("kitty")"#).unwrap();
        assert_eq!(
            bind_for(&cfg, MOD_LOGO, "Return"),
            Some(&Action::Spawn("kitty".into())),
        );
    }

    #[test]
    fn modifier_order_does_not_matter() {
        // If the two spellings landed on different entries the table would not
        // really be keyed, and one would silently shadow the other.
        let a = load(r#"oxin.keys["MOD+SHIFT+q"] = oxin.action.quit"#).unwrap();
        let b = load(r#"oxin.keys["SHIFT+MOD+q"] = oxin.action.quit"#).unwrap();
        let mods = MOD_LOGO | MOD_SHIFT;
        assert_eq!(bind_for(&a, mods, "q"), Some(&Action::Quit));
        assert_eq!(bind_for(&a, mods, "q"), bind_for(&b, mods, "q"));
    }

    #[test]
    fn binding_the_same_chord_twice_replaces_rather_than_duplicates() {
        let cfg = load(
            r#"
                oxin.keys["MOD+t"] = oxin.action.spawn("one")
                oxin.keys["MOD+t"] = oxin.action.spawn("two")
            "#,
        )
        .unwrap();
        let sym = key("t");
        let hits = cfg
            .binds
            .iter()
            .filter(|b| b.mods == MOD_LOGO && b.keysym == sym)
            .count();
        assert_eq!(hits, 1, "a keyed registrar must replace, not append");
        assert_eq!(
            bind_for(&cfg, MOD_LOGO, "t"),
            Some(&Action::Spawn("two".into())),
        );
    }

    #[test]
    fn defaults_fill_in_around_the_config() {
        // docs/design.md: user config merges, never replaces. A one-line config
        // still has the whole built-in keymap.
        let cfg = load(r#"oxin.keys["MOD+t"] = oxin.action.spawn("x")"#).unwrap();
        assert!(
            cfg.binds.len() > 20,
            "expected the built-in keymap alongside the one override, got {}",
            cfg.binds.len(),
        );
        assert_eq!(
            bind_for(&cfg, MOD_LOGO, "Return"),
            Some(&Action::Spawn("kitty".into()))
        );
    }

    #[test]
    fn nil_unbinds_a_default_and_it_stays_unbound() {
        // The .conf format had no way to say this at all.
        let with = load("").unwrap();
        assert!(bind_for(&with, MOD_LOGO, "Return").is_some());

        let without = load(r#"oxin.keys["MOD+Return"] = nil"#).unwrap();
        assert!(
            bind_for(&without, MOD_LOGO, "Return").is_none(),
            "an unbound default must not be restored when the built-ins are filled in",
        );
    }

    #[test]
    fn a_chord_with_no_modifier_works() {
        let cfg = load(r#"oxin.keys["XF86AudioRaiseVolume"] = oxin.action.spawn("up")"#).unwrap();
        assert_eq!(
            bind_for(&cfg, 0, "XF86AudioRaiseVolume"),
            Some(&Action::Spawn("up".into()))
        );
    }

    #[test]
    fn a_lua_function_can_be_bound() {
        let mut lua = vm::build();
        let finished = config::load_all(
            &mut lua,
            Some((
                Path::new("init.lua"),
                br#"oxin.keys["MOD+g"] = function() end"#.to_vec(),
            )),
            &[],
        )
        .unwrap();
        assert_eq!(
            bind_for(&finished.config, MOD_LOGO, "g"),
            Some(&Action::Lua(0))
        );
        assert_eq!(
            finished.functions.len(),
            1,
            "the function must be kept for dispatch",
        );
    }

    #[test]
    fn an_unknown_key_name_is_refused() {
        let err = load(r#"oxin.keys["MOD+Retrun"] = oxin.action.quit"#).unwrap_err();
        assert!(err.contains("Retrun"), "{err}");
        assert!(err.contains("xkb"), "{err}");
    }

    #[test]
    fn a_mistyped_action_is_refused_rather_than_silently_unbinding() {
        // The whole reason oxin.action raises on __index: `nil` is how a config
        // *removes* a binding, so a typo would quietly delete one.
        let err = load(r#"oxin.keys["MOD+q"] = oxin.action.clsoe"#).unwrap_err();
        assert!(err.contains("clsoe"), "{err}");
    }

    #[test]
    fn the_modifier_must_be_set_before_any_mod_chord() {
        let err = load(
            r#"
                oxin.keys["MOD+t"] = oxin.action.quit
                oxin.modifier = "alt"
            "#,
        )
        .unwrap_err();
        assert!(err.contains("before any binding"), "{err}");

        // The other order is fine, and the built-ins follow the new modifier.
        let cfg = load(
            r#"
                oxin.modifier = "alt"
                oxin.keys["MOD+t"] = oxin.action.quit
            "#,
        )
        .unwrap();
        assert_eq!(bind_for(&cfg, MOD_ALT, "t"), Some(&Action::Quit));
        assert_eq!(
            bind_for(&cfg, MOD_ALT, "Return"),
            Some(&Action::Spawn("kitty".into()))
        );
    }

    #[test]
    fn directional_and_workspace_actions_carry_their_argument() {
        let cfg = load(
            r#"
                oxin.keys["MOD+h"] = oxin.action.focus("left")
                oxin.keys["MOD+j"] = oxin.action.move("down")
                oxin.keys["MOD+k"] = oxin.action.resize("up")
                oxin.keys["MOD+3"] = oxin.action.workspace(3)
            "#,
        )
        .unwrap();
        assert_eq!(
            bind_for(&cfg, MOD_LOGO, "h"),
            Some(&Action::MoveFocus(Direction::Left))
        );
        assert_eq!(
            bind_for(&cfg, MOD_LOGO, "j"),
            Some(&Action::MoveWindow(Direction::Down))
        );
        assert_eq!(
            bind_for(&cfg, MOD_LOGO, "k"),
            Some(&Action::ResizeWindow(Direction::Up))
        );
        // Workspaces are 1-based in a config and 0-based in the enum.
        assert_eq!(bind_for(&cfg, MOD_LOGO, "3"), Some(&Action::Workspace(2)));
    }

    #[test]
    fn workspace_numbers_are_range_checked() {
        assert!(load("oxin.keys['MOD+z'] = oxin.action.workspace(0)").is_err());
        assert!(load("oxin.keys['MOD+z'] = oxin.action.workspace(99)").is_err());
    }

    #[test]
    fn hold_binds_take_a_duration() {
        let cfg = load(r#"oxin.hold["XF86PowerOff"] = { ms = 2000, action = oxin.action.quit }"#)
            .unwrap();
        assert_eq!(cfg.hold_binds.len(), 1);
        let h = &cfg.hold_binds[0];
        assert_eq!(h.duration_ms, 2000);
        assert_eq!(h.action, Action::Quit);
        assert_eq!(h.keysym, key("XF86PowerOff"));
    }

    #[test]
    fn a_chord_can_have_both_a_press_and_a_hold() {
        // keys and hold are separate tables, so the same chord in both is not
        // a conflict — it is a short press and a long press.
        let cfg = load(
            r#"
                oxin.keys["XF86PowerOff"] = oxin.action.spawn("lock")
                oxin.hold["XF86PowerOff"] = { ms = 2000, action = oxin.action.spawn("menu") }
            "#,
        )
        .unwrap();
        assert_eq!(
            bind_for(&cfg, 0, "XF86PowerOff"),
            Some(&Action::Spawn("lock".into()))
        );
        assert_eq!(cfg.hold_binds.len(), 1);
    }

    #[test]
    fn gestures_bind_by_trigger_name() {
        let cfg = load(
            r#"
                oxin.gestures["bottom-up"]   = oxin.action.keyboard_show
                oxin.gestures["three-left"]  = oxin.action.move_to_workspace_prev
                oxin.gestures["double-tap"]  = oxin.action.solo
            "#,
        )
        .unwrap();
        assert_eq!(cfg.gestures.len(), 3);
        let find = |t| {
            cfg.gestures
                .iter()
                .find(|g| g.trigger == t)
                .map(|g| &g.action)
        };
        assert_eq!(find(GestureTrigger::BottomUp), Some(&Action::KeyboardShow));
        assert_eq!(
            find(GestureTrigger::ThreeLeft),
            Some(&Action::MoveToWorkspacePrevious)
        );
        assert_eq!(find(GestureTrigger::DoubleTap), Some(&Action::ToggleSolo));
    }

    #[test]
    fn an_unknown_gesture_is_refused() {
        let err = load(r#"oxin.gestures["sideways-wiggle"] = oxin.action.quit"#).unwrap_err();
        assert!(err.contains("sideways-wiggle"), "{err}");
    }

    #[test]
    fn monitors_are_keyed_by_connector_name() {
        let cfg = load(
            r#"
                oxin.monitors["DP-1"]  = { x = 0, y = 0, scale = 1.0 }
                oxin.monitors["DP-2"]  = { x = 1920, y = 0 }
                oxin.monitors["DP-1"]  = { x = 0, y = -1080, scale = 2.0 }
            "#,
        )
        .unwrap();
        assert_eq!(
            cfg.monitors.len(),
            2,
            "the repeat must replace, not duplicate"
        );
        let dp1 = cfg.monitors.iter().find(|m| m.name == "DP-1").unwrap();
        assert_eq!((dp1.x, dp1.y, dp1.scale), (0, -1080, 2.0));
        // Omitted members take sensible defaults rather than failing.
        let dp2 = cfg.monitors.iter().find(|m| m.name == "DP-2").unwrap();
        assert_eq!((dp2.x, dp2.y, dp2.scale), (1920, 0, 1.0));
    }

    #[test]
    fn a_registrar_cannot_be_assigned_over() {
        let err = load("oxin.keys = {}").unwrap_err();
        assert!(err.contains("index"), "{err}");
    }

    #[test]
    fn a_config_can_loop_over_the_workspaces() {
        // This is what the .conf format could never do: nine bindings from two
        // lines, because the config is statements rather than a returned table.
        let cfg = load(
            r#"
                for i = 1, 9 do
                  oxin.keys["MOD+" .. i]       = oxin.action.workspace(i)
                  oxin.keys["MOD+SHIFT+" .. i] = oxin.action.move_to_workspace(i)
                end
            "#,
        )
        .unwrap();
        for i in 1..=9 {
            let name = i.to_string();
            assert_eq!(
                bind_for(&cfg, MOD_LOGO, &name),
                Some(&Action::Workspace(i - 1)),
                "workspace {i}",
            );
            assert_eq!(
                bind_for(&cfg, MOD_LOGO | MOD_SHIFT, &name),
                Some(&Action::MoveToWorkspace(i - 1)),
                "move to workspace {i}",
            );
        }
    }
}

#[test]
fn the_module_exposes_its_functions_and_namespaces() {
    // A missing entry here shows up in a config as "could not call a nil
    // value", which says nothing about which name was wrong.
    use crate::lua::{config, vm};
    let mut lua = vm::build();
    for name in [
        "keys",
        "hold",
        "gestures",
        "monitors",
        "action",
        "on",
        "spawn",
        "workspace",
        "workspaces",
    ] {
        let src = format!("if oxin.{name} == nil then error('oxin.{name} is missing') end");
        config::load_all(
            &mut lua,
            Some((std::path::Path::new("probe.lua"), src.as_bytes().to_vec())),
            &[],
        )
        .unwrap_or_else(|e| panic!("{e}"));
    }
}

#[test]
fn the_shipped_example_config_loads() {
    // A broken example is worse than none: it is the first thing anyone copies.
    use crate::lua::{config, vm};
    let source = include_bytes!("../../init.lua.example");
    let mut lua = vm::build();
    let finished = config::load_all(
        &mut lua,
        Some((std::path::Path::new("init.lua.example"), source.to_vec())),
        &[],
    )
    .unwrap_or_else(|e| panic!("the shipped example must load: {e}"));
    // It writes out the default keymap explicitly, so the result should match
    // the built-ins rather than adding to them.
    assert!(
        finished.config.binds.len() >= 30,
        "{}",
        finished.config.binds.len()
    );
}

// ---------------------------------------------------------------------------
// Plugins
// ---------------------------------------------------------------------------

mod plugins {
    use std::fs;
    use std::path::{Path, PathBuf};

    use crate::config::names::keysym_from_name;
    use crate::config::{Action, MOD_LOGO};
    use crate::lua::{config, plugins, vm};

    /// Build a runtimepath root on disk and return it.
    fn root(name: &str, files: &[(&str, &str)]) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("0xin-plug-{}-{name}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        for (rel, body) in files {
            let path = dir.join(rel);
            fs::create_dir_all(path.parent().unwrap()).unwrap();
            fs::write(path, body).unwrap();
        }
        dir
    }

    fn bind(cfg: &crate::config::Config, name: &str) -> Option<Action> {
        let sym = keysym_from_name(name)?;
        cfg.binds
            .iter()
            .find(|b| b.mods == MOD_LOGO && b.keysym == sym)
            .map(|b| b.action.clone())
    }

    #[test]
    fn plugin_files_are_ordered_alphabetically_with_after_last() {
        let dir = root(
            "order",
            &[
                ("plugin/b.lua", ""),
                ("plugin/a.lua", ""),
                ("plugin/nested/z.lua", ""),
                ("after/plugin/late.lua", ""),
                ("pack/vendor/start/demo/plugin/p.lua", ""),
                ("lua/helper.lua", "return {}"),
            ],
        );
        let files = plugins::plugin_files(std::slice::from_ref(&dir));
        let names: Vec<String> = files
            .iter()
            .map(|p| p.strip_prefix(&dir).unwrap().to_string_lossy().into_owned())
            .collect();
        assert_eq!(
            names,
            vec![
                // Packages join the path before the root's own plugin dir.
                "pack/vendor/start/demo/plugin/p.lua",
                // Alphabetical within a directory, files before subdirectories.
                "plugin/a.lua",
                "plugin/b.lua",
                "plugin/nested/z.lua",
                // `after` is always last.
                "after/plugin/late.lua",
            ],
        );
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn lua_directories_join_the_require_path_but_do_not_auto_run() {
        let dir = root(
            "luadir",
            &[("lua/helper.lua", "return {}"), ("plugin/p.lua", "")],
        );
        let path = plugins::lua_path(std::slice::from_ref(&dir));
        assert!(path.contains("lua/?.lua"), "{path}");
        assert!(path.contains("lua/?/init.lua"), "{path}");

        // The split that matters: `plugin/` runs, `lua/` only answers require.
        let files = plugins::plugin_files(std::slice::from_ref(&dir));
        assert!(
            files.iter().all(|f| !f.to_string_lossy().contains("/lua/")),
            "a lua/ file must never be auto-run: {files:?}",
        );
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_plugin_registers_and_after_overrides_it() {
        let dir = root(
            "override",
            &[
                (
                    "plugin/a.lua",
                    r#"oxin.keys["MOD+t"] = oxin.action.spawn("plugin")"#,
                ),
                (
                    "after/plugin/z.lua",
                    r#"oxin.keys["MOD+t"] = oxin.action.spawn("after")"#,
                ),
            ],
        );
        let files = plugins::plugin_files(std::slice::from_ref(&dir));
        let mut lua = vm::build();
        let finished = config::load_all(
            &mut lua,
            Some((
                Path::new("init.lua"),
                br#"oxin.keys["MOD+t"] = oxin.action.spawn("init")"#.to_vec(),
            )),
            &files,
        )
        .unwrap();

        // neovim's order: init.lua, then plugin/, then after/. So `after` wins,
        // which is exactly why it is the seam for overriding a plugin.
        assert_eq!(
            bind(&finished.config, "t"),
            Some(Action::Spawn("after".into())),
        );
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn one_broken_plugin_does_not_stop_the_others() {
        // Deliberately unlike init.lua, where a raise is fatal to the load: a
        // plugin failing is one of several, and taking the compositor's whole
        // config down with it is worse than doing without it.
        let dir = root(
            "broken",
            &[
                ("plugin/a.lua", "oxin.gap = 11"),
                ("plugin/b.lua", "error('broken on purpose')"),
                ("plugin/c.lua", "oxin.corner_radius = 5"),
            ],
        );
        let files = plugins::plugin_files(std::slice::from_ref(&dir));
        let mut lua = vm::build();
        let finished = config::load_all(&mut lua, None, &files).unwrap();
        assert_eq!(finished.config.gap, 11, "the plugin before the failure");
        assert_eq!(finished.config.corner_radius, 5, "the one after it");
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_missing_root_is_not_an_error() {
        let missing = PathBuf::from("/nonexistent/0xin");
        assert!(plugins::plugin_files(std::slice::from_ref(&missing)).is_empty());
        assert!(plugins::lua_path(std::slice::from_ref(&missing)).is_empty());
    }

    #[test]
    fn the_runtimepath_is_a_list_not_a_single_directory() {
        // A path with one element is still a path; growing one later would be
        // a config break, so the shape is here from the start.
        let roots = plugins::runtimepath();
        assert!(roots.len() >= 2, "{roots:?}");
        assert!(roots[0].starts_with("/etc"), "system root first: {roots:?}");
    }
}
