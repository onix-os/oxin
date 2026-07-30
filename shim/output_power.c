#define WLR_USE_UNSTABLE
#include <wlr/types/wlr_output_power_management_v1.h>

#include "oxide_shim_internal.h"

// --- wlr-output-power-management-unstable-v1 --------------------------------
//
// The global itself is created directly from Rust via the bindgen binding for
// wlr_output_power_manager_v1_create (same pattern as wlr_xdg_shell_create) —
// no shim wrapper needed for a plain creation call. wlroots owns the whole
// wire protocol; we only listen for `set_mode` and apply it via
// oxide_output_set_powered (shim/output.c).

struct oxide_listener *oxide_output_power_manager_add_set_mode(
        struct wlr_output_power_manager_v1 *manager, oxide_callback callback,
        void *userdata) {
    return signal_add(&manager->events.set_mode, callback, userdata);
}

struct wlr_output *oxide_output_power_set_mode_event_output(
        struct wlr_output_power_v1_set_mode_event *event) {
    return event->output;
}

bool oxide_output_power_set_mode_event_is_on(
        struct wlr_output_power_v1_set_mode_event *event) {
    return event->mode == ZWLR_OUTPUT_POWER_V1_MODE_ON;
}
