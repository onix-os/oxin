//! Build script: the FFI pipeline for 0xin.
//!
//! 1. Locate wlroots 0.19 + wayland-server via pkg-config (emits linker flags).
//! 2. Generate the xdg-shell protocol server header with wayland-scanner —
//!    wlroots' xdg headers #include it, and it isn't shipped as a system header.
//! 3. Compile our C shim against those headers.
//! 4. Run bindgen over wrapper.h for the functions Rust calls directly.

use std::env;
use std::path::PathBuf;
use std::process::Command;

fn main() {
    let out_dir = PathBuf::from(env::var("OUT_DIR").unwrap());

    // 1. Find the libraries. `.probe()` returns include paths AND prints the
    //    cargo directives that link `-lwlroots-0.19` / `-lwayland-server`.
    let wlroots = pkg_config::Config::new()
        .probe("wlroots-0.19")
        .expect("wlroots-0.19 not found via pkg-config");
    let wayland = pkg_config::Config::new()
        .probe("wayland-server")
        .expect("wayland-server not found via pkg-config");
    // Our shim calls xkbcommon directly (keymap compilation), so link it.
    let xkbcommon = pkg_config::Config::new()
        .probe("xkbcommon")
        .expect("xkbcommon not found via pkg-config");
    // The corner-radius mask pass drives GLES2/EGL directly (wlroots' scene
    // API has no shader-injection point), so we need both libraries too.
    let glesv2 = pkg_config::Config::new()
        .probe("glesv2")
        .expect("glesv2 not found via pkg-config");
    let egl = pkg_config::Config::new()
        .probe("egl")
        .expect("egl not found via pkg-config");

    // 2. Generate xdg-shell-protocol.h into OUT_DIR. wlroots' xdg-shell header
    //    #includes it; wayland-scanner produces it from the protocol XML.
    let protocols_dir = pkg_config::get_variable("wayland-protocols", "pkgdatadir")
        .expect("wayland-protocols pkgdatadir not found");
    let xdg_xml = format!("{protocols_dir}/stable/xdg-shell/xdg-shell.xml");
    let xdg_header = out_dir.join("xdg-shell-protocol.h");
    let status = Command::new("wayland-scanner")
        .arg("server-header")
        .arg(&xdg_xml)
        .arg(&xdg_header)
        .status()
        .expect("failed to run wayland-scanner");
    assert!(status.success(), "wayland-scanner failed on {xdg_xml}");

    // Same idea for wlr-layer-shell-unstable-v1: wlroots' wlr_layer_shell_v1.h
    // #includes the generated protocol header, but the protocol isn't shipped
    // as a system header. We vendor its XML in-repo (protocols/) instead of
    // depending on the wlr-protocols package, so the build stays self-contained.
    let manifest_dir = PathBuf::from(env::var("CARGO_MANIFEST_DIR").unwrap());
    let layer_shell_xml = manifest_dir.join("protocols/wlr-layer-shell-unstable-v1.xml");
    let layer_shell_header = out_dir.join("wlr-layer-shell-unstable-v1-protocol.h");
    let status = Command::new("wayland-scanner")
        .arg("server-header")
        .arg(&layer_shell_xml)
        .arg(&layer_shell_header)
        .status()
        .expect("failed to run wayland-scanner");
    assert!(
        status.success(),
        "wayland-scanner failed on {}",
        layer_shell_xml.display()
    );

    // Same again for wlr-output-power-management-unstable-v1: wlroots' own
    // wlr_output_power_management_v1.h #includes the generated protocol
    // header. wlroots implements the whole wire protocol for us — we only
    // react to its `set_mode` signal (see src/output_power.rs).
    let output_power_xml =
        manifest_dir.join("protocols/wlr-output-power-management-unstable-v1.xml");
    let output_power_header =
        out_dir.join("wlr-output-power-management-unstable-v1-protocol.h");
    let status = Command::new("wayland-scanner")
        .arg("server-header")
        .arg(&output_power_xml)
        .arg(&output_power_header)
        .status()
        .expect("failed to run wayland-scanner");
    assert!(
        status.success(),
        "wayland-scanner failed on {}",
        output_power_xml.display()
    );

    // Include dirs shared by the shim build (cc) and bindgen (clang): the
    // library headers plus OUT_DIR for our generated protocol header.
    let mut include_paths: Vec<PathBuf> = wlroots
        .include_paths
        .iter()
        .chain(wayland.include_paths.iter())
        .chain(xkbcommon.include_paths.iter())
        .chain(glesv2.include_paths.iter())
        .chain(egl.include_paths.iter())
        .cloned()
        .collect();
    include_paths.push(out_dir.clone());

    // 3. Compile the C shim. WLR_USE_UNSTABLE is set inside each .c file. Split
    //    by protocol (core listener glue, then one file per Wayland protocol)
    //    so no single file covers more than one concern.
    let shim_files = [
        "shim/core.c",
        "shim/output.c",
        "shim/xdg_shell.c",
        "shim/layer_shell.c",
        "shim/session_lock.c",
        "shim/output_power.c",
        "shim/decoration.c",
        "shim/input.c",
        "shim/gles2_corner.c",
    ];
    let mut shim = cc::Build::new();
    shim.files(shim_files);
    for path in &include_paths {
        shim.include(path);
    }
    shim.compile("oxide_shim");
    // `cc` emits the static shim archive here. Repeat xkbcommon after it so
    // strict `--as-needed` linkers (Alpine/musl) see the library after the
    // shim's xkb_* references instead of discarding the earlier pkg-config
    // link flag before those references appear. Same story for GLESv2/EGL,
    // which the corner-radius mask pass (shim/gles2_corner.c) calls directly.
    println!("cargo:rustc-link-lib=xkbcommon");
    println!("cargo:rustc-link-lib=GLESv2");
    println!("cargo:rustc-link-lib=EGL");

    // 4. Generate Rust bindings. We allowlist exactly the functions Rust calls
    //    directly; bindgen pulls in the types they reference automatically. The
    //    listener glue and helpers stay in C (declared in oxide_shim.h).
    let clang_args: Vec<String> = include_paths
        .iter()
        .map(|p| format!("-I{}", p.display()))
        .collect();

    let bindings = bindgen::Builder::default()
        .header("wrapper.h")
        .clang_args(&clang_args)
        .allowlist_function("wl_display_create")
        .allowlist_function("wl_display_get_event_loop")
        .allowlist_function("wl_display_add_socket_auto")
        .allowlist_function("wl_display_run")
        .allowlist_function("wl_display_terminate")
        .allowlist_function("wl_display_destroy_clients")
        .allowlist_function("wlr_backend_autocreate")
        .allowlist_function("wlr_backend_start")
        .allowlist_function("wlr_backend_destroy")
        .allowlist_function("wlr_renderer_autocreate")
        .allowlist_function("wlr_renderer_init_wl_display")
        .allowlist_function("wlr_allocator_autocreate")
        .allowlist_function("wlr_output_init_render")
        .allowlist_function("wlr_output_layout_create")
        .allowlist_function("wlr_output_layout_add_auto")
        .allowlist_function("wlr_output_layout_add")
        .allowlist_function("wlr_scene_create")
        .allowlist_function("wlr_scene_attach_output_layout")
        .allowlist_function("wlr_scene_output_create")
        .allowlist_function("wlr_scene_output_layout_add_output")
        .allowlist_function("wlr_compositor_create")
        .allowlist_function("wlr_subcompositor_create")
        .allowlist_function("wlr_data_device_manager_create")
        .allowlist_function("wlr_xdg_shell_create")
        .allowlist_function("wlr_xdg_toplevel_set_size")
        .allowlist_function("wlr_xdg_toplevel_set_fullscreen")
        .allowlist_function("wlr_xdg_toplevel_send_close")
        .allowlist_function("wlr_layer_shell_v1_create")
        .allowlist_function("wlr_output_power_manager_v1_create")
        .allowlist_function("wlr_xdg_decoration_manager_v1_create")
        .allowlist_function("wlr_screencopy_manager_v1_create")
        .allowlist_function("wlr_xdg_output_manager_v1_create")
        .allowlist_function("wlr_viewporter_create")
        .allowlist_function("wlr_fractional_scale_manager_v1_create")
        .allowlist_type("wlr_xdg_toplevel")
        .allowlist_type("wlr_session")
        .allowlist_type("wlr_seat")
        .allowlist_type("wlr_input_device")
        .allowlist_type("wlr_cursor")
        .allowlist_function("wlr_log.*")
        .layout_tests(false)
        .generate()
        .expect("bindgen failed to generate wlroots bindings");

    bindings
        .write_to_file(out_dir.join("wlr_bindings.rs"))
        .expect("failed to write wlr_bindings.rs");

    println!("cargo:rerun-if-changed=wrapper.h");
    for file in &shim_files {
        println!("cargo:rerun-if-changed={file}");
    }
    println!("cargo:rerun-if-changed=shim/oxide_shim.h");
    println!("cargo:rerun-if-changed=shim/oxide_shim_internal.h");
    println!("cargo:rerun-if-changed=protocols/wlr-layer-shell-unstable-v1.xml");
    println!(
        "cargo:rerun-if-changed=protocols/wlr-output-power-management-unstable-v1.xml"
    );
}
