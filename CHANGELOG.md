# Changelog

All notable changes to Snap It are recorded here.
The format follows [Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and the project uses
[Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

## [0.6.0] - 2026-11-25
### Added
- **Keyboard and screen-reader support in Settings, setup, History and the QR maker.** Tab and
  Shift+Tab move between controls with a visible focus ring; Space toggles a switch or presses a
  button; arrow keys move between pages and choices; Enter saves Settings or goes to the next
  setup step; Ctrl+Tab switches Settings pages. In History, arrow keys, Home/End and Page Up/Down
  move through captures, Enter opens one, Delete removes it and Shift+F10 opens its menu.
  Narrator and other screen readers (UI Automation) can now read every control's name, state and
  value, and operate them; setup announces each step.

### Fixed
- **Scrolling capture works in more apps, and upwards (chats).** When scrolling down moves
  nothing, it now scrolls up from the same spot (chats open at the newest message) and stitches
  the older messages above, in reading order. For apps that ignore simulated mouse-wheel input,
  such as WhatsApp, it then asks the app to scroll through UI Automation (the accessibility
  interface screen readers use). It still only runs when scrolling capture is turned on.
- **Scrolling capture lines up chats with a wallpaper.** Messages scrolling over a background
  that stays still (WhatsApp's doodle wallpaper) could not be lined up, so the capture stopped
  after the first scroll. Snap It now falls back to a brightness comparison that tolerates the
  faint background and ignores parts that don't scroll (the chat list beside a conversation),
  and only accepts a match that clearly beats every other. A small animation in view (a GIF, a
  hover button) at the end of a chat no longer stops the capture; it scrolls the other way. A
  notification that pops up over the window while it scrolls no longer stops the capture or
  gets stitched in again and again (only the last screen can still show it).
- **Moving the mouse no longer stops a scrolling capture; Esc or a right-click does.** The
  slightest nudge used to end it. Now the pointer is put back on its spot before each scroll, and
  Esc or a right-click stops the capture (both watched only while it runs). If another app
  already uses Esc, a large deliberate mouse move stops it too. Failsafes: it also stops if
  another window comes to the front (so it never scrolls something else), and after 3 minutes.
- **No more "Scrolling capture…" notification in the capture.** The toast slid in over the
  bottom-right of the screen and was stitched into captures there; it is no longer shown.
- **Text boxes behave like Windows ones.** The caret is drawn where it really is (also in long
  text and multi-line boxes); select with Shift+arrows, mouse drag, double-click or Ctrl+A;
  Ctrl+C/X/V work on the selection (Ctrl+X no longer clears a box with nothing selected); undo and
  redo (Ctrl+Z/Y); Ctrl+arrows and Ctrl+Backspace/Delete work by word; Up/Down move through
  wrapped lines; emoji and IME input (Chinese, Japanese, Korean…) work.

## [0.5.1] - 2026-11-25
### Fixed
- **Scrolling capture no longer repeats floating overlays.** Something that floats at the
  bottom of the window while the page scrolls, such as a browser's link preview (the URL shown
  in the corner when the pointer rests on a link) or a chat button, used to be stitched in
  again on every screen. Snap It now takes those rows from the next screen, after they have
  scrolled clear; only the final screen can still show it.
- **Scrolling capture works with hand-drawn selections.** It used to scroll at the exact centre
  of the selection, so a rectangle that also covered a browser's tabs, a sidebar or another
  window could scroll nothing and stop after one screen. It now scrolls over the window that
  covers most of the selection, and if nothing moves it tries a few other spots first.

## [0.5.0] - 2026-11-25
### Added
- **Use Print Screen for Snap It (setup wizard).** When Windows gives the Print Screen key to its
  own screen capture, the Hotkeys step offers to turn that Windows setting off for you (opt-in,
  your account only). If Windows doesn't let go of the key straight away, setup tells you to sign
  out and back in. Setup re-appears once after updating only if this applies to you.

### Changed
- **New look for every window.** Settings, the setup wizard, History, the QR code maker and the
  editor now share one custom-drawn design: an integrated title bar with Snap It's icon, a
  sidebar in Settings, cards, toggle switches, segmented choices, keycap-style hotkey boxes and
  a cyan-to-indigo accent. Windows 11 rounded corners, Snap Layouts on the maximize button and
  Aero Snap keep working. The editor's title bar shows the file name; size and zoom stay in the
  status bar.
- **Image editor opens on a blank canvas.** Tools > "Image editor…" (and its hotkey) now opens
  the editor straight away instead of a file picker. Paste an image (Ctrl+V) or open one
  (Ctrl+O, or the Image button) and the canvas takes that image's size; once the canvas has
  content, pasted images are added on top as before, and Ctrl+O opens a new editor window.

## [0.4.0] - 2026-11-25
### Added
- **First-launch setup.** The first time Snap It starts (usually from the installer's
  "Launch Snap It"), a short wizard walks through the key features, lets you choose hotkeys
  (and checks whether another app already uses them, with a pointer to the Windows 11 Print
  Screen setting), turn on opt-in features such as scrolling capture, update checks, start at
  sign-in and the Explorer menu, and pick what happens after a capture. Your current settings
  are always filled in and kept unless you change them. It comes back once after an update
  that adds new setup options (new items are marked "(new)"), never after bug-fix releases.
  Run it again any time from Settings > General, or with `snapit --welcome`.
- **Text outline colour and width.** The text tool's outline now has its own colour (or "No
  outline") and size in the options strip, in the editor and in quick annotate. Before, it was
  a fixed contrast colour.

## [0.3.6] - 2026-11-25
### Changed
- **A finished scrolling capture opens in the editor**, fitted to the window, as well as
  running the usual after-capture tasks. Before, the only sign it had finished was a "Saved"
  tray balloon, which Windows may hide. A scrolling workflow with its own `tasks` in
  settings.toml still uses those instead.

## [0.3.5] - 2026-11-25
### Fixed
- **Scrolling capture stopped after one scroll** on most real pages: each new screen had to
  match the previous one almost exactly, so a moving scrollbar thumb, a blinking caret, a
  hover highlight or an unfinished smooth scroll ended it. Rows are now compared in strips,
  strips that do not scroll with the page (scrollbar, fixed sidebar) are ignored, a screen
  that does not line up is checked once more before stopping, and small areas are scrolled
  less so consecutive screens still overlap.
- When scrolling capture fails, stops early, finds that nothing scrolled, or is stopped by
  the mouse before the first scroll, a dialog now says why instead of a tray balloon that
  Windows may hide.

## [0.3.4] - 2026-11-25
### Fixed
- The scrolling capture hotkey seemed to do nothing while the feature was off (it is off by
  default): its only feedback was a tray balloon, which Windows often hides. It now asks
  whether to turn scrolling capture on and, if you agree, goes straight to choosing the area.

## [0.3.3] - 2026-11-25
### Added
- **Freehand arrows:** the arrow tool's new Path option (Straight / Freehand) draws a smoothed
  freehand line with heads at the end, the start or both ends. Each head points along the
  last stretch of the path, so a wobble on release does not swing it around. Switching Path
  on a selected arrow converts it, keeping its ends.

### Fixed
- Quick edit's tool-options strip could be missing when the selection was near the bottom
  of the screen: it was placed off-screen or behind the taskbar. The toolbar now leaves
  room for the strip and both are kept inside the monitor's work area.

## [0.3.2] - 2026-11-25
### Added
- **Starting Snap It opens Settings** (from the Start menu, a shortcut or the installer), and
  starting it again while it runs brings Settings to the front. Started at sign-in it stays in
  the tray: the "Start with Windows" shortcut now passes `--background` (an existing shortcut
  is updated once).
- **Record hotkeys by pressing them:** click a box on the Hotkeys tab and press the keys
  (PrintScreen and Alt/Win combinations included). Backspace clears a box, Esc leaves it
  unchanged. Snap It's own hotkeys pause while a box is active.
- **Quick edit has the editor's tool options** in a strip under its toolbar (replacing the
  Style popup).

### Changed
- **Snap It follows the Windows light/dark setting everywhere,** and switches live when it
  changes: Settings and the QR dialog (standard Windows controls, now with dark styles and
  readable dark-mode checkboxes), every window's title bar, and the region overlay (which
  was always dark). Settings > General > Theme can still force light or dark.

### Fixed
- Upgrading with the MSI while Snap It was running sat at "Validating install" for about
  100 seconds. Snap It ignored Windows' "session ending" request (also sent by Restart
  Manager during installs, sign-out and shutdown), so Windows waited for its hung-app
  timeout. It now exits at once (measured: 0.1 s instead of about 100 s).
- The MSI wizard now also asks a running Snap It to quit from its own process when Install is
  clicked. The existing quit step runs in the Windows Installer service, which can't reach
  the running copy.

## [0.3.1] - 2026-11-24
### Added
- **MSI setup wizard** with the same choices as the setup .exe: license, install folder,
  desktop shortcut, and "Launch Snap It" at the end (it used to install silently).
  Upgrades keep the previous folder and desktop-shortcut choice; `msiexec /qn` still
  installs silently.
- The setup .exe wizard shows the license too.

## [0.3.0] - 2026-11-24
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

[Unreleased]: https://github.com/abyssalsoldier/snap-it/compare/v0.6.0...dev
[0.6.0]: https://github.com/abyssalsoldier/snap-it/compare/v0.5.1...v0.6.0
[0.5.1]: https://github.com/abyssalsoldier/snap-it/compare/v0.5.0...v0.5.1
[0.5.0]: https://github.com/abyssalsoldier/snap-it/compare/v0.4.0...v0.5.0
[0.4.0]: https://github.com/abyssalsoldier/snap-it/compare/v0.3.6...v0.4.0
[0.3.6]: https://github.com/abyssalsoldier/snap-it/compare/v0.3.5...v0.3.6
[0.3.5]: https://github.com/abyssalsoldier/snap-it/compare/v0.3.4...v0.3.5
[0.3.4]: https://github.com/abyssalsoldier/snap-it/compare/v0.3.3...v0.3.4
[0.3.3]: https://github.com/abyssalsoldier/snap-it/compare/v0.3.2...v0.3.3
[0.3.2]: https://github.com/abyssalsoldier/snap-it/compare/v0.3.1...v0.3.2
[0.3.1]: https://github.com/abyssalsoldier/snap-it/compare/v0.3.0...v0.3.1
[0.3.0]: https://github.com/abyssalsoldier/snap-it/compare/v0.2.0...v0.3.0
[0.2.0]: https://github.com/abyssalsoldier/snap-it/compare/v0.1.0...v0.2.0
[0.1.0]: https://github.com/abyssalsoldier/snap-it/releases/tag/v0.1.0
