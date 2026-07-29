#define WLR_USE_UNSTABLE
#include <signal.h>
#include <stdlib.h>
#include <wayland-server.h>
#include <xkbcommon/xkbcommon.h>
#include <wlr/backend/session.h>
#include <wlr/util/log.h>
#include <wlr/version.h>

#include "oxide_shim_internal.h"

const char *oxide_wlroots_version(void) {
    return WLR_VERSION_STR;
}

void oxide_log_init(void) {
    // Debug verbosity, default stderr sink. Done in C so the enum stays native.
    wlr_log_init(WLR_DEBUG, NULL);
}

// Resolve a key name from the config (e.g. "Return", "q", "1") to an xkb keysym,
// case-insensitively. Returns 0 (XKB_KEY_NoSymbol) for an unknown name. We match
// bindings on level-0 keysyms, so case-insensitive lookup gives the unshifted
// form (e.g. "Q" -> lowercase q), exactly what handle_key reports.
uint32_t oxide_keysym_from_name(const char *name) {
    return xkb_keysym_from_name(name, XKB_KEYSYM_CASE_INSENSITIVE);
}

static int handle_signal(int sig, void *data) {
    (void)sig;
    wl_display_terminate(data); // unwinds wl_display_run -> graceful shutdown
    return 0;
}

void oxide_setup_signals(struct wl_event_loop *loop, struct wl_display *display) {
    // Handled via the event loop's signalfd, so it's safe (not an async signal).
    wl_event_loop_add_signal(loop, SIGINT, handle_signal, display);
    wl_event_loop_add_signal(loop, SIGTERM, handle_signal, display);
    // Auto-reap spawned clients (POSIX: ignoring SIGCHLD reaps children on
    // exit). We never wait() on any child, so without this every closed app
    // would stay a zombie for the compositor session's lifetime. The ignored
    // disposition would leak into clients (see oxide_reset_child_signals).
    signal(SIGCHLD, SIG_IGN);
}

struct oxide_fd_source {
    oxide_callback callback;
    void *userdata;
};

static int handle_readable_fd(int fd, uint32_t mask, void *data) {
    (void)fd;
    if ((mask & WL_EVENT_READABLE) == 0) {
        return 0;
    }
    struct oxide_fd_source *source = data;
    source->callback(source->userdata, NULL);
    return 0;
}

void *oxide_event_loop_add_readable(struct wl_event_loop *loop, int fd,
        oxide_callback callback, void *userdata) {
    struct oxide_fd_source *source = calloc(1, sizeof(*source));
    source->callback = callback;
    source->userdata = userdata;
    // The display event loop owns the wl_event_source. The tiny callback
    // context intentionally lives until compositor exit with the loop.
    if (wl_event_loop_add_fd(loop, fd, WL_EVENT_READABLE,
                handle_readable_fd, source) == NULL) {
        free(source);
        return NULL;
    }
    return source;
}

// Undo the compositor's signal state in a freshly forked child, before exec.
// Two things leak into clients otherwise, because they survive exec:
//  - SIG_IGN dispositions (unlike handlers, which exec resets): our ignored
//    SIGCHLD makes the kernel auto-reap the client's own children, so
//    anything that reads child exit codes (Qt's QProcess, shells) breaks —
//    found via quickshell's recording indicator treating every pgrep as
//    exit 0, i.e. "wf-recorder is running", permanently.
//  - The blocked-signal mask: libwayland's signalfd setup blocks SIGINT and
//    SIGTERM in our process, so clients inherit them blocked and a plain
//    kill -TERM would sit pending forever.
// Called via pre_exec from every Rust spawn path; everything here is
// async-signal-safe.
void oxide_reset_child_signals(void) {
    signal(SIGCHLD, SIG_DFL);
    sigset_t empty;
    sigemptyset(&empty);
    sigprocmask(SIG_SETMASK, &empty, NULL);
}

// --- session / VT ----------------------------------------------------------

// Switch to virtual terminal `vt` (1-based). No-op when there's no session
// (e.g. running nested, where autocreate hands back a NULL session).
void oxide_session_change_vt(struct wlr_session *session, unsigned vt) {
    if (session != NULL) {
        wlr_session_change_vt(session, vt);
    }
}

// True if the session currently owns the VT (false while switched away).
bool oxide_session_is_active(struct wlr_session *session) {
    return session != NULL && session->active;
}

// Subscribe to the session active signal (fires on every VT switch, away and
// back). The handler uses oxide_session_is_active to tell direction. No-op
// (NULL) when there's no session, e.g. nested.
struct oxide_listener *oxide_session_add_active(struct wlr_session *session,
        oxide_callback callback, void *userdata) {
    if (session == NULL) {
        return NULL;
    }
    return signal_add(&session->events.active, callback, userdata);
}

// --- listener glue -----------------------------------------------------

static void oxide_listener_notify(struct wl_listener *listener, void *data) {
    struct oxide_listener *l = wl_container_of(listener, l, listener);
    l->callback(l->userdata, data);
}

struct oxide_listener *signal_add(struct wl_signal *signal,
        oxide_callback callback, void *userdata) {
    struct oxide_listener *l = calloc(1, sizeof(*l));
    l->listener.notify = oxide_listener_notify;
    l->callback = callback;
    l->userdata = userdata;
    wl_signal_add(signal, &l->listener);
    return l;
}

// Unsubscribe and free a listener. Each per-window listener must be removed
// before its object is destroyed (wlroots asserts an empty destroy list).
void oxide_listener_remove(struct oxide_listener *l) {
    wl_list_remove(&l->listener.link);
    free(l);
}
