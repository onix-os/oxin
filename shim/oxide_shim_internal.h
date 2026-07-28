#ifndef OXIN_SHIM_INTERNAL_H
#define OXIN_SHIM_INTERNAL_H

// Shared across every shim .c file; never seen by bindgen (wrapper.h doesn't
// include any shim header — see build.rs). wlroots delivers every event
// through wl_signal/wl_listener: you embed a wl_listener in your own struct,
// attach it to a signal, and on fire recover your struct from the listener
// pointer via wl_container_of (offsetof math). We wrap that intrusive pattern
// once and expose a plain (userdata, data) callback so Rust never touches the
// linked list or the pointer arithmetic.

#include <wayland-server-core.h>

#include "oxide_shim.h"

struct oxide_listener {
    struct wl_listener listener; // must stay put once added to a signal
    oxide_callback callback;   // Rust function to invoke
    void *userdata;              // opaque pointer Rust handed us
};

// Defined in core.c; every other shim .c file calls this to register a
// listener without touching wl_signal/wl_listener directly.
struct oxide_listener *signal_add(struct wl_signal *signal,
        oxide_callback callback, void *userdata);

#endif // OXIN_SHIM_INTERNAL_H
