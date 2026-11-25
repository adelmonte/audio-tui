# audio-tui

A terminal mixer and control panel for PipeWire, written in Rust with [ratatui](https://ratatui.rs). It covers what pavucontrol does, plus device renaming, virtual devices, sample rate and WirePlumber's per-app memory.

- Volume and mute for apps, outputs and inputs, with live level meters
- Move apps between devices, set defaults, choose ports, per-channel volume
- Card profiles, including Bluetooth codecs (LDAC, AAC, SBC, ...)
- Rename or disable devices system-wide
- Per-app routing rules
- Virtual devices: null sink, combined output, loopback, echo cancellation, noise suppression, equalizer
- Sample rate, buffer size and WirePlumber settings
- View, forget or disable what WirePlumber remembers about each app
- Mouse and keyboard throughout

## Install

### Arch Linux

A PKGBUILD is in [`packaging/`](packaging/PKGBUILD):

```sh
git clone https://github.com/adelmonte/audio-tui
cd audio-tui/packaging
makepkg -si
```

### From source

Needs Rust 1.85+, clang and the PipeWire headers.

```sh
cargo build --release
install -Dm755 target/release/audio-tui ~/.local/bin/audio-tui
```

At runtime it needs PipeWire and WirePlumber 0.5+. The noise-suppressed mic needs `noise-suppression-for-voice`.

## Usage

```
audio-tui [--demo] [-c|--config PATH] [--print-default-config] [--apply]
```

| Option | |
|---|---|
| `--demo` | Run with a fake audio graph |
| `-c`, `--config PATH` | Use a config file other than `~/.config/audio-tui/config.toml` |
| `--print-default-config` | Print the default config and exit |
| `--apply` | Write the PipeWire/WirePlumber drop-ins from the config and exit |

### Keys

| Key | Action |
|---|---|
| `1`–`8`, `Tab` | Switch tab |
| `j` / `k`, `↓` / `↑` | Move |
| `h` / `l`, `←` / `→` | Volume down / up (`<` / `>` for 1%) |
| `m` | Mute |
| `Enter` | Send app to a device / make device default |
| `c` | Per-channel volume |
| `Ctrl-S` | Apply system changes |
| `:`, `Ctrl-P` | Command palette |
| `?` | All keys |

## Configuration

`~/.config/audio-tui/config.toml` is created on first run with every option commented. System changes are written as `60-audio-tui-*.conf` drop-ins under `~/.config/pipewire/` and `~/.config/wireplumber/`; no other files are modified.

## License

GPL-3.0-or-later
