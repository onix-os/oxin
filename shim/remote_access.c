// What a program that drives this session from outside needs: a remote-desktop
// server (wayvnc, 0xin-rdp), a clipboard manager, wl-clipboard. All standard
// protocols that wlroots implements in full; 0xin only creates the globals and
// plugs a virtual pointer into the cursor.
//
//  - wlr-virtual-pointer-unstable-v1: the remote user's mouse. Its events go
//    through the same wlr_cursor as a real mouse, so focus, grabs and gestures
//    behave the same.
//  - wlr-data-control-unstable-v1 and ext-data-control-v1: read and set the
//    clipboard without having keyboard focus (a server syncing it with the
//    remote side; wl-copy, which otherwise waits for focus).
//  - ext-image-copy-capture-v1 with the output capture source: the modern
//    screen-capture protocol, next to wlr-screencopy (created in main.rs).
//
// The virtual keyboard is set up with the other keyboards (keyboard_seat.c).

#define WLR_USE_UNSTABLE
#include <stdlib.h>
#include <wayland-server-core.h>
#include <wlr/types/wlr_cursor.h>
#include <wlr/types/wlr_data_control_v1.h>
#include <wlr/types/wlr_ext_data_control_v1.h>
#include <wlr/types/wlr_ext_image_capture_source_v1.h>
#include <wlr/types/wlr_ext_image_copy_capture_v1.h>
#include <wlr/types/wlr_output.h>
#include <wlr/types/wlr_pointer.h>
#include <wlr/types/wlr_virtual_pointer_v1.h>
#include <wlr/util/log.h>

#include "oxide_shim_internal.h"

static void handle_new_virtual_pointer(void *userdata, void *data) {
    struct wlr_cursor *cursor = userdata;
    struct wlr_virtual_pointer_v1_new_pointer_event *event = data;
    struct wlr_input_device *device = &event->new_pointer->pointer.base;
    // wlr_cursor drops the device by itself when the client destroys it.
    wlr_cursor_attach_input_device(cursor, device);
    // A client that names an output sends absolute motion relative to that
    // output rather than to the whole layout.
    if (event->suggested_output != NULL) {
        wlr_cursor_map_input_to_output(cursor, device, event->suggested_output);
    }
    wlr_log(WLR_INFO, "0xin: virtual pointer attached%s%s",
            event->suggested_output != NULL ? ", mapped to " : "",
            event->suggested_output != NULL ? event->suggested_output->name : "");
}

void oxide_remote_access_setup(struct wl_display *display,
        struct wlr_cursor *cursor) {
    struct wlr_virtual_pointer_manager_v1 *pointers =
            wlr_virtual_pointer_manager_v1_create(display);
    signal_add(&pointers->events.new_virtual_pointer,
            handle_new_virtual_pointer, cursor);

    wlr_data_control_manager_v1_create(display);
    wlr_ext_data_control_manager_v1_create(display, 1);

    wlr_ext_image_copy_capture_manager_v1_create(display, 1);
    wlr_ext_output_image_capture_source_manager_v1_create(display, 1);
}
