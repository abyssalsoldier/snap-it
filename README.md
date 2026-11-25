# Snap It

A fast, local-only screenshot and screen-recording tool for Windows 10/11, written in Rust.
It covers the everyday ShareX workflow (capture, annotate, copy/save/pin, record) and leaves out
everything that tends to trip endpoint detection (EDR) or antivirus heuristics.

## Features

**Capture**
- Region selection with window and control snapping, a 17×17 pixel magnifier, size readout,
  arrow-key nudging and resize handles
- Window picker, active window, monitor under the cursor, all screens
- Last region and a saved custom region
- Optional capture delay and mouse pointer
- Scrolling capture (opt-in, off by default). It stitches as the page scrolls and handles
  sticky headers and footers.

**Quick annotate, without opening the editor.** After you select a region, a toolbar appears
right on the overlay:
- Tools: rectangle, ellipse, line, arrow, pen, highlighter, text, speech balloon, step numbers,
  blur, pixelate, smart eraser, magnifier and stickers
- Colour, width and text size, plus undo and redo
- Finish keys:

| Key | Action |
|---|---|
| Enter or double-click | Run your after-capture tasks |
| Ctrl+C | Copy |
| Ctrl+S | Save |
| P | Pin |
| E | Open in the full editor, with annotations still editable |
| Esc | Cancel |

**Editor**
- All annotation tools, plus select, move and resize of shapes, images and stickers
- Image insertion from a file, the clipboard or a new screen capture
- Crop and cut-out (remove a strip); annotations shift with the image
- Effects: beautify (gradient backdrop, rounded corners, shadow), border, drop shadow,
  rounded corners, resize, rotate and flip
- Zoom and pan, unlimited undo, save and save as (PNG, JPEG, WebP), copy and pin

**Recording**
- MP4 (H.264 through Media Foundation, GPU-accelerated when available) and GIF. The GIF encoder
  only re-encodes the parts of each frame that changed.
- A red frame outside the recorded area, and a REC pill with a timer, Stop and Cancel
- Press the hotkey again to stop. GIF length is capped (60 s by default).
- No audio, ever.

**Tools**
- Text recognition (OCR) with the on-device Windows OCR engine
- QR code scan and generate
- Screen colour picker (copies the hex value) and an on-screen ruler

**App**
- Tray icon and global hotkeys
- Workflows: a hotkey plus an action, each with an optional task chain
- After-capture task chain: copy, save, open editor, pin, notify, sound
- History browser with thumbnails
- Pin-to-screen windows (zoom, opacity, drag)
- Light, dark or system theme, portable mode, command-line actions and single instance

## Defaults

| Hotkey | Action |
|---|---|
| PrintScreen | Region (with quick annotate) |
| Ctrl+PrintScreen | All screens |
| Shift+PrintScreen | Active window |
| Ctrl+Shift+PrintScreen | Record MP4 (press again to stop) |

If another app (for example the Windows Snipping Tool) already owns PrintScreen, Snap It tells
you which hotkeys were taken. Change them in **Settings > Hotkeys**.

## EDR-safety design

| Avoided | Used instead |
|---|---|
| Low-level keyboard or mouse hooks (`SetWindowsHookEx`) | `RegisterHotKey`. Snap It only receives the chords it registered. |
| Background or timed screen capture | Every capture starts from a hotkey, the tray or the command line. The tray icon is always visible. |
| Child processes (FFmpeg, cmd, PowerShell) | Everything runs in-process: Media Foundation, a Rust GIF encoder, Windows OCR. Folders and URLs open through the shell. |
| Uploads or any network use of captures | There is no upload code at all. The only network call is an opt-in update check that only notifies. |
| Self-updating | The update check opens the release page and never downloads or runs anything. |
| `Run` key persistence | An opt-in shortcut in the Startup folder |
| Clipboard monitoring | The clipboard is written only when you copy and read only when you paste |
| Elevation | `asInvoker` manifest; no admin rights needed or requested |
| Packed or obfuscated binaries | A plain stripped build with an embedded icon, version info and manifest; `scripts/release.ps1` signs it |
| GDI capture loops for video | Windows.Graphics.Capture, which shows the system capture border |

Scrolling capture is the one feature that sends synthetic input (mouse-wheel `SendInput` to the
area you select). That is why it is off by default and must be enabled in **Settings > Capture**.

## Building

Requirements: Rust (stable, MSVC toolchain) and the Visual Studio C++ build tools / Windows SDK.

```powershell
cargo test --workspace          # unit tests (graphics, annotation, capture, OCR, QR, stitching…)
cargo run -p snap-it            # debug build with a console for logs
./scripts/release.ps1           # tests + release build + SHA-256 + Defender scan
./scripts/release.ps1 -CertThumbprint <sha1>   # …and Authenticode-sign it
```

Extra checks:

- `cargo test -p snapit-record -- --ignored` records about 1.5 s of a small screen area to MP4
  and GIF. Windows shows its capture border while it runs.
- `target/debug/snapit.exe --selftest <dir>` (debug builds only) drives every window hidden,
  with synthetic input and a fake desktop image. It writes PNG renders of each screen to `<dir>`.
  Nothing is shown, captured or copied to the clipboard.
- After changing the icon art, regenerate it with `cargo run -p snapit-gfx --example make_icon`.

## Branches and versioning

**Branches**

| Branch | Purpose |
|---|---|
| `dev` | Day-to-day work. The default branch; `git push` goes here. |
| `master` | Released code only. Every release is a tagged commit (`vX.Y.Z`). |

**Versions** follow [Semantic Versioning](https://semver.org/).
- There is one version for the whole workspace: `[workspace.package] version` in `Cargo.toml`.
- It is embedded in the exe's version info, the tray tooltip and `snapit --version`.
- Notable changes go under `## [Unreleased]` in [CHANGELOG.md](CHANGELOG.md) as you work.

**Release steps**

```powershell
./scripts/bump-version.ps1 patch      # or minor / major / 1.2.3 (run on dev; commits the bump)
git push                              # dev
git switch master; git merge --ff-only dev
git tag -a v0.1.1 -m "Snap It 0.1.1"
git push origin master v0.1.1
git switch dev
./scripts/release.ps1 -CertThumbprint <sha1>   # build, sign, scan -> dist/
```

## Command line

```
snapit --capture region|window|active_window|monitor|fullscreen|last_region|custom_region|
                 scrolling|record_mp4|record_gif|ocr|scan_qr|color_picker|ruler
snapit --edit  <image>     open an image in the editor
snapit --pin   <image>     pin an image to the screen
```

If Snap It is already running, these are forwarded to the running instance.

## Files

| What | Where |
|---|---|
| Settings | `%APPDATA%\SnapIt\settings.toml`. Hand-editable; `[[workflows]]` allows a per-hotkey task chain. |
| History | `%APPDATA%\SnapIt\history.jsonl` |
| Screenshots | `Pictures\Snap It` (configurable) |
| Portable mode | Put an empty file named `portable` next to `snapit.exe`. Everything then lives beside the exe. |

## Layout

```
crates/gfx       BGRA pixmaps, text-as-paths, effects (blur, pixelate, beautify…), encoders, icon art
crates/capture   monitors, windows/controls, one-shot screen capture
crates/annotate  annotation model, renderer, stickers, undo history (shared by overlay and editor)
crates/record    Windows.Graphics.Capture → Media Foundation MP4 / streaming GIF
crates/app       tray, hotkeys, overlay, editor, pin, history, settings, OCR/QR, recording UI
```
