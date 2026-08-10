#define WLR_USE_UNSTABLE
#include <stdbool.h>
#include <stdint.h>
#include <stdlib.h>
#include <wayland-server-core.h>
#include <wlr/types/wlr_compositor.h>
#include <wlr/types/wlr_seat.h>
#include <wlr/types/wlr_text_input_v3.h>

#include "oxide_shim_internal.h"

struct oxide_text_input_manager;

struct oxide_text_input {
    struct wl_list link;
    struct oxide_text_input_manager *manager;
    struct wlr_text_input_v3 *text_input;
    struct oxide_listener *enable;
    struct oxide_listener *disable;
    struct oxide_listener *destroy;
};

struct oxide_text_input_manager {
    struct wlr_text_input_manager_v3 *manager;
    struct wlr_seat *seat;
    struct wl_list text_inputs;
    struct oxide_listener *new_text_input;
    struct oxide_listener *focus_change;
    oxide_callback visibility_callback;
    void *userdata;
    bool visible;
};

static void update_visibility(struct oxide_text_input_manager *manager) {
    bool visible = false;
    struct oxide_text_input *entry;
    wl_list_for_each(entry, &manager->text_inputs, link) {
        if (entry->text_input->current_enabled
                && entry->text_input->focused_surface != NULL) {
            visible = true;
            break;
        }
    }
    if (manager->visible == visible) {
        return;
    }
    manager->visible = visible;
    manager->visibility_callback(manager->userdata,
            (void *)(uintptr_t)(visible ? 1 : 0));
}

static void handle_enable(void *userdata, void *data) {
    (void)data;
    struct oxide_text_input *entry = userdata;
    update_visibility(entry->manager);
}

static void handle_disable(void *userdata, void *data) {
    (void)data;
    struct oxide_text_input *entry = userdata;
    update_visibility(entry->manager);
}

static void handle_destroy(void *userdata, void *data) {
    (void)data;
    struct oxide_text_input *entry = userdata;
    struct oxide_text_input_manager *manager = entry->manager;
    wl_list_remove(&entry->link);
    oxide_listener_remove(entry->enable);
    oxide_listener_remove(entry->disable);
    oxide_listener_remove(entry->destroy);
    free(entry);
    update_visibility(manager);
}

static void handle_new_text_input(void *userdata, void *data) {
    struct oxide_text_input_manager *manager = userdata;
    struct wlr_text_input_v3 *text_input = data;
    if (text_input->seat != manager->seat) {
        return;
    }
    struct oxide_text_input *entry = calloc(1, sizeof(*entry));
    if (entry == NULL) {
        return;
    }
    entry->manager = manager;
    entry->text_input = text_input;
    entry->enable = signal_add(&text_input->events.enable, handle_enable, entry);
    entry->disable = signal_add(&text_input->events.disable, handle_disable, entry);
    entry->destroy = signal_add(&text_input->events.destroy, handle_destroy, entry);
    wl_list_insert(&manager->text_inputs, &entry->link);

    // A client may create its text-input object after its toplevel already has
    // keyboard focus. In that case no focus-change event will arrive to tell
    // the client it may enable text input, so deliver the current focus now.
    struct wlr_surface *focused = manager->seat->keyboard_state.focused_surface;
    if (focused != NULL
            && wl_resource_get_client(text_input->resource)
                == wl_resource_get_client(focused->resource)) {
        wlr_text_input_v3_send_enter(text_input, focused);
    }
}

static void handle_focus_change(void *userdata, void *data) {
    struct oxide_text_input_manager *manager = userdata;
    struct wlr_seat_keyboard_focus_change_event *event = data;
    struct oxide_text_input *entry;
    wl_list_for_each(entry, &manager->text_inputs, link) {
        if (entry->text_input->focused_surface != NULL) {
            wlr_text_input_v3_send_leave(entry->text_input);
        }
        if (event->new_surface != NULL
                && wl_resource_get_client(entry->text_input->resource)
                    == wl_resource_get_client(event->new_surface->resource)) {
            wlr_text_input_v3_send_enter(entry->text_input, event->new_surface);
        }
    }
    update_visibility(manager);
}

void oxide_text_input_setup(struct wl_display *display, struct wlr_seat *seat,
        oxide_callback visibility_callback, void *userdata) {
    struct oxide_text_input_manager *manager = calloc(1, sizeof(*manager));
    if (manager == NULL) {
        return;
    }
    manager->manager = wlr_text_input_manager_v3_create(display);
    if (manager->manager == NULL) {
        free(manager);
        return;
    }
    manager->seat = seat;
    manager->visibility_callback = visibility_callback;
    manager->userdata = userdata;
    wl_list_init(&manager->text_inputs);
    manager->new_text_input = signal_add(&manager->manager->events.text_input,
            handle_new_text_input, manager);
    manager->focus_change = signal_add(&seat->keyboard_state.events.focus_change,
            handle_focus_change, manager);
}
