#define WLR_USE_UNSTABLE
#include <stdlib.h>
#include <wlr/types/wlr_keyboard.h>
#include <wlr/types/wlr_scene.h>
#include <wlr/types/wlr_seat.h>
#include <wlr/types/wlr_session_lock_v1.h>

#include "oxide_shim_internal.h"

struct wlr_session_lock_manager_v1 *oxide_session_lock_manager_create(
        struct wl_display *display) {
    return wlr_session_lock_manager_v1_create(display);
}

struct oxide_listener *oxide_session_lock_manager_add_new_lock(
        struct wlr_session_lock_manager_v1 *manager,
        oxide_callback callback, void *userdata) {
    return signal_add(&manager->events.new_lock, callback, userdata);
}

struct oxide_listener *oxide_session_lock_add_new_surface(
        struct wlr_session_lock_v1 *lock,
        oxide_callback callback, void *userdata) {
    return signal_add(&lock->events.new_surface, callback, userdata);
}

struct oxide_listener *oxide_session_lock_add_unlock(
        struct wlr_session_lock_v1 *lock,
        oxide_callback callback, void *userdata) {
    return signal_add(&lock->events.unlock, callback, userdata);
}

struct oxide_listener *oxide_session_lock_add_destroy(
        struct wlr_session_lock_v1 *lock,
        oxide_callback callback, void *userdata) {
    return signal_add(&lock->events.destroy, callback, userdata);
}

void oxide_session_lock_send_locked(struct wlr_session_lock_v1 *lock) {
    wlr_session_lock_v1_send_locked(lock);
}

void oxide_session_lock_reject(struct wlr_session_lock_v1 *lock) {
    wlr_session_lock_v1_destroy(lock);
}

struct wlr_output *oxide_session_lock_surface_output(
        struct wlr_session_lock_surface_v1 *surface) {
    return surface->output;
}

struct wlr_scene_tree *oxide_scene_session_lock_surface_create(
        struct wlr_scene_tree *parent,
        struct wlr_session_lock_surface_v1 *surface) {
    return wlr_scene_subsurface_tree_create(parent, surface->surface);
}

void oxide_session_lock_surface_configure(
        struct wlr_session_lock_surface_v1 *surface,
        uint32_t width, uint32_t height) {
    wlr_session_lock_surface_v1_configure(surface, width, height);
}

void oxide_focus_session_lock_surface(struct wlr_seat *seat,
        struct wlr_session_lock_surface_v1 *surface) {
    struct wlr_keyboard *keyboard = wlr_seat_get_keyboard(seat);
    if (keyboard != NULL) {
        wlr_seat_keyboard_notify_enter(seat, surface->surface,
                keyboard->keycodes, keyboard->num_keycodes,
                &keyboard->modifiers);
    } else {
        wlr_seat_keyboard_notify_enter(seat, surface->surface,
                NULL, 0, NULL);
    }
}

void oxide_seat_clear_keyboard_focus(struct wlr_seat *seat) {
    wlr_seat_keyboard_clear_focus(seat);
}

struct oxide_listener *oxide_session_lock_surface_add_map(
        struct wlr_session_lock_surface_v1 *surface,
        oxide_callback callback, void *userdata) {
    return signal_add(&surface->surface->events.map, callback, userdata);
}

struct oxide_listener *oxide_session_lock_surface_add_destroy(
        struct wlr_session_lock_surface_v1 *surface,
        oxide_callback callback, void *userdata) {
    return signal_add(&surface->events.destroy, callback, userdata);
}
