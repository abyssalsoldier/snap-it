# Changelog

All notable changes to Snap It are recorded here.
The format follows [Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and the project uses
[Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

### Added
- **Update prompt:** when a newer release exists, a dialog shows what's new with Download,
  Release notes and Later, plus "Don't remind me about this version". Download opens the
  installer in the browser: the .msi for MSI installs, otherwise the setup wizard (portable
  copies included, so updating always means running the wizard). Snap It still never
  downloads or runs anything itself.
- MIT license (JRankin Tech). The repository is now public.

### Fixed
- Starting Snap It while it was already running did nothing visible. Now the running copy
  shows "Snap It is already running"; a newer version (for example a freshly unzipped
  portable copy) closes the old one and takes over; a different copy of the same or an older
  version asks before switching. This uses named kernel objects only, no window lookup.

### Changed
- The update check uses Snap It's own release repository by default (the setting can still
  point elsewhere).

## [0.2.0] - 2026-11-24
### Added
- Versioning: `CHANGELOG.md`, `scripts/bump-version.ps1`, a `snapit --version` flag, and the
  version in the tray tooltip.
- Region overlay: scroll the mouse wheel while hovering to widen or narrow the snap target
  (control → enclosing panel → window). Text labels, tiny parts and boxes split by a divider
  line are not picked by default; scroll down to reach them.
- Setting "Use UI Automation to find controls in modern apps" (on by default).
- **Editor tool options**, modelled on ShareX's: a strip in the top bar shows the options of
  the current tool (or of the selected item, which they then restyle): border colour and size,
  fill colour (or no fill), corner radius, line style (solid, dashed, dotted, dash-dot), drop
  shadow, arrow heads (end, start, both), step type (1, A, a, I, i), text background, blur
  radius, pixel size, magnify zoom, and spotlight dim and shape. Colours come from the palette
  or the Windows colour picker; the mouse wheel changes numbers. Options are remembered.
- **Spotlight** tool (S) in the editor and quick edit: dims everything outside a rectangle or
  ellipse. Several spotlights share one dimmed layer.
- **Installers:** a setup .exe (Inno Setup) and an .msi (WiX), both per-user with no admin
  prompt, with a Start menu shortcut and an uninstaller, plus a portable .zip.
  `scripts/package.ps1` builds all three; pushing `master` builds them in GitHub Actions and
  publishes the GitHub release.
- `snapit --quit` closes a running Snap It (the installers use it before upgrading or
  uninstalling).

### Changed
- Quick-edit toolbar: about 25% bigger buttons, in two rows (tools on top, actions below).
- The editor's Style button and popup are replaced by the tool-options strip.

### Fixed
- Region snapping only found whole windows in apps without child windows (browsers, Electron
  apps such as VS Code, WPF, WinUI, the Windows 11 shell). Control snapping now uses UI
  Automation element bounds, fetched once per window on a background thread with timeouts,
  and falls back to finding panels and boxes from edges in the screenshot.
- Region overlay lag: rendering is deferred to WM_PAINT and limited to the areas that changed,
  and the annotated image is patched locally. About 3 ms per frame on a 4K desktop.

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

[Unreleased]: https://github.com/abyssalsoldier/snap-it/compare/v0.2.0...dev
[0.2.0]: https://github.com/abyssalsoldier/snap-it/compare/v0.1.0...v0.2.0
[0.1.0]: https://github.com/abyssalsoldier/snap-it/releases/tag/v0.1.0
