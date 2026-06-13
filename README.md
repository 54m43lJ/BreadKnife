# BreadKnife

A fully customizable desktop shell built with Rust, GTK4, and Astal — no compromises, no black boxes.

## Philosophy

BreadKnife is a desktop shell for people who want complete control over their environment. Every pixel, every animation, every interaction is defined by you in code. Unlike traditional shells that sit on top of a heavyweight desktop environment, BreadKnife is the environment: a standalone compositor-aware shell layer that owns bars, launchers, notifications, overlays, and everything in between.

- **You own it.** Nothing is hidden behind compiled-in defaults or opaque daemons.
- **Rust all the way down.** Type safety, zero-cost abstractions, fearless concurrency.
- **GTK4 for rendering.** Battle-tested widget toolkit with GPU-accelerated rendering via NGL.
- **Astal for shell primitives.** Batteries-included shell widgets (bars, drawers, notifications, app launchers) wired to Wayland protocols.

## Tech Stack

| Layer | Technology |
|-------|------------|
| Language | Rust |
| GUI Toolkit | GTK4 (gtk-rs) |
| Shell Framework | Astal (astal-rs) |
| Layer Shell | wlr-layer-shell / ext-layer-shell |
| Compositor Protocol | Wayland (via smithay-client-toolkit) |
| CSS Styling | GTK4 CSS Provider |

## Features (Planned)

- [ ] **Bar / Panel** — multi-monitor, per-output configuration
- [ ] **App Launcher** — fuzzy-search, keyboard-driven
- [ ] **Notification Center** — grouped, actionable, history
- [ ] **OSD Overlays** — volume, brightness, keyboard layout
- [ ] **Workspace Switcher** — EWMH + wlr-ext-workspace
- [ ] **System Tray** — StatusNotifierItem (SNI) protocol
- [ ] **Lock Screen** — pam-based authentication
- [ ] **Power Menu** — suspend, reboot, shutdown
- [ ] **Configuration** — compile-time checked, no runtime parsing errors

## Building

### Dependencies

```bash
# Arch Linux
sudo pacman -S rust gtk4 gtk4-layer-shell wireplumber

# Ubuntu / Debian
sudo apt install cargo libgtk-4-dev libgtk4-layer-shell-dev libwireplumber-dev

# Fedora
sudo dnf install cargo gtk4-devel gtk4-layer-shell-devel wireplumber-devel
```

### Build & Run

```bash
cargo build --release
cargo run --release
```

## Architecture

```
┌─────────────────────────────────┐
│           Compositor            │
│  (wlroots / Sway / Hyprland)    │
└──────────┬──────────────────────┘
           │ Wayland protocols
┌──────────▼──────────────────────┐
│       BreadKnife Shell          │
│  ┌──────────┐  ┌────────────┐   │
│  │   Bar    │  │  Launcher  │   │
│  └──────────┘  └────────────┘   │
│  ┌──────────┐  ┌────────────┐   │
│  │Notifications│  Overlays  │   │
│  └──────────┘  └────────────┘   │
│          Astal + GTK4           │
└─────────────────────────────────┘
```

Each shell component is an independent GTK4 window managed by the layer-shell protocol. Astal provides the shell-specific widget abstractions and protocol bindings, while GTK4 handles rendering, input, and CSS theming.

## License

MIT
