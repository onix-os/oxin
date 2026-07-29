# Stage 15 — Edge Workspace Gestures

**What it is.** This stage adds opt-in one-finger side-edge gestures for
cycling through 0xin workspaces on a touch-first device.

**Phase gate.** *On the FP5 reference profile, an inward swipe from the left
edge selects the previous workspace and an inward swipe from the right edge
selects the next.*

## Ownership and thresholds

A touch must begin within 28 logical pixels of an output's left or right edge.
Once captured, it belongs to the compositor for the full touch sequence:

- dragging at least 70 logical pixels rightward from the left edge selects the
  previous workspace;
- dragging at least 70 logical pixels leftward from the right edge selects the
  next workspace;
- lifting before the threshold performs no action.

The narrow start region is the important policy boundary. Horizontal scrolling,
carousels, and Firefox gestures that begin elsewhere continue through native
Wayland touch unchanged. The keyboard handle is checked first, so its bottom
gesture remains available independently.

While the virtual keyboard is visible, the side-edge strips stop at its top
edge. The keyboard owns its entire surface, including edge-column keys such as
Tab, Backspace, P, and Return; workspace swipes remain available in the
application area above it.

## Workspace behavior

The gesture calls the same workspace-switching function as keyboard bindings.
It therefore preserves per-workspace layout and focus, updates scene
visibility, and follows the existing multi-output swap rule. The nine
workspaces form a ring: previous from workspace 1 selects workspace 9, while
next from workspace 9 selects workspace 1.

## Configuration

Stage 16 expresses the opt-in behavior as mappings to shared actions:

```ini
gesture = edge-left-in, workspaceprev
gesture = edge-right-in, workspacenext
```

The FP5 reference profile enables it. Desktop configurations default to
no gesture mappings, keeping the full touchscreen surface available to
applications. Desktop users can invoke the same actions from keys:

```ini
bind = MOD, bracketleft, workspaceprev
bind = MOD, bracketright, workspacenext
```

Earlier mobile testing demonstrated native application gestures but did not
include compositor workspace gestures; Stage 12 explicitly deferred that
policy. Stage 15 is the first implementation of edge workspace switching.
