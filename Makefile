SHELL := /bin/bash

PROJECT_NAME_FROM_CARGO := $(shell sed -n 's/^[[:space:]]*name[[:space:]]*=[[:space:]]*"\([^"]*\)".*/\1/p' Cargo.toml | head -1)
PROJECT_VERSION_FROM_CARGO := $(shell sed -n 's/^[[:space:]]*version[[:space:]]*=[[:space:]]*"\([^"]*\)".*/\1/p' Cargo.toml | head -1)
PROJECT_NAME ?= $(or $(PROJECT_NAME_FROM_CARGO),$(notdir $(CURDIR)))
PROJECT_VERSION ?= $(or $(PROJECT_VERSION_FROM_CARGO),dev)

TOP_DIR := $(CURDIR)
CARGO := cargo

# Every target runs inside the flake's dev shell, so a bare `make build` works
# from any shell — the compositor needs libinput/libseat/libgbm/EGL from it,
# and --impure lets the flake read the running NVIDIA driver version out of
# /proc. Already inside the shell (direnv, or `nix develop` by hand), we skip
# the wrapper instead of nesting a second one; OXIN_DEVSHELL is set by the
# flake, unlike IN_NIX_SHELL which any unrelated nix shell also sets.
# `make NIX_RUN=` opts out entirely.
ifdef OXIN_DEVSHELL
NIX_RUN ?=
else
NIX_RUN ?= nix develop --impure --option warn-dirty false --command
endif
# Windowing backend the nested (winit) compositor opens its window through.
BACKEND ?= wayland
DISPLAY ?= :1
APP_BIN ?= 0xin
APP_TARGET := --bin $(APP_BIN)
# The nested backend renders with GLES/EGL, so it needs the GL wrapper — not
# the Vulkan one. Assigned with := rather than ?= on purpose: a RUN_WITH left
# in the environment for Vulkan apps (nixVulkan, nixLavapipe) would break the
# nested run, while `make run RUN_WITH=...` on the command line still wins.
# On NixOS, or anywhere /run/opengl-driver exists, use RUN_WITH= .
RUN_WITH := nixGL
# Software rendering: auto (only when the session cannot do hardware GL), 1 to
# force llvmpipe, 0 to insist on the GPU. A waypipe session started --no-gpu
# has no GPU passthrough, and hardware EGL fails there with BAD_ALLOC — this is
# the GL counterpart of mara's RUN_WITH=nixLavapipe.
SOFTWARE ?= auto
# Nested, the host compositor grabs Super-chords before we see them, so
# development uses Alt as the modifier.
MOD ?= alt
# A client launched inside 0xin, so the window is not empty. First terminal
# found wins; CLIENT= for none.
CLIENT ?= $(shell command -v ghostty kitty foot alacritty 2>/dev/null | head -n1)
# ghostty defaults to single-instance: a bare `ghostty` hands the request to the
# daemon already running on your session, which then opens the window on the
# host screen and ignores the WAYLAND_DISPLAY we gave it. Force a fresh process
# so the window lands inside 0xin.
CLIENT_ARGS ?= $(if $(findstring ghostty,$(CLIENT)),--gtk-single-instance=false,)
ARGS ?=

$(info ------------------------------------------)
$(info Project: $(PROJECT_NAME) v$(PROJECT_VERSION))
$(info Display: $(BACKEND) backend, $(RUN_WITH) wrapper)
$(info ------------------------------------------)

.PHONY: build b compile c run r nested tty test t check fmt clippy clean help h

build:
	@$(NIX_RUN) $(CARGO) build $(APP_TARGET)

b: build

compile:
	@$(NIX_RUN) $(CARGO) clean
	@$(MAKE) build

c: compile

# Nested: 0xin opens as a window inside the current session, with a client
# already running in it so there is something to look at and tile.
# A long-lived terminal (tmux, a waypipe session) often carries a WAYLAND_DISPLAY
# whose socket is long dead, and winit then fails with "Failed to initialize an
# event loop". mara avoids this because its .envrc runs `use display`; ours does
# not, so the same resolution happens here: keep WAYLAND_DISPLAY if its socket
# still answers, else take the newest live waypipe display, else wayland-0, else
# fall back to X11.
run:
	@runtime=$${XDG_RUNTIME_DIR:-/run/user/$$(id -u)}; \
	alive() { \
		[ -S "$$1" ] || return 1; \
		command -v python3 >/dev/null 2>&1 || return 0; \
		python3 -c 'import socket,sys; s=socket.socket(socket.AF_UNIX); s.settimeout(1); s.connect(sys.argv[1])' "$$1" 2>/dev/null; \
	}; \
	wl="$$WAYLAND_DISPLAY"; \
	if [ -z "$$wl" ] || ! alive "$$runtime/$$wl"; then \
		was="$$wl"; wl=""; \
		gpu=$$(ps -u $$(id -u) -o args= 2>/dev/null | grep -- "waypipe " | grep -v -- "--no-gpu" | sed -n 's/.*--display[ ]*\([^ ]*\).*/\1/p' | tac); \
		nogpu=$$(ps -u $$(id -u) -o args= 2>/dev/null | grep -- "waypipe .*--no-gpu" | sed -n 's/.*--display[ ]*\([^ ]*\).*/\1/p' | tac); \
		for cand in $$gpu $$nogpu wayland-0; do \
			if alive "$$runtime/$$cand"; then wl="$$cand"; break; fi; \
		done; \
		if [ -n "$$wl" ] && [ -n "$$was" ]; then \
			echo "make run: WAYLAND_DISPLAY=$$was is dead — using $$wl"; \
		fi; \
	fi; \
	if [ -z "$$wl" ] && [ -z "$$DISPLAY" ]; then \
		echo "make run: no live Wayland socket and no DISPLAY — you are not in a session."; \
		echo "          On a bare TTY use 'make tty' instead."; \
		exit 1; \
	fi; \
	software=$(SOFTWARE); \
	if [ "$$software" = auto ]; then \
		software=0; \
		if [ -n "$$wl" ] && ps -u $$(id -u) -o args= 2>/dev/null \
			| grep -q -- "waypipe .*--no-gpu.*--display[ ]*$$wl\( \|$$\)"; then \
			software=1; \
			echo "make run: $$wl is a waypipe --no-gpu session — rendering with llvmpipe"; \
		fi; \
	fi; \
	if [ -n "$$wl" ]; then display_env="WAYLAND_DISPLAY=$$wl"; backend=$(BACKEND); \
	else \
		echo "make run: no live Wayland socket — falling back to X11 ($$DISPLAY)"; \
		display_env="WAYLAND_DISPLAY="; backend=x11; \
	fi; \
	launch() { \
		if [ "$$1" = 1 ]; then wrap=""; soft="LIBGL_ALWAYS_SOFTWARE=1"; \
		else wrap="$(RUN_WITH)"; soft="LIBGL_ALWAYS_SOFTWARE="; fi; \
		$(NIX_RUN) env $$display_env $$soft WINIT_UNIX_BACKEND=$$backend OXIN_MOD=$(MOD) \
			$$wrap $(CARGO) run $(APP_TARGET) -- $(ARGS) $(CLIENT) $(CLIENT_ARGS); \
	}; \
	if launch $$software; then exit 0; fi; \
	status=$$?; \
	if [ "$$software" = 0 ] && [ "$(SOFTWARE)" = auto ]; then \
		echo; \
		echo "make run: the GPU path failed (nested EGL on this driver) — retrying with llvmpipe."; \
		echo "          'make run SOFTWARE=1' skips the attempt; 'SOFTWARE=0' keeps it hard."; \
		launch 1; \
	else \
		exit $$status; \
	fi

r: run

nested: run

# On real hardware, from a free VT (Ctrl+Alt+F5, logged in).
tty: build
	@echo
	@echo "From a bare TTY, with no WAYLAND_DISPLAY/DISPLAY set:"
	@echo
	@echo "  LIBSEAT_BACKEND=logind $(TOP_DIR)/target/debug/$(APP_BIN) $(CLIENT) 2>~/0xin-tty.log"
	@echo
	@echo "The modifier is the real Super key there. Ctrl+Alt+F1 gets you back."

test:
	@$(NIX_RUN) $(CARGO) test

t: test

check:
	@$(NIX_RUN) $(CARGO) check --all-targets

fmt:
	@$(NIX_RUN) $(CARGO) fmt --all

clippy:
	@$(NIX_RUN) $(CARGO) clippy --all-targets

clean:
	@$(NIX_RUN) $(CARGO) clean

help:
	@echo
	@echo "Usage: make [target]"
	@echo
	@echo "Available targets:"
	@echo "  build        Build $(APP_BIN) and 0xinctl"
	@echo "  compile      Clean and rebuild"
	@echo "  run          Run 0xin nested in a window ($(BACKEND) backend, $(RUN_WITH) wrapper)"
	@echo "  tty          Print how to run on real hardware (DRM/KMS on a VT)"
	@echo "  test         Run the test suite"
	@echo "  check        Type-check all targets"
	@echo "  fmt          Format the tree"
	@echo "  clippy       Lint all targets"
	@echo "  clean        Remove Cargo build artifacts"
	@echo
	@echo "Examples:"
	@echo "  make run"
	@echo "  make run CLIENT=foot          # launch a different client inside 0xin"
	@echo "  make run CLIENT=              # empty compositor, no client"
	@echo "  make run BACKEND=x11          # open the nested window on X11/XWayland"
	@echo "  make run BACKEND=wayland      # force native Wayland"
	@echo "  make run MOD=super            # use Super instead of Alt for binds"
	@echo "  make run RUN_WITH=            # no GL wrapper (NixOS, /run/opengl-driver)"
	@echo "  make run SOFTWARE=1           # force llvmpipe (no GPU passthrough)"
	@echo "  make run SOFTWARE=0           # insist on the GPU via $(RUN_WITH)"
	@echo "  make run ARGS=--verbose       # extra arguments for 0xin itself"
	@echo "  make build NIX_RUN=           # already in a shell with the deps"
	@echo

h: help
