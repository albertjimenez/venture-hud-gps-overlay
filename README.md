# dashcam-hud

A Rust tool that reads telemetry embedded in Vantrue dashcam footage and renders a **videogame-style transparent HUD overlay** — speedometer, G-force radar, compass bar, GPS tag, and mini-map — that can be composited over your original video.

# Disclaimer
I have used Claude Code to come up with most of the heavy logic but leveraged some rust idioms to drive the code to a maintainable state, 

```
┌────────────────────────────────────────────────────────────────────────┐
│ [G-FORCE RADAR]        [COMPASS BAR]              [GPS TAG]            │
│  top-left               top-center                 top-right           │
│                                                                        │
│ [SPEEDOMETER]                                     [MINI-MAP]           │
│  bottom-left                                       bottom-right        │
└────────────────────────────────────────────────────────────────────────┘
```

## Current Limitations

- **Venture Dashcams only** – The telemetry extractor and HUD logic are tuned for the telemetry layout embedded in Venture dashcam footage.
- **30 fps, 4K (3840×2160) only** – At the moment the renderer is hard‑wired to 30fps and the 4K canvas size.
- **Other frame rates/resolutions** – Rendering 60fps, 1080p, or any non‑4K resolution is not yet supported.
- **GoPro support** – A port for GoPro’s proprietary telemetry format is in progress and will be added in a future release thanks to exiftool.

Feel free to open an issue or pull request if you need a different fps/resolution or GoPro support sooner. Happy hacking! 🎮

---
## Screenshots

![HUD](screenshots/frame_0.png)

## Requirements

| Dependency | Purpose |
|---|---|
| [Rust](https://rustup.rs) 1.75+ | Build toolchain |
| [ExifTool](https://exiftool.org) | Telemetry extraction from MP4 |
| [FFmpeg](https://ffmpeg.org) | Video encoding / compositing |

### macOS

```bash
brew install exiftool ffmpeg
```

### Linux (Debian / Ubuntu)

```bash
sudo apt install exiftool ffmpeg
```

---

## Installation

```bash
git clone https://github.com/yourname/dashcam-hud
cd dashcam-hud
cargo build --release
# Binary is at ./target/release/dashcam-hud
```

Optionally install globally:

```bash
cargo install --path .
```

---

## Quick Start

```bash
# Burn the HUD directly onto your footage (simplest, recommended)
dashcam-hud compose clip.MP4 final.mp4

# Check what telemetry was extracted before rendering
dashcam-hud inspect clip.MP4
```

---

## Subcommands

### `compose` — burn HUD onto footage in one pass

The simplest workflow. Reads telemetry from the source file, renders the HUD, and composites it directly over the footage using alpha blending. No intermediate files needed, no black background issues.

```
dashcam-hud compose <SOURCE> <OUTPUT> [OPTIONS]
```

```bash
# Default HUD (speedometer + compass + map)
dashcam-hud compose clip.MP4 final.mp4

# Full HUD — all five widgets
dashcam-hud compose clip.MP4 final.mp4 --preset full

# Custom combo
dashcam-hud compose clip.MP4 final.mp4 --preset none --compass --gps-tag
```

---

### `render` — render HUD as a standalone overlay file

Renders the HUD to either a directory of PNG frames or a video file with an alpha channel, for manual compositing in DaVinci Resolve, Premiere Pro, Final Cut Pro, or any NLE.

```
dashcam-hud render <VIDEO> [OPTIONS]
```

```bash
# PNG frames (one per rendered frame) — manual compositing
dashcam-hud render clip.MP4 -o frames/

# ProRes 4444 .mov with full alpha — best for NLE compositing
dashcam-hud render clip.MP4 -o hud_overlay.mov

# VP9 .webm with alpha — smaller file, good for web tools
dashcam-hud render clip.MP4 -o hud_overlay.webm

# FFV1 .mkv — lossless, perfect alpha, large file
dashcam-hud render clip.MP4 -o hud_overlay.mkv

# Save both a .mov AND PNG frames simultaneously
dashcam-hud render clip.MP4 -o hud_overlay.mov --save-frames --frames-dir frames/
```

> **⚠️ Do not use `.mp4` for the overlay file.** The MP4 container cannot carry an alpha channel — the transparent background will be baked to solid black. Use `.mov`, `.webm`, or `.mkv` instead, or use the `compose` subcommand.

#### Manual compositing with FFmpeg

Once you have your `.mov` overlay:

```bash
ffmpeg -i original_clip.MP4 -i hud_overlay.mov \
  -filter_complex "[0:v][1:v]overlay=0:0:format=auto" \
  -c:a copy \
  final.mp4
```

The `format=auto` flag is required — without it FFmpeg ignores the alpha channel and you get a black box.

---

### `inspect` — debug telemetry extraction

Prints the parsed telemetry frames to stdout. Useful for verifying that ExifTool is reading your clip correctly before committing to a full render.

```
dashcam-hud inspect <VIDEO> [--count N]
```

```bash
# Show first 10 frames (default)
dashcam-hud inspect clip.MP4

# Show first 30 frames
dashcam-hud inspect clip.MP4 --count 30
```

Example output:

```
── Doc1 ──
  GPS  lat=39.983521  lon=-3.921047  speed=72.4km/h  hdg=214.0°  ts=2026:03:07 17:14:45
  Accel  x=+0.021  y=-0.043  z=-0.982

── Doc2 ──
  GPS  lat=39.983301  lon=-3.921218  speed=74.1km/h  hdg=215.0°  ts=2026:03:07 17:14:46
  Accel  x=-0.015  y=+0.062  z=-0.991
```

---

## HUD Widgets

| Widget | Position | Shows |
|---|---|---|
| **Speedometer** | Bottom-left | Speed arc (60 segments, redline zone), needle, large digital readout, progress bar |
| **G-Force Radar** | Top-left | Lateral/longitudinal G dot with history trail, concentric rings, Z-axis bar |
| **Compass Bar** | Top-center | Scrolling heading tape, cardinal markers, current heading badge, speed readout, timestamp |
| **GPS Tag** | Top-right | Latitude and longitude to 6 decimal places |
| **Mini-Map** | Bottom-right | Full route (dim), recent trail (glowing cyan with fade), current position dot, heading arrow |

---

## HUD Presets and Flags

### Presets

| Preset | Speedometer | G-Force | Compass | GPS Tag | Mini-Map |
|---|:---:|:---:|:---:|:---:|:---:|
| `default` *(default)* | ✓ | — | ✓ | — | ✓ |
| `full` | ✓ | ✓ | ✓ | ✓ | ✓ |
| `minimal` | ✓ | — | — | — | — |
| `none` | — | — | — | — | — |

```bash
dashcam-hud compose clip.MP4 out.mp4 --preset full
dashcam-hud compose clip.MP4 out.mp4 --preset minimal
```

### Individual widget flags

Any flag overrides the preset. Positive and negative flags are mutually exclusive.

| Enable | Disable |
|---|---|
| `--speedometer` | `--no-speedometer` |
| `--gforce` | `--no-gforce` |
| `--compass` | `--no-compass` |
| `--gps-tag` | `--no-gps-tag` |
| `--minimap` | `--no-minimap` |

```bash
# Full preset but without the G-force radar
dashcam-hud compose clip.MP4 out.mp4 --preset full --no-gforce

# Default preset but add the GPS tag
dashcam-hud compose clip.MP4 out.mp4 --gps-tag

# Only compass and GPS, nothing else
dashcam-hud compose clip.MP4 out.mp4 --preset none --compass --gps-tag

# Everything except the minimap
dashcam-hud compose clip.MP4 out.mp4 --preset full --no-minimap
```

---

## Render Tuning Options

These flags are available on both `render` and `compose`.

| Flag | Default | Description |
|---|---|---|
| `--width` | `3840` | Canvas width in pixels |
| `--height` | `2160` | Canvas height in pixels |
| `--fps` | `30` | Output frame rate |
| `--max-speed` | `120.0` | Maximum speed on the speedometer dial (km/h) |
| `--threads` | `0` | Render threads — `0` uses all available CPU cores |
| `--sync-offset` | `0.0` | Shift HUD data in seconds. Positive = HUD advances (fixes HUD lagging behind footage). Negative = HUD retreats. Run `inspect` to check timestamps, then adjust until speeds match. |

```bash
# 1080p output at 60fps with a 200km/h speedometer
dashcam-hud compose clip.MP4 out.mp4 \
  --width 1920 --height 1080 \
  --fps 60 \
  --max-speed 200

# Nudge HUD data forward by half a second (fixes HUD lagging behind dashcam readout)
dashcam-hud compose clip.MP4 out.mp4 --sync-offset 0.5

# Limit render to 4 threads
dashcam-hud compose clip.MP4 out.mp4 --threads 4
```

---

## Output Format Reference

| Extension | Codec | Alpha | File Size | Best For |
|---|---|---|:---:|---|
| `.mov` | ProRes 4444 | ✓ | Large | DaVinci Resolve, Premiere Pro, Final Cut |
| `.webm` | VP9 | ✓ | Medium | Web tools, Kdenlive |
| `.mkv` | FFV1 | ✓ | Very large | Lossless archival |
| `.mp4` | H.264 | ✗ | Small | **Use `compose` instead** |
| `frames/` | PNG | ✓ | Large | Manual compositing, frame-by-frame work |

---

## Architecture

```
src/
├── main.rs                    CLI — render / compose / inspect subcommands
├── pipeline.rs                Parallel render engine
│                                • Rayon thread pool for frame rendering
│                                • Producer/consumer channel → ffmpeg stdin
│                                • MinimapCache pre-computed once, shared across threads
│                                • G-history ring-buffer pre-computed per telemetry step
├── hud/
│   ├── elements.rs            HudElements struct — which widgets to draw
│   ├── interpolation.rs       Smooth interpolation between telemetry frames
│   └── renderer.rs            All five HUD widgets + compositor
└── telemetry/
    ├── extractor.rs           ExifTool wrapper — runs exiftool -ee -G3 -s -a -u
    ├── parser.rs              JSON → TelemetryFrame (DMS coords, accelerometer parsing)
    └── vantrue_frames.rs      Data models: TelemetryFrame, GpsFrame, AccelerometerFrame
```

### Performance design

- **Rayon parallel rendering** — every frame is rendered on a separate CPU core simultaneously. On an 8-core machine expect ~7× speedup over sequential.
- **Producer/consumer channel** — render threads and the ffmpeg writer thread run concurrently. Render threads never wait for writes; the writer never waits for a full batch.
- **Bounded channel** (`3 × CPU count`) — caps peak RAM usage regardless of clip length.
- **Pre-computed caches** — `MinimapCache` (GPS bounding box) and G-force history are computed once before the render loop, not on every frame.
- **Zero-copy image init** — `RgbaImage::new` zero-initialises to transparent; no extra fill loop needed.

---

## Troubleshooting

**`exiftool extraction failed`**
ExifTool is not installed or not on `PATH`. Install it with `brew install exiftool` (macOS) or `sudo apt install exiftool` (Linux).

**`Failed to spawn ffmpeg`**
FFmpeg is not installed or not on `PATH`. Install it with `brew install ffmpeg` or `sudo apt install ffmpeg`.

**No telemetry frames found / 0 GPS frames**
Your clip may not have embedded telemetry, or ExifTool doesn't recognise the format. Run `dashcam-hud inspect clip.MP4` to see what was extracted. The tool expects Vantrue dashcam files with GPS and accelerometer data embedded as ExifTool `Doc*` fields.

**HUD is black when compositing**
You used `.mp4` as the overlay format. MP4 cannot carry an alpha channel. Use `compose` subcommand, or render to `.mov` and use `overlay=0:0:format=auto` in your FFmpeg command.

**HUD elements are missing at the start of the clip**
Normal — the mini-map trail and G-force radar history need a few seconds of telemetry to build up before they show anything interesting.

**Render is slow**
- Make sure you built with `cargo build --release`, not `cargo build`.
- Use `--threads 0` (default) to use all CPU cores.
- Pipe directly to video with `compose` or `render -o out.mov` rather than writing PNG frames — disk I/O is usually the bottleneck when saving thousands of PNGs.

---

## License

MIT