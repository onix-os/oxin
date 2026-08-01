# 0xin — one-command dev loop.
#
# The important target is `make run`: it starts 0xin nested, as a window inside
# your current Wayland session, with a terminal already running in it so there
# is something to look at and tile.
#
#   make run                 # nested window + default client
#   make run CLIENT=foot     # nested window + a specific client
#   make run CLIENT=         # nested window, no client
#   make run ARGS=--verbose  # extra args for 0xin itself
#   make tty                 # notes for running on a real TTY (DRM/KMS)

CARGO ?= cargo
BIN := target/debug/0xin

# On a non-NixOS host the compositor needs the driver's EGL/GLES userspace put
# in front of it — that is what the flake's nixGL wrapper does. Deliberately
# NOT named RUN_WITH: .envrc exports that for Vulkan apps (nixLavapipe), and
# the nested backend is GLES, so it needs the GL wrapper instead.
# Override with `make run GL_WRAP=` to run unwrapped.
GL_WRAP ?= $(shell command -v nixGL 2>/dev/null)

# A client to launch inside 0xin, so the window is not empty. First terminal
# found wins; override with CLIENT= to pick one, or CLIENT= (empty) for none.
CLIENT ?= $(shell command -v ghostty kitty foot alacritty 2>/dev/null | head -n1)

# The host compositor grabs Super-chords before we ever see them, so nested
# development uses Alt as the modifier.
MOD ?= alt

ARGS ?=

.PHONY: all build compile run nested test fmt clippy clean tty help

all: build

## Build the compositor and 0xinctl.
build:
	$(CARGO) build

## Type-check only — much faster than a full build.
compile:
	$(CARGO) check

## Run 0xin nested, in a window inside the current Wayland session.
run: build
	@if [ -z "$$WAYLAND_DISPLAY" ] && [ -z "$$DISPLAY" ]; then \
		echo "make run: no WAYLAND_DISPLAY or DISPLAY — you are not in a session."; \
		echo "          On a bare TTY use 'make tty' instead."; \
		exit 1; \
	fi
	OXIN_MOD=$(MOD) $(GL_WRAP) ./$(BIN) $(ARGS) $(CLIENT)

## Same thing, matching the `cargo nested` alias.
nested: run

test:
	$(CARGO) test

fmt:
	$(CARGO) fmt

clippy:
	$(CARGO) clippy --all-targets

clean:
	$(CARGO) clean

## Running on real hardware, from a free VT (Ctrl+Alt+F5, logged in).
tty: build
	@echo "From a bare TTY, with no WAYLAND_DISPLAY/DISPLAY set:"
	@echo
	@echo "  LIBSEAT_BACKEND=logind $(PWD)/$(BIN) $(CLIENT) 2>~/0xin-tty.log"
	@echo
	@echo "The modifier is the real Super key there. Ctrl+Alt+F1 gets you back."

help:
	@grep -B1 -E '^[a-z-]+:' $(MAKEFILE_LIST) \
		| grep -A1 '^##' \
		| sed 's/^## //; s/:.*//' \
		| paste - - \
		| sort
