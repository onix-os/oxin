# 0xin — cargo builds, make installs.
#
# **Cargo is the build system**, and stays it: every target below shells out to it.
# This file exists for the one thing cargo cannot do — put files that are not binaries
# where the system expects them. `cargo install` handles binaries only, and only into
# ~/.cargo/bin, which is not somewhere a display manager looks for a session entry.
# Same division of labour as cosmic-comp's Makefile; niri takes the other route and
# leaves the placing to distro packaging.
#
# PREFIX follows Hyprland: /usr/local when you build from source, and a packager
# overrides PREFIX=/usr (which is how Arch's hyprland lands in /usr/bin). DESTDIR is
# honoured so a package can be staged into a build root.
#
#   make                      # release build
#   sudo make install         # into /usr/local
#   make install PREFIX=/usr DESTDIR=pkg    # staged, for packaging
#   sudo make uninstall

PREFIX ?= /usr/local
DESTDIR ?=

BINDIR = $(DESTDIR)$(PREFIX)/bin
SHAREDIR = $(DESTDIR)$(PREFIX)/share

CARGO ?= cargo
CARGO_TARGET_DIR ?= target

# `make DEBUG=1` for the fast build; release is the default because that is what an
# install is for.
DEBUG ?= 0
ifeq ($(DEBUG),0)
	PROFILE_DIR = release
	CARGO_ARGS = --release
else
	PROFILE_DIR = debug
	CARGO_ARGS =
endif

BUILT = $(CARGO_TARGET_DIR)/$(PROFILE_DIR)

.PHONY: all build test install uninstall clean

all: build

build:
	$(CARGO) build $(CARGO_ARGS)

test:
	$(CARGO) test $(CARGO_ARGS)

# `install -D` creates the parent directories, so there is no mkdir dance.
install: build
	install -Dm755 $(BUILT)/0xin $(BINDIR)/0xin
	install -Dm755 $(BUILT)/0xinctl $(BINDIR)/0xinctl
	install -Dm644 dist/0xin.desktop $(SHAREDIR)/wayland-sessions/0xin.desktop

uninstall:
	rm -f $(BINDIR)/0xin
	rm -f $(BINDIR)/0xinctl
	rm -f $(SHAREDIR)/wayland-sessions/0xin.desktop

clean:
	$(CARGO) clean
