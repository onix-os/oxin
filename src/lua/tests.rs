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
        config::load(&mut lua, Path::new("init.lua"), source.as_bytes())
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
// The conversion fixture
// ---------------------------------------------------------------------------

/// `profiles/fp5/config/0xin/0xin.conf` is the most complete config in the
/// tree, so it is what the Lua path has to reproduce. Until the registrars
/// land this covers the settings half; the binds, gestures and autostarts it
/// also exercises follow in the next increment.
mod fixture {
    use std::path::Path;

    use crate::config::Config;
    use crate::lua::{config, vm};

    /// The settings from the fp5 profile, written the way the Lua config
    /// expresses them.
    const FP5_LUA: &str = r#"
        local oxin = require("oxin")

        oxin.modifier       = "super"
        oxin.gap            = 5
        oxin.first_split    = "horizontal"
        oxin.background     = { 0.04, 0.04, 0.06 }
        oxin.wallpaper      = "~/pics/286257.jpg"
        oxin.window_opacity = 0.95
        oxin.corner_radius  = 40
        oxin.gesture_handle = "hidden"

        oxin.keyboard = {
          show   = "pkill -USR2 -x patin-osk",
          hide   = "pkill -USR1 -x patin-osk",
          height = 262,
        }
    "#;

    #[test]
    fn the_lua_path_reproduces_the_fp5_settings() {
        let mut lua = vm::build();
        let got = config::load(&mut lua, Path::new("init.lua"), FP5_LUA.as_bytes())
            .expect("the fp5 settings should load");

        // Expected values read straight off profiles/fp5/config/0xin/0xin.conf.
        let want = Config {
            modifier: crate::config::MOD_LOGO,
            gap: 5,
            first_split_vertical: false,
            background: (0.04, 0.04, 0.06),
            wallpaper: Some("~/pics/286257.jpg".into()),
            window_opacity: 0.95,
            corner_radius: 40,
            gesture_handle_visible: false,
            virtual_keyboard_show: Some("pkill -USR2 -x patin-osk".into()),
            virtual_keyboard_hide: Some("pkill -USR1 -x patin-osk".into()),
            virtual_keyboard_height: 262,
            ..Config::default()
        };

        assert_eq!(got.modifier, want.modifier);
        assert_eq!(got.gap, want.gap);
        assert_eq!(got.first_split_vertical, want.first_split_vertical);
        assert_eq!(got.background, want.background);
        assert_eq!(got.wallpaper, want.wallpaper);
        assert_eq!(got.window_opacity, want.window_opacity);
        assert_eq!(got.corner_radius, want.corner_radius);
        assert_eq!(got.gesture_handle_visible, want.gesture_handle_visible);
        assert_eq!(got.virtual_keyboard_show, want.virtual_keyboard_show);
        assert_eq!(got.virtual_keyboard_hide, want.virtual_keyboard_hide);
        assert_eq!(got.virtual_keyboard_height, want.virtual_keyboard_height);
    }

    #[test]
    fn the_checked_in_fp5_conf_still_says_what_this_fixture_claims() {
        // Guards against the fixture drifting from the profile it mirrors: if
        // someone edits the .conf, this fails rather than the comparison above
        // quietly testing nothing.
        let conf = include_str!("../../profiles/fp5/config/0xin/0xin.conf");
        for expected in [
            "gap = 5",
            "first_split = horizontal",
            "background = 0.04 0.04 0.06",
            "wallpaper = ~/pics/286257.jpg",
            "window_opacity = 0.95",
            "corner_radius = 40",
            "gesture_handle = hidden",
            "virtual_keyboard_height = 262",
        ] {
            assert!(
                conf.lines().any(|l| l.trim() == expected),
                "fp5 profile no longer contains {expected:?}; update FP5_LUA to match",
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
    std::env::set_var("OXIN_LUA_CONFIG", &path);
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

    std::env::remove_var("OXIN_LUA_CONFIG");
    let _ = std::fs::remove_dir_all(&dir);
}
