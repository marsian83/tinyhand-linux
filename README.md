<p align="center"><img src="packaging/icons/tinyhand.png" width="96" alt="" /></p>

<h1 align="center">TinyHand for Linux</h1>

<p align="center"><strong>A tiny hand on a stick that points at things on your screen.</strong></p>

<p align="center">
  <a href="https://github.com/marsian83/tinyhand-linux/actions/workflows/ci.yml"><img src="https://github.com/marsian83/tinyhand-linux/actions/workflows/ci.yml/badge.svg" alt="CI status" /></a>
  <a href="https://aur.archlinux.org/packages/tinyhand-linux-git"><img src="https://img.shields.io/aur/version/tinyhand-linux-git?color=E8735A&label=AUR" alt="AUR package" /></a>
  <a href="LICENSE"><img src="https://img.shields.io/github/license/marsian83/tinyhand-linux?color=E8735A" alt="MIT license" /></a>
  <img src="https://img.shields.io/badge/Wayland_%2B_X11-555" alt="Runs on Wayland and X11" />
  <img src="https://img.shields.io/badge/Rust-000?logo=rust&logoColor=white" alt="Written in Rust" />
</p>

<p align="center">
  <a href="#install"><strong>Install</strong></a>
  &nbsp;·&nbsp;
  <a href="#set-up-the-shortcut">Shortcut</a>
  &nbsp;·&nbsp;
  <a href="#use">How to use it</a>
  &nbsp;·&nbsp;
  <a href="#differences-from-the-macos-app">Differences from macOS</a>
  &nbsp;·&nbsp;
  <a href="#resource-use">Resource use</a>
</p>

When you share your screen, a mouse cursor is a tiny arrow nobody follows. TinyHand turns
it into a hand on a stick: click to tap (with a little sound), drag to draw on the
screen, and hold <kbd>Alt</kbd> when you need to click something for real.

<p align="center">
  <img src="docs/images/demo.gif" width="100%" alt="The TinyHand pointer tapping and drawing over a code editor, then switching hands from the toolbar" />
</p>

This is a Linux port of [TinyHand](https://tinyhand.app) by
[Kamran Ahmed](https://github.com/kamranahmedse) ([nilbuild/tinyhand](https://github.com/nilbuild/tinyhand)).
The hands, the icon, the tap sounds and the way the arm moves all come from the
original macOS app. The port rewrites it in Rust for Wayland and X11, and its toolbar
follows your desktop's light or dark style and accent colour.

> [!NOTE]
> **Unofficial port.** It is not affiliated with or endorsed by the original author.
> Please report Linux problems [here](https://github.com/marsian83/tinyhand-linux/issues),
> not on the original project.

## Supported desktops

| Desktop | Session | How it draws | Status |
|---|---|---|---|
| Hyprland | Wayland | wlr-layer-shell | Tested |
| KDE Plasma, Sway, niri, river, COSMIC, Wayfire, labwc | Wayland | wlr-layer-shell | Should work (same protocols; not yet tested) |
| Xfce, Cinnamon, MATE, i3, KDE on X11, … | X11 | override-redirect windows | Tested on a headless Xorg; not yet on real desktops |
| GNOME | Wayland | X11 backend through XWayland (GNOME has no layer-shell) | Untested; "hold Alt" only works over X11 apps, use `tinyhand real` |

TinyHand picks Wayland when the compositor offers wlr-layer-shell and falls back to X11
otherwise. `TINYHAND_BACKEND=x11` or `=wayland` forces one.

## Install

### Arch Linux (AUR)

```sh
yay -S tinyhand-linux-git      # or: paru -S tinyhand-linux-git
```

Or build the package from a checkout: `cd packaging/arch && makepkg -si`.

### Prebuilt tarball (any distro)

Download `tinyhand-linux-<arch>.tar.gz` from the [latest release](https://github.com/marsian83/tinyhand-linux/releases/latest), then:

```sh
tar xf tinyhand-linux-x86_64.tar.gz
cd tinyhand-linux-x86_64
./install.sh                # into ~/.local (no root needed)
sudo ./install.sh --system  # or into /usr/local
./install.sh --uninstall    # remove it again (add --system if you installed it there)
```

`~/.local/bin` has to be on your `PATH`. The binary needs glibc 2.28 or later. The CI
tarball has the pointer only; `install.sh` also installs `tinyhand-settings` when it sits
next to it (or in `target/release` of a checkout).

### Debian / Ubuntu (.deb)

```sh
cargo install cargo-deb
cargo build --release --workspace  # needs libgtk-4-dev and libadwaita-1-dev (settings window)
cargo deb --no-build
sudo apt install ./target/debian/tinyhand_*.deb
```

### Fedora / openSUSE (.rpm)

```sh
cargo install cargo-generate-rpm
cargo build --release --workspace  # needs gtk4-devel and libadwaita-devel (settings window)
cargo generate-rpm
sudo dnf install ./target/generate-rpm/tinyhand-*.rpm
```

### From source

You need a stable Rust toolchain:

```sh
cargo build --release
./target/release/tinyhand toggle
```

That builds only the pointer, which needs no GTK. The settings window is a second program
in the same workspace; it needs the GTK 4 and libadwaita development packages:

```sh
cargo build --release -p tinyhand-settings
./target/release/tinyhand settings
```

A debug build (`cargo run`) finds the hands in `assets/hands` by itself; for a release
build from the checkout, set `TINYHAND_ASSETS=$PWD/assets/hands`. To install by hand,
copy `target/release/tinyhand` onto your `PATH` and `assets/hands` to
`/usr/share/tinyhand/hands/`, or run `packaging/install.sh`.

The hands in `assets/hands` are the original app's, made smaller by
`scripts/prepare-hands.py` (about 65 MB down to about 12 MB). It clears hidden pixel
data and trims empty columns; it doesn't scale or recolour anything.

Tap sounds are built into the binary. They play through ALSA (`libasound.so.2`), which
also reaches PipeWire and PulseAudio. Without it TinyHand stays silent.

## Set up the shortcut

There is no menu bar icon and nothing runs in the background. `tinyhand toggle` starts
the pointer, and running it again ends the process. **`tinyhand setup` prints the exact
lines for the desktop you're on.** Wayland doesn't let apps grab global
keys, so bind the commands in your desktop's settings. The suggested keys match the
macOS app:

| Key | Command |
|---|---|
| **Ctrl+Alt+H** | `tinyhand toggle` |
| **Ctrl+Alt+X** | `tinyhand clear` |

**Hyprland** (`~/.config/hypr/hyprland.conf`)

```ini
bind = CTRL ALT, H, exec, tinyhand toggle
bind = CTRL ALT, X, exec, tinyhand clear
layerrule = noanim, tinyhand
```

Newer Hyprland with a Lua config (`~/.config/hypr/hyprland.lua`):

```lua
hl.bind("CTRL + ALT + H", hl.dsp.exec_cmd("tinyhand toggle"), { description = "TinyHand pointer" })
hl.bind("CTRL + ALT + X", hl.dsp.exec_cmd("tinyhand clear"), { description = "TinyHand: clear drawing" })
hl.layer_rule({ match = { namespace = "tinyhand" }, no_anim = true })
```

**Sway** (`~/.config/sway/config`)

```
bindsym Ctrl+Alt+h exec tinyhand toggle
bindsym Ctrl+Alt+x exec tinyhand clear
```

**niri** (`~/.config/niri/config.kdl`)

```kdl
binds {
    Ctrl+Alt+H { spawn "tinyhand" "toggle"; }
    Ctrl+Alt+X { spawn "tinyhand" "clear"; }
}
```

**river** (`~/.config/river/init`)

```sh
riverctl map normal Control+Alt H spawn 'tinyhand toggle'
riverctl map normal Control+Alt X spawn 'tinyhand clear'
```

**KDE Plasma**

1. Open **System Settings > Keyboard > Shortcuts**.
2. Click **Add New > Command or Script...**, enter `tinyhand toggle` and press **Add**.
3. Select the new entry, click **Add custom shortcut** and press **Ctrl+Alt+H**.
4. Do the same for `tinyhand clear` with **Ctrl+Alt+X**.

If you installed a package, you can also use **Add New > Application...** and pick
TinyHand, which runs `tinyhand toggle`.

**GNOME**

GNOME has no layer-shell, so TinyHand uses its X11 backend through XWayland there.
Add the shortcuts in **Settings > Keyboard > Keyboard Shortcuts > Custom Shortcuts**,
or from a terminal:

```sh
KEY=/org/gnome/settings-daemon/plugins/media-keys/custom-keybindings
SCHEMA=org.gnome.settings-daemon.plugins.media-keys.custom-keybinding

gsettings set $SCHEMA:$KEY/tinyhand-toggle/ name 'TinyHand'
gsettings set $SCHEMA:$KEY/tinyhand-toggle/ command 'tinyhand toggle'
gsettings set $SCHEMA:$KEY/tinyhand-toggle/ binding '<Control><Alt>h'
gsettings set $SCHEMA:$KEY/tinyhand-clear/ name 'TinyHand: clear drawing'
gsettings set $SCHEMA:$KEY/tinyhand-clear/ command 'tinyhand clear'
gsettings set $SCHEMA:$KEY/tinyhand-clear/ binding '<Control><Alt>x'

# This replaces your list of custom shortcuts. If you already have some, run
# `gsettings get org.gnome.settings-daemon.plugins.media-keys custom-keybindings`
# and add the two paths below to what it prints instead.
gsettings set org.gnome.settings-daemon.plugins.media-keys custom-keybindings \
  "['$KEY/tinyhand-toggle/', '$KEY/tinyhand-clear/']"
```

## Use

| | |
|---|---|
| **Ctrl+Alt+H** (`tinyhand toggle`) | Turn the pointer on or off |
| Click | Tap (nothing underneath gets clicked) |
| Hold and drag | Draw on the screen (fades after 3 s by default) |
| Hold **Alt** and click | Click the thing underneath for real (needs the binding below) |
| Scroll | Scrolls the window underneath |
| **Ctrl+Alt+X** (`tinyhand clear`) | Clear the drawing |
| Toolbar at the bottom | Pick a pinned hand or the laser dot, mute the tap, or exit |

`tinyhand setup` prints the right key bindings for the desktop you're on.

### Commands

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

### Click for real (hold a key)

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

### Settings window

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

### Config file

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

Make your own hand: run `tinyhand prompt knitted wool in mustard yellow` (any style you
like), paste the prompt into ChatGPT or another image tool, save the PNG it draws and run
`tinyhand make-hand that.png "Wool"`. TinyHand checks the image (transparent background,
one upright hand with the finger at the top), puts it on a stick, picks an ink colour and
prints the new hand's id (`custom-…`), which works anywhere a hand name does. Your hands
are kept in `~/.local/share/tinyhand/custom/`.

You can also pass the path of any PNG laid out like the built-in hands (a right hand
pointing up, on a transparent background, ending in a stick at the bottom) as the hand.

TinyHand looks for hands in `$TINYHAND_ASSETS`, `assets/` next to the binary,
`<prefix>/share/tinyhand/hands` next to the installed binary,
`$XDG_DATA_HOME/tinyhand/hands`, and `$XDG_DATA_DIRS/tinyhand/hands`.

The toolbar takes its light or dark look and its accent colour from your desktop, through
the desktop portal (`org.freedesktop.appearance`, which GNOME, KDE Plasma, COSMIC and the
GTK portal answer). Without an answer it stays dark. `TINYHAND_THEME=light` or
`TINYHAND_THEME=dark,#3584e4` sets it by hand.

## Differences from the macOS app

- No menu bar icon or self-update: bind the commands to keys, change settings in
  `tinyhand settings` (or the config file), and update through your package manager.
  The settings window can't record shortcuts; it shows the lines to add to your desktop's
  key bindings instead. The toolbar has the pinned hands,
  the laser, sound and exit, but not the ink-colour tray or hint text (set `ink =` instead).
- **Esc doesn't turn the pointer off.** The overlay never takes the keyboard (so typing
  still reaches your app); use the toggle key.
- **Scrolling** reaches the window underneath, but the first notch of each scroll is
  used to let the rest through, and the hand hides for a moment while you scroll.
- **Hover effects** under the pointer (tooltips, highlighted links) don't show while the
  pointer is on, because the overlay receives the mouse.
- **Menus:** macOS lets clicks through to open menus automatically. On Linux, hold the
  click-for-real key to use menus.

## Resource use

Measured on a 2880×1800 laptop screen at 1.6× scaling, 120 Hz (Intel Core Ultra 200V,
Hyprland):

| | |
|---|---|
| While off | Nothing: no process, 0 bytes, 0 CPU |
| Memory while on | about 6 MB resident (2.7 MB proportional); about 11 MB (4.8 MB proportional) once the first tap sound has loaded the audio libraries |
| CPU while on and the mouse is still | 0 (no timers, no wakeups) |
| CPU while moving the mouse at 120 Hz | about 9 % of one core; ~0.5 ms to draw a frame |
| Start-up to first frame | about 13 ms with the prepared hands |
| Binary size | about 1.1 MB |

How it stays this small: the full-screen overlay is a single transparent pixel stretched
by the compositor; the hand, stick and ink are drawn on the CPU into small tiles that
only cover what's drawn; the shadow is drawn at quarter resolution and scaled up by the
compositor; and nothing runs between mouse events.

## License

MIT, same as the original. See [LICENSE](LICENSE).

Copyright (c) 2026 Kamran Ahmed (TinyHand, artwork and sounds).
Copyright (c) 2026 Spandan Barve (Linux port).

"TinyHand" is the name of Kamran Ahmed's app. It is used here only to say what this
port is a port of.
