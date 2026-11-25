//! H.264 MP4 through the Media Foundation sink writer (uses the GPU encoder when present).

use std::path::Path;

use windows::core::HSTRING;
use windows::Win32::Media::MediaFoundation::*;

use crate::Sink;

const MF_VERSION_FULL: u32 = 0x0002_0070;

pub struct Mp4 {
    writer: IMFSinkWriter,
    stream: u32,
    w: u32,
    h: u32,
    frame_100ns: i64,
}

fn e(err: windows::core::Error) -> String {
    let m = err.message().to_string();
    if m.is_empty() { format!("{err:?}") } else { m }
}

fn pack(hi: u32, lo: u32) -> u64 {
    ((hi as u64) << 32) | lo as u64
}

impl Mp4 {
    pub fn new(path: &Path, w: u32, h: u32, fps: u32, kbps: u32) -> Result<Mp4, String> {
        if let Some(d) = path.parent() {
            std::fs::create_dir_all(d).map_err(|e| e.to_string())?;
        }
        unsafe {
            MFStartup(MF_VERSION_FULL, MFSTARTUP_FULL).map_err(e)?;
            let mut attrs = None;
            MFCreateAttributes(&mut attrs, 2).map_err(e)?;
            let attrs = attrs.ok_or("no attributes")?;
            attrs.SetUINT32(&MF_READWRITE_ENABLE_HARDWARE_TRANSFORMS, 1).map_err(e)?;
            attrs.SetUINT32(&MF_SINK_WRITER_DISABLE_THROTTLING, 0).map_err(e)?;
            let writer = MFCreateSinkWriterFromURL(&HSTRING::from(path.as_os_str()), None, &attrs).map_err(e)?;

            let outt = MFCreateMediaType().map_err(e)?;
            outt.SetGUID(&MF_MT_MAJOR_TYPE, &MFMediaType_Video).map_err(e)?;
            outt.SetGUID(&MF_MT_SUBTYPE, &MFVideoFormat_H264).map_err(e)?;
            outt.SetUINT32(&MF_MT_AVG_BITRATE, kbps.clamp(500, 100_000) * 1000).map_err(e)?;
            outt.SetUINT32(&MF_MT_INTERLACE_MODE, MFVideoInterlace_Progressive.0 as u32).map_err(e)?;
            outt.SetUINT64(&MF_MT_FRAME_SIZE, pack(w, h)).map_err(e)?;
            outt.SetUINT64(&MF_MT_FRAME_RATE, pack(fps, 1)).map_err(e)?;
            outt.SetUINT64(&MF_MT_PIXEL_ASPECT_RATIO, pack(1, 1)).map_err(e)?;
            let stream = writer.AddStream(&outt).map_err(e)?;

            let int = MFCreateMediaType().map_err(e)?;
            int.SetGUID(&MF_MT_MAJOR_TYPE, &MFMediaType_Video).map_err(e)?;
            int.SetGUID(&MF_MT_SUBTYPE, &MFVideoFormat_RGB32).map_err(e)?;
            int.SetUINT32(&MF_MT_INTERLACE_MODE, MFVideoInterlace_Progressive.0 as u32).map_err(e)?;
            int.SetUINT64(&MF_MT_FRAME_SIZE, pack(w, h)).map_err(e)?;
            int.SetUINT64(&MF_MT_FRAME_RATE, pack(fps, 1)).map_err(e)?;
            int.SetUINT64(&MF_MT_PIXEL_ASPECT_RATIO, pack(1, 1)).map_err(e)?;
            writer.SetInputMediaType(stream, &int, None).map_err(e)?;
            writer.BeginWriting().map_err(e)?;
            Ok(Mp4 { writer, stream, w, h, frame_100ns: 10_000_000 / fps as i64 })
        }
    }
}

impl Sink for Mp4 {
    fn frame(&mut self, bgra: &[u8], index: u64) -> Result<(), String> {
        let row = self.w as usize * 4;
        let len = row * self.h as usize;
        unsafe {
            let buf = MFCreateMemoryBuffer(len as u32).map_err(e)?;
            let mut ptr = std::ptr::null_mut();
            buf.Lock(&mut ptr, None, None).map_err(e)?;
            // Uncompressed RGB in Media Foundation is bottom-up by default: write rows reversed.
            let dst = std::slice::from_raw_parts_mut(ptr, len);
            for y in 0..self.h as usize {
                let s = &bgra[y * row..(y + 1) * row];
                let d = (self.h as usize - 1 - y) * row;
                dst[d..d + row].copy_from_slice(s);
            }
            buf.Unlock().map_err(e)?;
            buf.SetCurrentLength(len as u32).map_err(e)?;
            let sample = MFCreateSample().map_err(e)?;
            sample.AddBuffer(&buf).map_err(e)?;
            sample.SetSampleTime(index as i64 * self.frame_100ns).map_err(e)?;
            sample.SetSampleDuration(self.frame_100ns).map_err(e)?;
            self.writer.WriteSample(self.stream, &sample).map_err(e)
        }
    }

    fn finish(self: Box<Self>) -> Result<(), String> {
        let r = unsafe { self.writer.Finalize().map_err(e) };
        drop(self);
        unsafe {
            let _ = MFShutdown();
        }
        r
    }
}
