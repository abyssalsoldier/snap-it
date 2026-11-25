# Changelog

All notable changes to Snap It are recorded here.
The format follows [Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and the project uses
[Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

### Added
- Versioning: `CHANGELOG.md`, `scripts/bump-version.ps1`, a `snapit --version` flag, and the
  version in the tray tooltip.

## [0.1.0] - 2026-11-24

### Added
- **Capture:**
  - Region selection with window and control snapping and a magnifier
  - Window picker, active window, monitor under the cursor, all screens
  - Last region and a saved custom region
  - Scrolling capture (opt-in)
- **Quick annotate** toolbar on the capture overlay, with the same annotation tools as the editor.
- **Editor:**
  - Annotations stay editable
  - Image insertion, stickers and smart eraser
  - Crop and cut-out
  - Effects: beautify, border, shadow, rounded corners, resize, rotate, flip
- **Recording:** MP4 (Windows.Graphics.Capture + Media Foundation) and GIF, with no audio.
- **Tools:** OCR (Windows OCR), QR scan and generate, screen colour picker, ruler.
- **App:**
  - Tray icon, global hotkeys and workflows
  - After-capture task chain, notifications and capture sound
  - History browser, pin to screen, settings window
  - Portable mode, command line
- **Opt-in integrations:** start with Windows, Explorer context menu, notify-only update check.
- Release script (build, sign, SHA-256, Defender scan) and the EDR audit report.

[Unreleased]: https://github.com/abyssalsoldier/snap-it/compare/v0.1.0...dev
[0.1.0]: https://github.com/abyssalsoldier/snap-it/releases/tag/v0.1.0
