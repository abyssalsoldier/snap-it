//! User settings, stored as TOML. Unknown or missing keys fall back to defaults
//! so older files keep working after upgrades.

use std::path::PathBuf;

use serde::{Deserialize, Serialize};
use snapit_gfx::{effects::Beautify, io::Format, Rect, Rgba};

use crate::actions::Action;
use crate::hotkeys::Chord;

/// What happens automatically after a capture (the configurable task chain).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Tasks {
    pub copy: bool,
    pub save: bool,
    pub open_editor: bool,
    pub pin: bool,
    pub notify: bool,
    pub sound: bool,
}

impl Default for Tasks {
    fn default() -> Self {
        Self { copy: true, save: true, open_editor: false, pin: false, notify: true, sound: true }
    }
}

/// A workflow: a hotkey bound to an action, optionally with its own task chain.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Workflow {
    pub hotkey: String,
    pub action: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tasks: Option<Tasks>,
}

impl Workflow {
    pub fn new(hotkey: &str, action: Action) -> Self {
        Self { hotkey: hotkey.into(), action: action.name().into(), tasks: None }
    }
    pub fn resolve(&self) -> Option<(Chord, Action)> {
        Some((Chord::parse(&self.hotkey)?, Action::from_name(&self.action)?))
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Region {
    /// Show the quick-annotate toolbar after selecting a region.
    pub quick_annotate: bool,
    pub detect_windows: bool,
    pub detect_controls: bool,
    /// Ask UI Automation for control bounds in apps without child windows
    /// (Chromium/Electron, WPF, WinUI). Off: only image edges are used.
    pub uia_controls: bool,
    pub magnifier: bool,
    pub crosshair: bool,
    pub dim_opacity: u8,
}

impl Default for Region {
    fn default() -> Self {
        Self { quick_annotate: true, detect_windows: true, detect_controls: true, uia_controls: true, magnifier: true, crosshair: false, dim_opacity: 110 }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Annotate {
    pub color: Rgba,
    /// The highlighter keeps its own colour (a marker is yellow even when the pen is red).
    pub highlight: Rgba,
    pub width: f32,
    pub font_size: f32,
    pub palette: Vec<Rgba>,
    /// The editor's per-tool options (border, fill, shadow…), remembered between sessions.
    pub tools: ToolOptions,
}

/// Editor tool options, modelled on ShareX's annotation options. Shared options
/// (border style, shadow…) apply to every tool that has them, as in ShareX.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct ToolOptions {
    pub fill: bool,
    pub fill_color: Rgba,
    pub corner_radius: f32,
    pub dash: snapit_annotate::Dash,
    pub shadow: bool,
    pub heads: snapit_annotate::ArrowHeads,
    pub step_type: snapit_annotate::StepType,
    pub text_background: bool,
    pub blur_radius: f32,
    pub pixel_size: f32,
    /// Magnify zoom, as a factor (2.0 = 200%).
    pub zoom: f32,
    /// Spotlight dim, percent.
    pub spot_dim: f32,
    pub spot_ellipse: bool,
}

impl Default for ToolOptions {
    fn default() -> Self {
        Self {
            fill: false,
            fill_color: Rgba::WHITE,
            corner_radius: 0.0,
            dash: snapit_annotate::Dash::Solid,
            shadow: false,
            heads: snapit_annotate::ArrowHeads::End,
            step_type: snapit_annotate::StepType::Numbers,
            text_background: false,
            blur_radius: 12.0,
            pixel_size: 12.0,
            zoom: 2.0,
            spot_dim: snapit_annotate::SPOTLIGHT_DIM as f32,
            spot_ellipse: false,
        }
    }
}

impl Default for Annotate {
    fn default() -> Self {
        Self {
            color: Rgba::RED,
            highlight: Rgba::YELLOW,
            width: 4.0,
            font_size: 22.0,
            palette: vec![
                Rgba::RED,
                Rgba::rgb(249, 115, 22),
                Rgba::YELLOW,
                Rgba::rgb(34, 197, 94),
                Rgba::rgb(59, 130, 246),
                Rgba::rgb(168, 85, 247),
                Rgba::rgb(236, 72, 153),
                Rgba::WHITE,
                Rgba::rgb(107, 114, 128),
                Rgba::BLACK,
            ],
            tools: ToolOptions::default(),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Recording {
    pub mp4_fps: u32,
    pub gif_fps: u32,
    pub mp4_bitrate_kbps: u32,
    pub cursor: bool,
    /// GIFs are capped so a forgotten recording can't fill the disk.
    pub gif_max_seconds: u32,
}

impl Default for Recording {
    fn default() -> Self {
        Self { mp4_fps: 30, gif_fps: 15, mp4_bitrate_kbps: 8000, cursor: true, gif_max_seconds: 60 }
    }
}

/// Where Snap It's own releases are published.
pub const DEFAULT_UPDATE_REPO: &str = "abyssalsoldier/snap-it";

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Integration {
    pub start_with_windows: bool,
    pub context_menu: bool,
    pub update_check: bool,
    /// GitHub "owner/repo" whose releases are checked. Empty means [`DEFAULT_UPDATE_REPO`].
    pub update_repo: String,
    /// A release the user chose "Don't remind me about this version" for.
    pub skipped_version: String,
}

impl Default for Integration {
    fn default() -> Self {
        Self { start_with_windows: false, context_menu: false, update_check: false, update_repo: DEFAULT_UPDATE_REPO.into(), skipped_version: String::new() }
    }
}


#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Theme {
    #[default]
    System,
    Light,
    Dark,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    /// `None` uses Pictures\Snap It (or Screenshots\ next to the exe in portable mode).
    pub save_dir: Option<PathBuf>,
    /// Tokens: %y %mo %d %h %mi %s %ms %rnd %width %height %title
    pub filename_pattern: String,
    pub format: Format,
    pub jpeg_quality: u8,
    pub capture_cursor: bool,
    pub capture_delay_ms: u32,
    pub tasks: Tasks,
    pub region: Region,
    pub annotate: Annotate,
    pub beautify: Beautify,
    pub recording: Recording,
    pub integration: Integration,
    pub theme: Theme,
    /// Opt-in: scrolling capture sends synthetic wheel input to the target window.
    pub scrolling_capture: bool,
    pub custom_region: Option<Rect>,
    pub last_region: Option<Rect>,
    pub history_limit: usize,
    pub workflows: Vec<Workflow>,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            save_dir: None,
            filename_pattern: "SnapIt_%y-%mo-%d_%h-%mi-%s".into(),
            format: Format::Png,
            jpeg_quality: 90,
            capture_cursor: false,
            capture_delay_ms: 0,
            tasks: Tasks::default(),
            region: Region::default(),
            annotate: Annotate::default(),
            beautify: Beautify::default(),
            recording: Recording::default(),
            integration: Integration::default(),
            theme: Theme::System,
            scrolling_capture: false,
            custom_region: None,
            last_region: None,
            history_limit: 500,
            workflows: vec![
                Workflow::new("PrintScreen", Action::CaptureRegion),
                Workflow::new("Ctrl+PrintScreen", Action::CaptureFullscreen),
                Workflow::new("Shift+PrintScreen", Action::CaptureActiveWindow),
                Workflow::new("Ctrl+Shift+PrintScreen", Action::RecordMp4),
            ],
        }
    }
}

impl Settings {
    pub fn load() -> Settings {
        let path = crate::paths::settings_file();
        match std::fs::read_to_string(&path) {
            Ok(s) => toml::from_str(&s).unwrap_or_else(|e| {
                crate::log(&format!("settings file has errors, using defaults: {e}"));
                Settings::default()
            }),
            Err(_) => Settings::default(),
        }
    }

    pub fn save(&self) -> Result<(), String> {
        let path = crate::paths::settings_file();
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
        }
        let s = toml::to_string_pretty(self).map_err(|e| e.to_string())?;
        // Write-then-rename so a crash never leaves a half-written file.
        let tmp = path.with_extension("toml.tmp");
        std::fs::write(&tmp, s).map_err(|e| e.to_string())?;
        std::fs::rename(&tmp, &path).map_err(|e| e.to_string())
    }

    pub fn save_dir(&self) -> PathBuf {
        self.save_dir.clone().unwrap_or_else(crate::paths::default_save_dir)
    }

    /// Task chain for an action: the workflow's override if it has one, else the global chain.
    pub fn tasks_for(&self, action: Action) -> Tasks {
        self.workflows
            .iter()
            .find(|w| w.action == action.name() && w.tasks.is_some())
            .and_then(|w| w.tasks)
            .unwrap_or(self.tasks)
    }

    pub fn hotkey_for(&self, action: Action) -> Option<String> {
        self.workflows.iter().find(|w| w.action == action.name()).map(|w| w.hotkey.clone())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn toml_round_trip_and_partial_files() {
        let s = Settings::default();
        let text = toml::to_string_pretty(&s).unwrap();
        let back: Settings = toml::from_str(&text).unwrap();
        assert_eq!(back, s);

        let partial: Settings = toml::from_str("jpeg_quality = 70\n[tasks]\nsave = false\n").unwrap();
        assert_eq!(partial.jpeg_quality, 70);
        assert!(!partial.tasks.save && partial.tasks.copy);
        assert_eq!(partial.workflows.len(), 4);
    }

    #[test]
    fn workflow_overrides() {
        let mut s = Settings::default();
        s.workflows.push(Workflow {
            hotkey: "Ctrl+Alt+P".into(),
            action: "capture_monitor".into(),
            tasks: Some(Tasks { pin: true, save: false, ..Tasks::default() }),
        });
        assert!(s.tasks_for(Action::CaptureMonitor).pin);
        assert!(!s.tasks_for(Action::CaptureRegion).pin);
        assert!(s.workflows.iter().all(|w| w.resolve().is_some()));
    }
}
