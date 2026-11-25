//! User-triggered actions. Every capture in Snap It starts here, from an explicit
//! hotkey press, tray click or command-line flag. Nothing runs on a timer
//! except a user-requested capture delay.

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Action {
    CaptureRegion,
    CaptureWindow,
    CaptureActiveWindow,
    CaptureMonitor,
    CaptureFullscreen,
    CaptureLastRegion,
    CaptureCustomRegion,
    CaptureScrolling,
    RecordMp4,
    RecordGif,
    Ocr,
    ScanQr,
    MakeQr,
    ColorPicker,
    Ruler,
    OpenEditor,
    OpenHistory,
    OpenFolder,
    OpenSettings,
    Exit,
}

/// (action, stable settings name, menu label)
const TABLE: &[(Action, &str, &str)] = &[
    (Action::CaptureRegion, "capture_region", "Region"),
    (Action::CaptureWindow, "capture_window", "Window…"),
    (Action::CaptureActiveWindow, "capture_active_window", "Active window"),
    (Action::CaptureMonitor, "capture_monitor", "Monitor under cursor"),
    (Action::CaptureFullscreen, "capture_fullscreen", "All screens"),
    (Action::CaptureLastRegion, "capture_last_region", "Last region"),
    (Action::CaptureCustomRegion, "capture_custom_region", "Custom region"),
    (Action::CaptureScrolling, "capture_scrolling", "Scrolling capture"),
    (Action::RecordMp4, "record_mp4", "Record MP4"),
    (Action::RecordGif, "record_gif", "Record GIF"),
    (Action::Ocr, "ocr", "Text (OCR)"),
    (Action::ScanQr, "scan_qr", "Scan QR code"),
    (Action::MakeQr, "make_qr", "Make QR code…"),
    (Action::ColorPicker, "color_picker", "Screen color picker"),
    (Action::Ruler, "ruler", "Ruler"),
    (Action::OpenEditor, "open_editor", "Image editor…"),
    (Action::OpenHistory, "open_history", "History"),
    (Action::OpenFolder, "open_folder", "Screenshots folder"),
    (Action::OpenSettings, "open_settings", "Settings…"),
    (Action::Exit, "exit", "Exit"),
];

impl Action {
    fn index(self) -> usize {
        TABLE.iter().position(|(a, _, _)| *a == self).expect("every action is in TABLE")
    }
    /// Menu / hotkey id (never 0).
    pub fn id(self) -> u16 {
        self.index() as u16 + 1
    }
    pub fn from_id(id: u16) -> Option<Self> {
        TABLE.get((id as usize).checked_sub(1)?).map(|t| t.0)
    }
    pub fn name(self) -> &'static str {
        TABLE[self.index()].1
    }
    pub fn from_name(name: &str) -> Option<Self> {
        TABLE.iter().find(|t| t.1 == name).map(|t| t.0)
    }
    pub fn label(self) -> &'static str {
        TABLE[self.index()].2
    }
    #[cfg(test)]
    pub fn all() -> impl Iterator<Item = Action> {
        TABLE.iter().map(|t| t.0)
    }
    /// Actions that open a full-screen overlay (only one may be open at a time).
    pub fn uses_overlay(self) -> bool {
        matches!(
            self,
            Action::CaptureRegion
                | Action::CaptureWindow
                | Action::CaptureScrolling
                | Action::RecordMp4
                | Action::RecordGif
                | Action::Ocr
                | Action::ScanQr
                | Action::ColorPicker
                | Action::Ruler
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ids_and_names_round_trip() {
        for a in Action::all() {
            assert_eq!(Action::from_id(a.id()), Some(a));
            assert_eq!(Action::from_name(a.name()), Some(a));
        }
        assert_eq!(Action::from_id(0), None);
        let mut names: Vec<_> = Action::all().map(|a| a.name()).collect();
        names.sort_unstable();
        names.dedup();
        assert_eq!(names.len(), TABLE.len());
    }
}
