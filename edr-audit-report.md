# EDR / AV Audit — Snap It

**Scope:**
- Source: every crate under `crates/`, plus `scripts/release.ps1` and the `build.rs` resource generation.
- Binary: `target/release/snapit.exe` (2.8 MB, x64, release profile).

**Method:**
- Searched the source for the heuristic categories in the EDR-audit reference and read every hit in context.
- Inspected the PE import table with `dumpbin /imports`.
- Scanned the binary with Microsoft Defender (`MpCmdRun -Scan -ScanType 3`).
- Checked the Authenticode status.

## Executive summary

| Likelihood to trigger | Count | Notes |
|---|---|---|
| High | 0 | No injection, no hooks, no droppers, no obfuscation, no tampering |
| Medium | 3 | Binary is unsigned until signed (F1). Two opt-in features carry sensitive APIs (F2, F3). |
| Low | 5 | Expected screenshot-tool behaviour and opt-in integrations |
| Fixed during audit | 2 | Signing password on a command line (F9). Unchecked shell "open" of a settings path (F10). |

**Overall posture: good.** The default configuration performs only the following:
- One-shot `BitBlt` screen captures triggered by the user
- `RegisterHotKey`
- Clipboard writes on explicit actions
- File writes to `Pictures\Snap It`
- Windows.Graphics.Capture and Media Foundation calls during recordings the user starts

There are no child processes and no network access, and nothing persists. The binary scans clean with Defender.

The remaining sensitive behaviour sits behind three opt-in settings, all off by default:
- Scrolling capture (synthetic input)
- Explorer context menu (HKCU registry)
- Update check (one HTTPS GET)

The single biggest improvement left is **Authenticode signing** (F1).

### Binary import review

`dumpbin /imports` shows **none** of the following:
- `SetWindowsHookEx`, `GetAsyncKeyState`, `GetKeyboardState`
- `CreateProcess*`, `WinExec`, `ShellExecuteEx`
- `OpenProcess`, `VirtualAllocEx`, `WriteProcessMemory`, `CreateRemoteThread`
- `AdjustTokenPrivileges`, `CryptUnprotectData`
- `SetWindowDisplayAffinity`, `AddClipboardFormatListener`, `SetClipboardViewer`, `URLDownloadToFile`

Sensitive imports that are present, all accounted for:

| Import | Used by | Default |
|---|---|---|
| `SendInput` | `crates/app/src/scroll.rs:60`, scrolling capture | off |
| `RegSetValueExW` / `RegCreateKeyExW` | `crates/app/src/integration.rs:71-77`, Explorer verbs | off |
| `WinHttpOpen` + others | `crates/app/src/integration.rs:131-170`, update check | off |
| `LoadLibraryExA` | windows-rs WinRT activation fallback (library code) | n/a; constrained by `SetDefaultDllDirectories(System32)` at startup (`crates/app/src/main.rs`) |

Linked DLLs are all OS components: user32, gdi32, d3d11, mfplat, mfreadwrite, winhttp, shell32, ole32, comctl32, comdlg32, dwmapi, winmm, and the CRT.

## Findings

| # | Finding | Location | Likelihood | Status |
|---|---|---|---|---|
| F1 | Release binary is unsigned | `target/release/snapit.exe` | Medium | Needs a certificate (script ready) |
| F2 | Synthetic mouse-wheel input plus cursor moves | [scroll.rs:60](crates/app/src/scroll.rs#L60), [scroll.rs:77](crates/app/src/scroll.rs#L77) | Medium (when enabled) | Opt-in and off by default; documented |
| F3 | Full-desktop capture plus window enumeration and window titles | [capture/lib.rs:116](crates/capture/src/lib.rs#L116), [capture/lib.rs:198](crates/capture/src/lib.rs#L198), [capture/lib.rs:249](crates/capture/src/lib.rs#L249) | Medium (screen-capture analytics) | Inherent to the product; mitigated by design |
| F4 | Startup-folder shortcut | [integration.rs:49](crates/app/src/integration.rs#L49) | Low | Opt-in |
| F5 | HKCU shell verbs under `SystemFileAssociations\image\shell` | [integration.rs:82-104](crates/app/src/integration.rs#L82-L104) | Low | Opt-in, removable |
| F6 | Outbound HTTPS to api.github.com | [integration.rs:131](crates/app/src/integration.rs#L131) | Low | Opt-in, notify only |
| F7 | `ShellExecuteW` for folders and URLs | [shell.rs](crates/app/src/shell.rs) | Low | Hardened (F10) |
| F8 | `WM_COPYDATA` argument forwarding between instances | [main.rs](crates/app/src/main.rs) (`WM_COPYDATA` handler) | Low | Accepted |
| F9 | PFX password passed to `signtool /p` | [scripts/release.ps1](scripts/release.ps1) | Medium (credential in command-line telemetry) | **Fixed** |
| F10 | `ShellExecute "open"` on a path read from settings | [shell.rs](crates/app/src/shell.rs) `open_folder` | Low | **Fixed** |

### F1: Unsigned release binary (Medium)

**Why EDR flags this:**
- Unsigned, low-prevalence executables get stricter reputation scoring: SmartScreen, Defender cloud protection, and allowlisting rules such as WDAC or AppLocker publisher rules.
- That stricter scoring combines with the screen-capture behaviour described in F3.

**Already done:**
- Icon, `VERSIONINFO` (CompanyName, FileDescription, ProductName, versions, OriginalFilename) and the manifest (`asInvoker`, PerMonitorV2, Common Controls v6) are embedded by `crates/app/build.rs`.
- No packer is used. Symbols are stripped, which is normal.

**Mitigation:**
1. Sign every release with an OV or EV code-signing certificate and an RFC 3161 timestamp: `./scripts/release.ps1 -CertThumbprint <sha1>`.
2. Distribute from a stable URL. Install per machine into `C:\Program Files\Snap It\`, or ship as MSIX, rather than running from `Downloads` or `%APPDATA%`.

### F2: `SendInput` scrolling capture (Medium when enabled)

**Why EDR flags this:** synthetic input combined with screen capture resembles remote-control and automation tooling.

**Already mitigated:**
- Off by default; it must be enabled in Settings > Capture, and the UI says why.
- It only sends `MOUSEEVENTF_WHEEL`, never keystrokes.
- It runs only after the user selects an area, and stops when the user presses Esc (registered with
  `RegisterHotKey` only while the capture runs; no keyboard polling or hooks) or right-clicks
  (Raw Input for the mouse only, registered only while it runs; nothing is blocked or changed).
- Failsafes: it stops if another window comes to the front, and after 3 minutes regardless.
- If the wheel moves nothing, it can instead ask the app to scroll through UI Automation's Scroll
  pattern (the accessibility interface), under the same opt-in.

**Optional further step:** offer a "send scroll messages" mode that posts `WM_MOUSEWHEEL` to the target window instead of using `SendInput`. Some applications ignore posted wheel messages, so keep `SendInput` as the fallback.

### F3: Screen capture and window enumeration (Medium for spyware analytics)

**Why EDR flags this:** desktop `BitBlt` combined with `EnumWindows` and `GetWindowText` is also what screen-scraping spyware does.

**Already mitigated:**
- Every capture is user-initiated (hotkey, tray or command line), with a visible tray icon, sound and notification.
- There is no timer-driven capture, no upload path, and no clipboard monitoring.
- Window titles are used only for the `%title` file-name token and window snapping, and never leave the process.
- Video uses Windows.Graphics.Capture, which shows the OS capture border.

**SOC note:** see the allowlisting brief below.

### F4–F6: Opt-in integrations (Low)

- **Startup shortcut (F4):** created through `IShellLinkW` in the user's Startup folder, pointing at the current exe. It avoids the `Run` key, scheduled tasks and services. If users run a portable copy from a user-writable folder, the shortcut points there; recommend installed copies for managed fleets.
- **Explorer verbs (F5):** HKCU only, no COM servers, and the command is the exe plus `--edit "%1"` or `--pin "%1"`. They are removed by unticking the option.
- **Update check (F6):**
  - WinHTTP with the system proxy and certificate validation left on (`WINHTTP_FLAG_SECURE`).
  - The repository name is validated (`owner/repo` charset).
  - The response is only parsed for `tag_name` and `html_url`.
  - Nothing is downloaded or executed; it only notifies.
  - The user agent is a generic product string with no user data.

### F7, F10: Shell execution (Low, hardened)

- `open_url` accepts `https://` URLs only. It is triggered by a click on a notification, from an update result or a scanned QR code.
- **Fixed (F10):** `open_folder` now refuses anything that isn't an existing directory and uses the `explore` verb. A hand-edited `save_dir` pointing at an executable can no longer launch it from "Open folder".
- `show_in_folder` uses `SHOpenFolderAndSelectItems`, which never executes anything.

### F8: `WM_COPYDATA` forwarding (Low)

A second launch forwards its arguments to the running instance. Only `--capture <known action>`, `--edit <image>` and `--pin <image>` are honoured. Images are decoded by the `image` crate and never executed. No change needed.

### F9: Signing password on a command line (Medium, fixed)

**Was:**
- `release.ps1` converted the PFX `SecureString` to plain text and ran `signtool sign /f x.pfx /p <password>`.
- EDRs (and Sysmon event 1) record full process command lines, so the signing password would have been written to security telemetry.

**Now:**
- The PFX is imported into `Cert:\CurrentUser\My` with `Import-PfxCertificate` (the `SecureString` is never decoded) and signed by thumbprint (`/sha1`).
- It is removed from the store in a `finally` block.
- The timestamp URL uses plain HTTP, which is normal for RFC 3161 because the timestamp response is itself signed.

## Build, signing and operational hardening checklist

- [x] No packer (UPX) or obfuscation; plain release profile (`strip`, thin LTO)
- [x] PE icon and full `VERSIONINFO` metadata embedded (`crates/app/build.rs`)
- [x] Manifest: `asInvoker` (never elevates), PerMonitorV2, Common Controls v6
- [x] `SetDefaultDllDirectories(LOAD_LIBRARY_SEARCH_SYSTEM32)` at startup (blocks DLL planting)
- [x] No child processes, hooks, process access, or credential and secret access
- [x] No network by default; the opt-in check uses HTTPS with OS certificate validation
- [x] Persistence only as an opt-in Startup-folder shortcut; no `Run` key, tasks or services
- [x] Defender scan of the release binary: **no threats found**
- [x] `release.ps1`: tests, release build, SHA-256 file, Defender scan; secrets kept off command lines
- [ ] **Sign releases** (OV or EV cert with an RFC 3161 timestamp): `./scripts/release.ps1 -CertThumbprint <sha1>`
- [ ] Ship an installer (MSI or MSIX) to `C:\Program Files\Snap It\` for managed fleets
- [ ] Optional: submit the signed build to the Microsoft Defender file-submission portal ahead of a wide rollout

## SOC allowlisting brief

| Field | Value |
|---|---|
| Binary | `C:\Program Files\Snap It\snapit.exe` (recommended install path) |
| Signer | *your code-signing certificate subject and thumbprint, once signed* |
| Process tree | `explorer.exe → snapit.exe` (Startup shortcut or user launch). **No child processes.** Folders and URLs open through the shell (`explorer.exe`, default browser). |
| Network | None by default. Opt-in: `https://api.github.com/repos/<owner>/<repo>/releases/latest` (TCP 443, GET only). |
| Registry | None by default. Opt-in: `HKCU\Software\Classes\SystemFileAssociations\image\shell\SnapIt.Edit` and `SnapIt.Pin`. |
| Files written | `%APPDATA%\SnapIt\settings.toml` and `history.jsonl`; captures in `%USERPROFILE%\Pictures\Snap It\` (configurable). Opt-in: `…\Start Menu\Programs\Startup\Snap It.lnk`. |
| Sensitive behaviour | User-initiated screen capture (GDI `BitBlt`; Windows.Graphics.Capture while recording, with the OS border visible). Opt-in scrolling capture sends mouse-wheel `SendInput` to the selected area. |
| Justification | Screenshot and screen-recording utility. Every capture is started by the user and stays local; there is no upload capability in the codebase. |
