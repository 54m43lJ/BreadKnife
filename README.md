# BreadKnife

A fully customizable desktop shell built with Rust and GTK4 — no compromises, no black boxes. (Only support Hyprland)

## Build from Source

### Arch Linux

```bash
sudo pacman -S rustup gtk4 gtk4-layer-shell gcc
```

Other distributions: install GTK4 (>= 4.14) and gtk4-layer-shell build dependencies for your distribution.

### Optional: GTK API surface

The crate defaults to the GTK 4.14 API floor (CI compatible). On systems with newer GTK, build with more API enabled:

```bash
cargo build --features gtk4_22   # GTK 4.22 API (current Arch system version)
```

### Tray library

The system tray module is an independent crate under `tray/` (headless SNI + DBusMenu + fallback watcher). See `tray/docs/SPEC.md`, and observe it without any UI via:

```bash
cargo run -p tray --bin tray-inspect -- watch
```

## Credits

- [gtk4-rs](https://github.com/gtk-rs/gtk4-rs) — Rust bindings for GTK4
- [zbus](https://github.com/dbus2/zbus) — Rust DBus implementation (tray library)

## License

MIT
