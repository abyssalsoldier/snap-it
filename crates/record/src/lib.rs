//! Screen recording, entirely in-process (no FFmpeg, no child processes, no audio).
//!
//! Frames come from **Windows.Graphics.Capture** (the OS-sanctioned capture API,
//! which shows the system capture border while recording). They are cropped on
//! the GPU to the selected region, then either encoded to H.264 MP4 through
//! **Media Foundation** or streamed into an animated GIF, re-encoding only the
//! part of each frame that changed.

mod gif_out;
mod mp4;
mod wgc;

use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::Arc;
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

pub use snapit_gfx::Rect;

#[derive(Clone, Debug)]
pub enum Output {
    Mp4 { path: PathBuf, fps: u32, bitrate_kbps: u32 },
    Gif { path: PathBuf, fps: u32, max_seconds: u32 },
}

impl Output {
    pub fn path(&self) -> &PathBuf {
        match self {
            Output::Mp4 { path, .. } | Output::Gif { path, .. } => path,
        }
    }
    fn fps(&self) -> u32 {
        match self {
            Output::Mp4 { fps, .. } | Output::Gif { fps, .. } => (*fps).clamp(1, 60),
        }
    }
}

#[derive(Clone, Debug)]
pub struct Stats {
    pub path: PathBuf,
    pub frames: u32,
    pub seconds: f32,
    pub width: u32,
    pub height: u32,
}

/// A frame sink: MP4 or GIF.
trait Sink {
    fn frame(&mut self, bgra: &[u8], index: u64) -> Result<(), String>;
    fn finish(self: Box<Self>) -> Result<(), String>;
}

/// Keeps the multithreaded COM apartment alive for the whole process. WinRT activation
/// factories are cached process-wide, so letting the MTA shut down when a recording
/// thread exits would leave the next recording with a dangling factory.
pub fn ensure_mta() {
    static ONCE: std::sync::Once = std::sync::Once::new();
    ONCE.call_once(|| unsafe {
        // The usage count is never decremented, so the MTA lives until process exit.
        let _ = windows::Win32::System::Com::CoIncrementMTAUsage();
    });
}

pub struct Recorder {
    stop: Arc<AtomicBool>,
    frames: Arc<AtomicU32>,
    started: Instant,
    thread: Option<JoinHandle<Result<Stats, String>>>,
}

impl Recorder {
    /// Starts recording `region` (virtual-screen pixels) of monitor `monitor` (an HMONITOR,
    /// whose rect is `monitor_rect`). The region is clipped to that monitor.
    pub fn start(monitor: isize, monitor_rect: Rect, region: Rect, cursor: bool, out: Output) -> Result<Recorder, String> {
        ensure_mta();
        let local = region.intersect(monitor_rect).ok_or("the region is not on that monitor")?.offset(-monitor_rect.x, -monitor_rect.y);
        // H.264 needs even dimensions.
        let local = Rect::new(local.x, local.y, local.w & !1, local.h & !1);
        if local.w < 16 || local.h < 16 {
            return Err("the region is too small to record".into());
        }
        let stop = Arc::new(AtomicBool::new(false));
        let frames = Arc::new(AtomicU32::new(0));
        let (ready_tx, ready_rx) = std::sync::mpsc::channel::<Result<(), String>>();
        let (s2, f2) = (stop.clone(), frames.clone());
        let thread = std::thread::Builder::new()
            .name("snapit-record".into())
            .spawn(move || run(monitor, local, cursor, out, s2, f2, ready_tx))
            .map_err(|e| e.to_string())?;
        // Wait for capture to start so errors surface immediately.
        match ready_rx.recv_timeout(Duration::from_secs(10)) {
            Ok(Ok(())) => Ok(Recorder { stop, frames, started: Instant::now(), thread: Some(thread) }),
            Ok(Err(e)) => {
                let _ = thread.join();
                Err(e)
            }
            Err(_) => {
                stop.store(true, Ordering::SeqCst);
                Err("screen capture did not start".into())
            }
        }
    }

    pub fn elapsed(&self) -> Duration {
        self.started.elapsed()
    }

    pub fn frames(&self) -> u32 {
        self.frames.load(Ordering::Relaxed)
    }

    /// True once the recording stopped by itself (e.g. GIF length limit or an error).
    pub fn is_finished(&self) -> bool {
        self.thread.as_ref().is_none_or(|t| t.is_finished())
    }

    /// Signals stop and returns the join handle so the caller can wait off the UI thread.
    pub fn stop(mut self) -> JoinHandle<Result<Stats, String>> {
        self.stop.store(true, Ordering::SeqCst);
        self.thread.take().expect("recorder thread")
    }
}

impl Drop for Recorder {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
    }
}

fn run(
    monitor: isize,
    region: Rect,
    cursor: bool,
    out: Output,
    stop: Arc<AtomicBool>,
    frames: Arc<AtomicU32>,
    ready: std::sync::mpsc::Sender<Result<(), String>>,
) -> Result<Stats, String> {
    unsafe {
        let _ = windows::Win32::System::Com::CoInitializeEx(None, windows::Win32::System::Com::COINIT_MULTITHREADED);
    }
    let setup = (|| {
        let cap = wgc::Capture::new(monitor, region, cursor)?;
        let sink: Box<dyn Sink> = match &out {
            Output::Mp4 { path, fps, bitrate_kbps } => Box::new(mp4::Mp4::new(path, region.w as u32, region.h as u32, (*fps).clamp(1, 60), *bitrate_kbps)?),
            Output::Gif { path, fps, .. } => Box::new(gif_out::GifOut::new(path, region.w as u32, region.h as u32, (*fps).clamp(1, 50))?),
        };
        Ok::<_, String>((cap, sink))
    })();
    let (mut cap, mut sink) = match setup {
        Ok(v) => {
            let _ = ready.send(Ok(()));
            v
        }
        Err(e) => {
            let _ = ready.send(Err(e.clone()));
            return Err(e);
        }
    };

    let fps = out.fps();
    let interval = Duration::from_secs_f64(1.0 / fps as f64);
    let limit = match &out {
        Output::Gif { max_seconds, .. } => Some(Duration::from_secs((*max_seconds).max(1) as u64)),
        Output::Mp4 { .. } => None,
    };
    let start = Instant::now();
    let mut index: u64 = 0;
    let mut have_frame = false;
    let mut result = Ok(());
    while !stop.load(Ordering::SeqCst) {
        if limit.is_some_and(|l| start.elapsed() >= l) {
            break;
        }
        // Grab the newest frame (or keep the previous one if the screen didn't change).
        match cap.latest() {
            Ok(got) => have_frame |= got,
            Err(e) => {
                result = Err(e);
                break;
            }
        }
        if have_frame {
            if let Err(e) = sink.frame(cap.pixels(), index) {
                result = Err(e);
                break;
            }
            index += 1;
            frames.store(index as u32, Ordering::Relaxed);
        }
        let next = start + interval * (index as u32).max(1);
        let now = Instant::now();
        if next > now {
            std::thread::sleep(next - now);
        }
    }
    drop(cap);
    let finish = sink.finish();
    result?;
    finish?;
    Ok(Stats { path: out.path().clone(), frames: index as u32, seconds: start.elapsed().as_secs_f32(), width: region.w as u32, height: region.h as u32 })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Records ~1.5 s of a small corner of the primary monitor to MP4 and GIF.
    /// Ignored by default because Windows shows its capture border while it runs.
    #[test]
    #[ignore]
    fn records_mp4_and_gif() {
        use windows::Win32::Graphics::Gdi::{MonitorFromPoint, MONITOR_DEFAULTTOPRIMARY};
        let hmon = unsafe { MonitorFromPoint(windows::Win32::Foundation::POINT { x: 0, y: 0 }, MONITOR_DEFAULTTOPRIMARY) };
        let mut mi = windows::Win32::Graphics::Gdi::MONITORINFO { cbSize: std::mem::size_of::<windows::Win32::Graphics::Gdi::MONITORINFO>() as u32, ..Default::default() };
        unsafe {
            let _ = windows::Win32::Graphics::Gdi::GetMonitorInfoW(hmon, &mut mi);
        }
        let m = mi.rcMonitor;
        let mon = Rect::from_ltrb(m.left, m.top, m.right, m.bottom);
        let region = Rect::new(mon.x, mon.y, 160, 120);
        let dir = std::env::temp_dir().join(format!("snapit-rec-{}", std::process::id()));
        let only = std::env::var("SNAPIT_REC_ONLY").unwrap_or_default();
        let outs = [
            Output::Mp4 { path: dir.join("t.mp4"), fps: 20, bitrate_kbps: 2000 },
            Output::Gif { path: dir.join("t.gif"), fps: 10, max_seconds: 5 },
        ];
        for out in outs.into_iter().filter(|o| only.is_empty() || o.path().to_string_lossy().ends_with(only.as_str())) {
            let rec = Recorder::start(hmon.0 as isize, mon, region, true, out.clone()).expect("start");
            std::thread::sleep(Duration::from_millis(1500));
            let stats = rec.stop().join().unwrap().expect("finish");
            let bytes = std::fs::read(&stats.path).unwrap();
            assert!(stats.frames >= 5, "{stats:?}");
            match out {
                Output::Mp4 { .. } => assert_eq!(&bytes[4..8], b"ftyp"),
                Output::Gif { .. } => assert_eq!(&bytes[..6], b"GIF89a"),
            }
            eprintln!("{:?}: {} frames, {} bytes", stats.path.file_name().unwrap(), stats.frames, bytes.len());
        }
        let _ = std::fs::remove_dir_all(&dir);
    }
}
