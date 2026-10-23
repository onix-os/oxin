// inotify glue for config live reload (Stage 31). Rust decides which
// directories to watch and what a change means; this file only owns the
// variable-length event records, which are awkward to walk from Rust with no
// libc bindings.
#define WLR_USE_UNSTABLE
#include <stdbool.h>
#include <string.h>
#include <sys/inotify.h>
#include <unistd.h>

#include "oxide_shim_internal.h"

int oxide_inotify_open(void) {
    return inotify_init1(IN_NONBLOCK | IN_CLOEXEC);
}

bool oxide_inotify_watch_dir(int fd, const char *path) {
    // Directories, not files: editors save by writing a temporary and renaming
    // it over the original, which a watch on the file itself would lose.
    const uint32_t mask = IN_CLOSE_WRITE | IN_MOVED_TO | IN_MOVED_FROM |
            IN_CREATE | IN_DELETE | IN_ONLYDIR;
    return inotify_add_watch(fd, path, mask) >= 0;
}

static bool is_lua_name(const char *name) {
    size_t len = strlen(name);
    return len > 4 && strcmp(name + len - 4, ".lua") == 0;
}

bool oxide_inotify_drain(int fd) {
    // Aligned as the man page asks, so the records can be read in place.
    char buf[4096] __attribute__((aligned(__alignof__(struct inotify_event))));
    bool relevant = false;
    for (;;) {
        ssize_t len = read(fd, buf, sizeof(buf));
        if (len <= 0) {
            // EAGAIN: drained. Anything else: nothing more to read either.
            return relevant;
        }
        for (char *at = buf; at < buf + len;) {
            const struct inotify_event *event = (const struct inotify_event *)at;
            // A lost queue may have hidden a save; a new or removed directory
            // may be a `plugin/` that changes what there is to watch.
            if ((event->mask & (IN_Q_OVERFLOW | IN_ISDIR)) != 0 ||
                    (event->len > 0 && is_lua_name(event->name))) {
                relevant = true;
            }
            at += sizeof(*event) + event->len;
        }
    }
}
