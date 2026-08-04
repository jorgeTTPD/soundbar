# soundbar

Cava-style audio spectrum visualizer for [polybar](https://github.com/polybar/polybar),
written in Rust. It is a fork of [lookas](https://github.com/rccyx/lookas) with the
terminal UI removed: instead of drawing on screen, it prints the same raw output
that [cava](https://github.com/karlstav/cava) produces (`0-7` values separated by `;`).

It is designed for low resource usage: **~4 MB of RAM** while cava uses **~15 MB**
(and it ships as a single static-ish binary of ~1.7 MB).

## Features

- Raw output identical to cava: `v;v;...;v\n` per frame (default `0-7`), ready for polybar `custom/script` modules.
- System audio by default, with fallback to microphone; `--mic` and `--both` modes available.
- Adaptive sensitivity (gate, flow/spring smoothing) inherited from lookas.
- Configurable bars, levels and framerate via CLI flags.
- PipeWire/PulseAudio/ALSA capture through `cpal`.

## Usage

```
soundbar [options]

Options:
  --bars N      number of bars (default 8)
  --levels N    max value per bar, 1-9 (default 7, same as cava)
  --fps N       frames per second, 20-125 (default 60)
  --system      system audio (default; falls back to mic if it fails)
  --mic         microphone audio
  --both        mix system + microphone
  --help        show help
```

Output is one line per frame with values separated by `;`.

### Install

```sh
cargo build --release
cargo install --path .
```

### Polybar integration

Create a wrapper script that pipes `soundbar` into a FIFO and maps the `0-7`
values to `▂▃▄▅▆▇█` characters, exactly like the classic `cava.sh`, then use it
in your bar config:

```ini
[module/soundbar]
type = custom/script
exec = ~/.config/polybar/scripts/soundbar.sh
tail = true
format = <label>
label = %output%
```

Example wrapper script:

```sh
#!/usr/bin/env bash

FIFO="/tmp/soundbar.fifo"
PIDFILE="/tmp/soundbar.pid"

if [ ! -p "$FIFO" ]; then
    mkfifo "$FIFO"
fi

if ! kill -0 "$(cat "$PIDFILE" 2>/dev/null)" 2>/dev/null; then
    soundbar --bars 8 > "$FIFO" 2>&1 &
    echo $! > "$PIDFILE"
fi

while read -r line < "$FIFO"; do
    bars=" ▂▃▄▅▆▇█"
    output=""
    IFS=';' read -ra values <<< "$line"
    for v in "${values[@]}"; do
        if [ "$v" -ge 0 ] && [ "$v" -le 7 ]; then
            output+="${bars:$v:1}"
        else
            output+=" "
        fi
    done
    echo "$output"
done
```


## RAM comparison

| Tool    | RSS  |
|---------|------|
| cava    | ~15 MB |
| soundbar| ~4 MB  |

## Credits

Based on [lookas](https://github.com/rccyx/lookas) (MIT), by taking its DSP/audio
pipeline and replacing the TUI with cava-compatible raw output.

## License

MIT
