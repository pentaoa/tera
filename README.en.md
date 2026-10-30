<h1 align="center">tera</h1>

<p align="center">
  <a href="README.md">简体中文</a> &nbsp; / &nbsp; <strong>English</strong>
</p>

<p align="center">
  <strong>Sleeve's Album Flow and album grid, in your terminal.</strong>
</p>

<p align="center">
  <code>tera</code> browses your Music.app library as a horizontal cover flow or a tiled album grid, overlays
  high-resolution artwork through the Kitty graphics protocol, and leaves playback to Music.app.
  It paints no background, so your terminal's transparency and blur show through.
</p>

<p align="center">
  <img src="https://img.shields.io/badge/macOS-14%2B-20252B?style=flat-square" alt="macOS 14+">
  &nbsp;
  <img src="https://img.shields.io/badge/Rust-stable-20252B?style=flat-square" alt="Rust stable">
  &nbsp;
  <img src="https://img.shields.io/badge/TUI-Ratatui-FFA4A4?style=flat-square&amp;labelColor=20252B" alt="Ratatui TUI">
  &nbsp;
  <img src="https://img.shields.io/badge/Graphics-Kitty-BADFDB?style=flat-square&amp;labelColor=20252B" alt="Kitty graphics">
  &nbsp;
  <img src="https://img.shields.io/badge/Music.app-automation-20252B?style=flat-square" alt="Music.app automation">
</p>

<br>

<p align="center">
  <img src="assets/hero.png" alt="tera: album flow with high-resolution artwork" width="100%">
  <img src="assets/grid.png" alt="tera: album wall" width="100%">
</p>

<p align="center">
  <a href="https://ghostty.org"><img src="https://raw.githubusercontent.com/ghostty-org/ghostty/main/images/icons/icon_128.png" alt="Ghostty" width="28" height="28"></a><br>
  <sub>Best with <a href="https://ghostty.org"><strong>Ghostty</strong></a></sub>
</p>

---

> [!IMPORTANT]
> This project is only a UI and remote control for Music.app. Playback, library data and DRM remain
> with Apple Music; nothing here bypasses DRM or modifies Apple services.

## Quick start

Requires macOS 14+, a stable Rust toolchain, and a Music.app library. Ghostty or Kitty is recommended.
The first run asks for Automation access to Music.

```bash
git clone https://github.com/pentaoa/tera.git
cd tera
cargo run --release
cargo run --release -- --synthetic   # no Music access needed
```

Transparency is controlled by the terminal; tera never paints the background:

```ini
# ~/.config/ghostty/config
background-opacity = 0.85
background-blur = true
```

## Features

| | |
| --- | --- |
| **Album Flow** | Horizontal cover flow, nine perspective frames, baked mirror reflections, bottom-aligned |
| **Album grid** | A fixed album wall (Tab) with timed transitions and click-to-open albums |
| **High-res art** | Kitty graphics on Ghostty/Kitty; half-block pixel fallback elsewhere |
| **Responsive** | Cover size, spacing and text follow the terminal size |
| **Transparent** | Only artwork and text are drawn; no background fill |
| **Playback** | Enter opens the selected album and its track list, Space toggles play/pause in Music.app |

## Rendering

1. Half-block pixel canvas (`▀` carries two vertical pixels per cell).
2. Dirty-cell diff against the previous frame; stale text and covers are erased each frame.
3. `unicode-width`-aware cursor tracking for CJK titles.
4. Synchronized updates (`CSI ? 2026 h/l`).
5. Configurable 30, 60 or 120 fps frame loop (60 by default).
6. Kitty images uploaded once, placements updated by id with sub-cell `X`/`Y` offsets.
7. Nine pre-rendered Y-rotation frames per cover, with a vertically mirrored alpha reflection baked in.

Bench (200×60, animated): **1900+ fps** encoder, **~68 KB/frame**, **~8.0 MB/s at 120 fps**.
The actual frame rate depends on the terminal and display; this is not vertical sync.

## Keys

| Key | Action |
| --- | --- |
| ← / h | Previous album |
| → / l | Next album |
| Enter / i | Open selected album; play the selected track in details |
| Esc | Return from album details |
| s / , | Open settings for artwork, effects and menu layout |
| Space | Play / pause |
| Tab | Toggle flow / grid |
| q / Esc | Quit |

## Limitations

- Perspective uses nine pre-rendered angle frames, so transitions between angles are stepped.
- No search, lyrics or progress bar yet.
- High-resolution artwork requires Kitty graphics (Ghostty / Kitty).
- macOS only: library, artwork and playback go through Music.app automation.
