-- Fairphone 5 standalone touch-test profile.
local oxin = require("oxin")

oxin.modifier       = "super"
oxin.gap            = 5
oxin.first_split    = "horizontal"
oxin.background     = { 0.04, 0.04, 0.06 }
oxin.wallpaper      = "~/pics/286257.jpg"
oxin.window_opacity = 0.95
-- Real per-pixel corner masking (Stage 26). Re-enabled to retest after fixing
-- a likely root cause (alpha-less textures read garbage from their sampled
-- alpha channel on this GPU/driver) and adding real diagnostics
-- (~/.local/state/0xin-touch-test.log). See
-- docs/phases/stage-26-rounded-window-corners.md.
oxin.corner_radius  = 40

oxin.monitors["DSI-1"] = { x = 0, y = 0, scale = 3 }

-- Optional virtual-keyboard controller shared by key and gesture actions.
oxin.keyboard = {
  show   = "pkill -USR2 -x patin-osk",
  hide   = "pkill -USR1 -x patin-osk",
  -- patin-osk --keypad=extended's logical footprint height
  -- (see patin_keyboard::footprint_height).
  height = 262,
}

-- Keep the bottom gesture target but omit its training-wheel pill.
oxin.gesture_handle = "hidden"

-- Session shell and clients. Each starts once after 0xin's Wayland socket is
-- ready; registration order is preserved.
local function autostart(cmd)
  oxin.on.startup(function() oxin.spawn(cmd) end)
end

autostart("~/.local/bin/patin")
autostart("~/.local/bin/patin-workspaces-bar")
autostart("~/.local/bin/patin-osk --keypad=extended --hidden")
autostart("~/.local/bin/0xin-auto-rotate")

-- Physical touch triggers map to the same actions keyboard binds use.
oxin.gestures["bottom-up"]       = oxin.action.keyboard_show
oxin.gestures["bottom-down"]     = oxin.action.keyboard_hide
oxin.gestures["edge-left-in"]    = oxin.action.spawn("wtype -M alt -k Left -m alt")
oxin.gestures["edge-right-in"]   = oxin.action.spawn("wtype -M alt -k Right -m alt")
oxin.gestures["edge-left-up"]    = oxin.action.spawn("pactl set-sink-volume @DEFAULT_SINK@ +5%")
oxin.gestures["edge-left-down"]  = oxin.action.spawn("pactl set-sink-volume @DEFAULT_SINK@ -5%")
oxin.gestures["edge-right-up"]   = oxin.action.workspace_next
oxin.gestures["edge-right-down"] = oxin.action.workspace_prev
oxin.gestures["top-right"]       = oxin.action.spawn("brightnessctl set +5%")
oxin.gestures["top-left"]        = oxin.action.spawn("brightnessctl set 5%-")
oxin.gestures["top-down"]        =
  oxin.action.spawn("pgrep -x patin-launcher >/dev/null || ~/.local/bin/patin-launcher")

-- Multi-finger window management, available across the central app surface.
oxin.gestures["two-up"]      = oxin.action.move("up")
oxin.gestures["two-down"]    = oxin.action.move("down")
oxin.gestures["two-left"]    = oxin.action.move("left")
oxin.gestures["two-right"]   = oxin.action.move("right")
oxin.gestures["three-up"]    = oxin.action.close
oxin.gestures["three-down"]  = oxin.action.close
oxin.gestures["three-left"]  = oxin.action.move_to_workspace_prev
oxin.gestures["three-right"] = oxin.action.move_to_workspace_next
oxin.gestures["double-tap"]  = oxin.action.solo

-- Hardware volume keys are repurposed as quick-launch buttons — volume already
-- has the left-edge swipe gesture, so these standard xkb key names (rather than
-- FP5 event-device codes, for portability to other hardware) are free.
oxin.keys["XF86AudioRaiseVolume"] = oxin.action.spawn("~/.local/bin/toggle-flashlight")
oxin.keys["XF86AudioLowerVolume"] = oxin.action.spawn("~/.local/bin/phone-terminal")

-- Release the power key before two seconds to start Patin's touch-capable
-- session lock. Repeated short presses leave the existing locker alone.
oxin.keys["XF86PowerOff"] = oxin.action.spawn("pgrep -x patin-lock >/dev/null || patin-lock")
oxin.hold["XF86PowerOff"] = { ms = 2000, action = oxin.action.spawn("~/.local/bin/0xin-session-menu") }
