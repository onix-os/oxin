// Bindgen entry point: everything #included here becomes Rust declarations.
//
// wlroots gates its real API behind WLR_USE_UNSTABLE — without this define the
// headers expand to `#error`. The shim .c sets the same define for itself.
#define WLR_USE_UNSTABLE

#include <wayland-server-core.h>     // wl_display_*, wl_event_loop
#include <wlr/backend.h>             // wlr_backend_autocreate/start/destroy
#include <wlr/render/allocator.h>    // wlr_allocator_autocreate
#include <wlr/render/wlr_renderer.h> // wlr_renderer_autocreate, init_wl_display
#include <wlr/types/wlr_compositor.h>     // wlr_compositor_create
#include <wlr/types/wlr_subcompositor.h>  // wlr_subcompositor_create
#include <wlr/types/wlr_data_device.h>    // wlr_data_device_manager_create
#include <wlr/types/wlr_cursor.h>         // wlr_cursor (opaque to Rust)
#include <wlr/types/wlr_input_device.h>   // wlr_input_device (opaque to Rust)
#include <wlr/types/wlr_output.h>         // wlr_output, wlr_output_init_render
#include <wlr/types/wlr_output_layout.h>  // wlr_output_layout_create/add_auto
#include <wlr/types/wlr_scene.h>          // wlr_scene_*
#include <wlr/types/wlr_seat.h>           // wlr_seat (opaque to Rust)
#include <wlr/types/wlr_xdg_shell.h>      // wlr_xdg_shell_create, wlr_xdg_toplevel
#include <wlr/types/wlr_layer_shell_v1.h> // wlr_layer_shell_v1_create, wlr_layer_surface_v1
#include <wlr/types/wlr_output_power_management_v1.h> // wlr_output_power_manager_v1_create
#include <wlr/types/wlr_xdg_decoration_v1.h> // wlr_xdg_decoration_manager_v1_create
#include <wlr/types/wlr_screencopy_v1.h>   // wlr_screencopy_manager_v1_create
#include <wlr/types/wlr_xdg_output_v1.h>   // wlr_xdg_output_manager_v1_create
#include <wlr/types/wlr_viewporter.h>      // wlr_viewporter_create
#include <wlr/types/wlr_fractional_scale_v1.h> // wlr_fractional_scale_manager_v1_create
#include <wlr/util/log.h>            // wlr_log_*
#include <wlr/version.h>             // WLR_VERSION_STR
