// keyboard-shortcuts-inhibit-v1: a client (a remote-desktop viewer, a VM
// window) asks that the compositor's own shortcuts stand down while its
// surface has keyboard focus, so Super and every 0xin chord reach the remote
// machine instead. Remmina's keyboard grab and SDL's keyboard grab (FreeRDP's
// SDL client) both come in through this.
//
// The shim only tracks the requests and answers "is the focused surface
// inhibiting?"; what an inhibited key does is Rust's call (handle_keybinding
// lets through only the `pass_keys` toggle, which is the way back out).

#define WLR_USE_UNSTABLE
#include <stdlib.h>
#include <wayland-server-core.h>
#include <wlr/types/wlr_keyboard_shortcuts_inhibit_v1.h>
#include <wlr/types/wlr_seat.h>
#include <wlr/util/log.h>

#include "oxide_shim_internal.h"

struct oxide_shortcuts_inhibit {
    struct wlr_keyboard_shortcuts_inhibit_manager_v1 *manager;
    struct wl_event_loop *loop;
    // Whether a new request is granted straight away (`oxin.shortcuts_inhibit`).
    // When it isn't, the request is still kept, so `pass_keys` can grant it.
    bool grant;
    oxide_callback changed;
    void *userdata;
};

struct inhibitor_watch {
    struct oxide_shortcuts_inhibit *ctx;
    struct oxide_listener *destroy;
};

static void notify_changed(void *data) {
    struct oxide_shortcuts_inhibit *ctx = data;
    ctx->changed(ctx->userdata, NULL);
}

static void handle_inhibitor_destroy(void *userdata, void *data) {
    (void)data;
    struct inhibitor_watch *watch = userdata;
    struct oxide_shortcuts_inhibit *ctx = watch->ctx;
    // wlroots asserts its destroy signal has no listeners left afterwards.
    oxide_listener_remove(watch->destroy);
    free(watch);
    // The inhibitor is still on the manager's list (and maybe still active)
    // while this signal runs; tell Rust once it is really gone.
    wl_event_loop_add_idle(ctx->loop, notify_changed, ctx);
}

static void handle_new_inhibitor(void *userdata, void *data) {
    struct oxide_shortcuts_inhibit *ctx = userdata;
    struct wlr_keyboard_shortcuts_inhibitor_v1 *inhibitor = data;
    struct inhibitor_watch *watch = calloc(1, sizeof(*watch));
    watch->ctx = ctx;
    watch->destroy = signal_add(&inhibitor->events.destroy,
            handle_inhibitor_destroy, watch);
    if (ctx->grant) {
        wlr_keyboard_shortcuts_inhibitor_v1_activate(inhibitor);
    }
    wlr_log(WLR_INFO, "0xin: a client asked for all keys (shortcuts inhibit); %s",
            ctx->grant ? "granted" : "not granted (oxin.shortcuts_inhibit = false)");
    ctx->changed(ctx->userdata, NULL);
}

struct oxide_shortcuts_inhibit *oxide_shortcuts_inhibit_setup(
        struct wl_display *display, oxide_callback changed, void *userdata) {
    struct oxide_shortcuts_inhibit *ctx = calloc(1, sizeof(*ctx));
    ctx->manager = wlr_keyboard_shortcuts_inhibit_v1_create(display);
    ctx->loop = wl_display_get_event_loop(display);
    ctx->grant = true;
    ctx->changed = changed;
    ctx->userdata = userdata;
    signal_add(&ctx->manager->events.new_inhibitor, handle_new_inhibitor, ctx);
    return ctx;
}

void oxide_shortcuts_inhibit_set_grant(struct oxide_shortcuts_inhibit *ctx,
        bool grant) {
    ctx->grant = grant;
}

// The request made for the surface that has keyboard focus, if any.
static struct wlr_keyboard_shortcuts_inhibitor_v1 *focused_inhibitor(
        struct oxide_shortcuts_inhibit *ctx, struct wlr_seat *seat) {
    struct wlr_surface *focused = seat->keyboard_state.focused_surface;
    if (focused == NULL) {
        return NULL;
    }
    struct wlr_keyboard_shortcuts_inhibitor_v1 *inhibitor;
    wl_list_for_each(inhibitor, &ctx->manager->inhibitors, link) {
        if (inhibitor->surface == focused && inhibitor->seat == seat) {
            return inhibitor;
        }
    }
    return NULL;
}

bool oxide_shortcuts_inhibited(struct oxide_shortcuts_inhibit *ctx,
        struct wlr_seat *seat) {
    struct wlr_keyboard_shortcuts_inhibitor_v1 *inhibitor =
            focused_inhibitor(ctx, seat);
    return inhibitor != NULL && inhibitor->active;
}

bool oxide_shortcuts_inhibit_set(struct oxide_shortcuts_inhibit *ctx,
        struct wlr_seat *seat, bool active) {
    struct wlr_keyboard_shortcuts_inhibitor_v1 *inhibitor =
            focused_inhibitor(ctx, seat);
    if (inhibitor == NULL) {
        return false;
    }
    // Both send the client an event, so a viewer that shows "keyboard
    // grabbed" stays truthful.
    if (active && !inhibitor->active) {
        wlr_keyboard_shortcuts_inhibitor_v1_activate(inhibitor);
    } else if (!active && inhibitor->active) {
        wlr_keyboard_shortcuts_inhibitor_v1_deactivate(inhibitor);
    }
    return true;
}
