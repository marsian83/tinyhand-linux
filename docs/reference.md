# Reference

Everything beyond the [README](../README.md): the commands, click for real, the settings
window, the config file and where TinyHand looks for hands.

## Commands

```
tinyhand [on|off|toggle]       turn the pointer on / off
tinyhand clear                 clear the drawing
tinyhand real [on|off]         let clicks through to apps (no argument toggles)
tinyhand hand NAME|PATH        switch hand while it's on (e.g. coral, robot, laser)
tinyhand sound [on|off|NAME]   tap sound on/off, or pick thock, click, pop, wood, boop, squeak
tinyhand hands                 list the 36 built-in hands and your own
tinyhand prompt [IDEA]         print a prompt for ChatGPT or another image tool to draw a hand
tinyhand make-hand IMAGE [NAME]  check that image, put it on a stick and add it as a hand
tinyhand remove-hand ID        delete one of your own hands
tinyhand setup                 print key bindings for your desktop
tinyhand settings              open the settings window
```

Commands reach the running pointer through a socket in `$XDG_RUNTIME_DIR`. When the
pointer is off they do nothing (except `on`/`toggle`, which start it).

## Click for real (hold a key)

Wayland doesn't let an app watch the keyboard while another app has focus, so "hold Alt
to click for real" is a pair of bindings: one on press, one on release. Hyprland can bind a
key without taking it away from apps:

```lua
-- ~/.config/hypr/hyprland.lua (Lua config)
hl.bind("Alt_L", hl.dsp.exec_cmd("tinyhand real on"), { non_consuming = true })
hl.bind("ALT + Alt_L", hl.dsp.exec_cmd("tinyhand real off"), { non_consuming = true, release = true })
```

```ini
# ~/.config/hypr/hyprland.conf (classic config)
bindn = , Alt_L, exec, tinyhand real on
bindrn = ALT, Alt_L, exec, tinyhand real off
```

On other compositors bind `tinyhand real` (a toggle) to a key such as **Ctrl+Alt+R**.

## Settings window

`tinyhand settings` (or **TinyHand Settings** in your app menu) opens a GTK 4 /
libadwaita window with the same sections as the macOS app:

- **Pointers**: click a hand (or the laser dot) to use it, pin up to 8 to the toolbar and
  drag them into order, and **make your own hand**: describe it, open the prompt in
  ChatGPT (or copy it for another image tool), then drop the PNG in to see it on its
  stick, name it and add it. Your own hands can be renamed and deleted here too.
- **Look**: size and shadow, and whether scrolling reaches the window underneath.
- **Drawing**: ink colour, how long lines last, and the toolbar.
- **Sound**: the tap sound (click one to hear it) and the laser dot's tap.
- **Shortcuts**: the key-binding lines for your desktop (what `tinyhand setup` prints),
  ready to copy, and a button to try the pointer.
- **About**.

Changes are written to the config file right away, and a running pointer picks them up
at once. The window is a separate program (`tinyhand-settings`) that runs only while it's
open, so the pointer itself stays as small as before. It needs GTK 4.12 and libadwaita
1.4 or later (Ubuntu 24.04, Debian 13, Fedora 40 and newer).

## Config file

`~/.config/tinyhand/config`, one `key = value` per line. Command-line options
(`--hand`, `--size`, `--no-shadow`, `--no-sound`, `--tap-sound`, `--fade`, `--ink`)
override it.

```ini
hand = lavender          # one of `tinyhand hands`, "laser", or a path to a PNG
size = 0.13              # hand height as a fraction of the screen height (0.08–0.20)
shadow = true
sound = true             # tap sound for hands
laser_sound = false      # tap sound for the laser dot
tap_sound = thock        # thock, click, pop, wood, boop, squeak
fade_seconds = 3         # 0 keeps drawings until you clear them (laser trails always fade)
ink = match              # match (the hand's colour), red, orange, yellow, green, blue, purple, white, black, or #rrggbb
scroll_through = true    # let the scroll wheel reach the window underneath
toolbar = lavender, glove, coral, plush, robot, laser   # or: toolbar = off
```

## Your own hands

Run `tinyhand prompt knitted wool in mustard yellow` (any style you like), paste the
prompt into ChatGPT or another image tool, save the PNG it draws and run
`tinyhand make-hand that.png "Wool"`. TinyHand checks the image (transparent background,
one upright hand with the finger at the top), puts it on a stick, picks an ink colour and
prints the new hand's id (`custom-…`), which works anywhere a hand name does. Your hands
are kept in `~/.local/share/tinyhand/custom/`.

You can also pass the path of any PNG laid out like the built-in hands (a right hand
pointing up, on a transparent background, ending in a stick at the bottom) as the hand.

## Where the hands come from

TinyHand looks for hands in `$TINYHAND_ASSETS`, `assets/` next to the binary,
`<prefix>/share/tinyhand/hands` next to the installed binary,
`$XDG_DATA_HOME/tinyhand/hands`, and `$XDG_DATA_DIRS/tinyhand/hands`.

A debug build (`cargo run`) finds the hands in `assets/hands` by itself; for a release
build run from the checkout, set `TINYHAND_ASSETS=$PWD/assets/hands`. To install by hand,
copy `target/release/tinyhand` onto your `PATH` and `assets/hands` to
`/usr/share/tinyhand/hands/`.

The hands in `assets/hands` are the original app's, made smaller by
`scripts/prepare-hands.py` (about 65 MB down to about 12 MB). It clears hidden pixel
data and trims empty columns; it doesn't scale or recolour anything.

## Toolbar colours

The toolbar takes its light or dark look and its accent colour from your desktop, through
the desktop portal (`org.freedesktop.appearance`, which GNOME, KDE Plasma, COSMIC and the
GTK portal answer). Without an answer it stays dark. `TINYHAND_THEME=light` or
`TINYHAND_THEME=dark,#3584e4` sets it by hand.

## Sound

Tap sounds are built into the binary. They play through ALSA (`libasound.so.2`), which
also reaches PipeWire and PulseAudio. Without it TinyHand stays silent.
