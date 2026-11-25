//! Streaming animated-GIF writer.
//!
//! Each frame is compared with the previous one and only the bounding box of
//! changed pixels is quantised and written; unchanged frames just extend the
//! previous frame's delay. Encoding happens on its own thread behind a small
//! bounded queue so capture timing stays steady.

use std::fs::File;
use std::io::BufWriter;
use std::path::Path;
use std::sync::mpsc::{sync_channel, SyncSender, TrySendError};
use std::thread::JoinHandle;

use crate::Sink;

pub struct GifOut {
    tx: Option<SyncSender<Vec<u8>>>,
    worker: Option<JoinHandle<Result<(), String>>>,
}

struct Pending {
    frame: gif::Frame<'static>,
    delay_cs: u32,
}

/// Bounding box (x, y, w, h) of pixels that differ between two BGRA frames.
pub fn diff_box(a: &[u8], b: &[u8], w: usize, h: usize) -> Option<(usize, usize, usize, usize)> {
    let (mut x0, mut y0, mut x1, mut y1) = (w, h, 0, 0);
    for y in 0..h {
        let row = y * w * 4;
        let (ra, rb) = (&a[row..row + w * 4], &b[row..row + w * 4]);
        if ra == rb {
            continue;
        }
        let first = (0..w).find(|&x| ra[x * 4..x * 4 + 3] != rb[x * 4..x * 4 + 3]);
        if let Some(f) = first {
            let last = (0..w).rev().find(|&x| ra[x * 4..x * 4 + 3] != rb[x * 4..x * 4 + 3]).unwrap_or(f);
            x0 = x0.min(f);
            x1 = x1.max(last + 1);
            y0 = y0.min(y);
            y1 = y1.max(y + 1);
        }
    }
    (x1 > x0 && y1 > y0).then(|| (x0, y0, x1 - x0, y1 - y0))
}

impl GifOut {
    pub fn new(path: &Path, w: u32, h: u32, fps: u32) -> Result<GifOut, String> {
        if let Some(d) = path.parent() {
            std::fs::create_dir_all(d).map_err(|e| e.to_string())?;
        }
        let file = File::create(path).map_err(|e| e.to_string())?;
        let (tx, rx) = sync_channel::<Vec<u8>>(6);
        let frame_cs = (100.0 / fps as f32).round().max(2.0) as u32;
        let worker = std::thread::Builder::new()
            .name("snapit-gif".into())
            .spawn(move || -> Result<(), String> {
                let (wu, hu) = (w as usize, h as usize);
                let mut enc = gif::Encoder::new(BufWriter::new(file), w as u16, h as u16, &[]).map_err(|e| e.to_string())?;
                enc.set_repeat(gif::Repeat::Infinite).map_err(|e| e.to_string())?;
                let mut prev: Option<Vec<u8>> = None;
                let mut pending: Option<Pending> = None;
                let flush = |enc: &mut gif::Encoder<BufWriter<File>>, p: Pending| -> Result<(), String> {
                    let mut f = p.frame;
                    f.delay = p.delay_cs.min(u16::MAX as u32) as u16;
                    enc.write_frame(&f).map_err(|e| e.to_string())
                };
                for bgra in rx {
                    let rect = match &prev {
                        None => Some((0, 0, wu, hu)),
                        Some(p) => diff_box(p, &bgra, wu, hu),
                    };
                    match rect {
                        None => {
                            if let Some(p) = pending.as_mut() {
                                p.delay_cs += frame_cs;
                            }
                        }
                        Some((x, y, rw, rh)) => {
                            if let Some(p) = pending.take() {
                                flush(&mut enc, p)?;
                            }
                            let mut rgba = Vec::with_capacity(rw * rh * 4);
                            for yy in y..y + rh {
                                let i = (yy * wu + x) * 4;
                                for px in bgra[i..i + rw * 4].as_chunks::<4>().0 {
                                    rgba.extend_from_slice(&[px[2], px[1], px[0], 255]);
                                }
                            }
                            let mut f = gif::Frame::from_rgba_speed(rw as u16, rh as u16, &mut rgba, 10);
                            f.left = x as u16;
                            f.top = y as u16;
                            f.dispose = gif::DisposalMethod::Keep;
                            pending = Some(Pending { frame: f, delay_cs: frame_cs });
                        }
                    }
                    prev = Some(bgra);
                }
                if let Some(p) = pending.take() {
                    flush(&mut enc, p)?;
                }
                Ok(())
            })
            .map_err(|e| e.to_string())?;
        Ok(GifOut { tx: Some(tx), worker: Some(worker) })
    }
}

impl Sink for GifOut {
    fn frame(&mut self, bgra: &[u8], _index: u64) -> Result<(), String> {
        let Some(tx) = &self.tx else { return Ok(()) };
        match tx.try_send(bgra.to_vec()) {
            Ok(()) | Err(TrySendError::Full(_)) => Ok(()), // encoder busy: skip this frame
            Err(TrySendError::Disconnected(_)) => Err("GIF encoder stopped".into()),
        }
    }

    fn finish(mut self: Box<Self>) -> Result<(), String> {
        drop(self.tx.take());
        match self.worker.take().map(|w| w.join()) {
            Some(Ok(r)) => r,
            Some(Err(_)) => Err("GIF encoder crashed".into()),
            None => Ok(()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn diff_box_finds_changes() {
        let (w, h) = (8, 6);
        let a = vec![0u8; w * h * 4];
        let mut b = a.clone();
        assert_eq!(diff_box(&a, &b, w, h), None);
        b[(2 * w + 3) * 4] = 9;
        b[(4 * w + 5) * 4 + 1] = 9;
        assert_eq!(diff_box(&a, &b, w, h), Some((3, 2, 3, 3)));
    }

    #[test]
    fn writes_a_small_animated_gif() {
        let dir = std::env::temp_dir().join(format!("snapit-gif-{}", std::process::id()));
        let path = dir.join("t.gif");
        let mut g: Box<dyn Sink> = Box::new(GifOut::new(&path, 16, 16, 10).unwrap());
        let mut f = vec![255u8; 16 * 16 * 4];
        g.frame(&f, 0).unwrap();
        g.frame(&f, 1).unwrap(); // unchanged: extends delay
        f[0..4].copy_from_slice(&[0, 0, 255, 255]);
        g.frame(&f, 2).unwrap();
        g.finish().unwrap();
        let bytes = std::fs::read(&path).unwrap();
        assert_eq!(&bytes[..6], b"GIF89a");
        let mut opts = gif::DecodeOptions::new();
        opts.set_color_output(gif::ColorOutput::RGBA);
        let mut dec = opts.read_info(std::fs::File::open(&path).unwrap()).unwrap();
        let mut n = 0;
        while dec.read_next_frame().unwrap().is_some() {
            n += 1;
        }
        assert_eq!(n, 2);
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
