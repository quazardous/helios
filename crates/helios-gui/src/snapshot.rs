//! Pictures of the interface taken by the framework itself, without a
//! window manager in the way: a named widget, a panel, the window. Headless
//! through egui_kittest's wgpu renderer, or from a real window through
//! egui's screenshot command. Recordings are sequences of such pictures,
//! encoded to MP4 by `ffmpeg` when it is on the PATH, kept as numbered PNG
//! files otherwise.

use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};

use egui::{ColorImage, Rect};
use image::RgbaImage;

/// An egui screenshot as an image.
pub fn from_color_image(image: &ColorImage) -> RgbaImage {
    let [w, h] = image.size;
    let bytes = image.pixels.iter().flat_map(|c| c.to_array()).collect();
    RgbaImage::from_raw(w as u32, h as u32, bytes).expect("egui image size")
}

/// The part of a full-window picture under `rect` (in points).
pub fn crop(full: &RgbaImage, rect: Rect, pixels_per_point: f32) -> RgbaImage {
    let r = rect * pixels_per_point;
    let x0 = (r.min.x.round().max(0.0) as u32).min(full.width());
    let y0 = (r.min.y.round().max(0.0) as u32).min(full.height());
    let x1 = (r.max.x.round().max(0.0) as u32).clamp(x0, full.width());
    let y1 = (r.max.y.round().max(0.0) as u32).clamp(y0, full.height());
    image::imageops::crop_imm(full, x0, y0, x1 - x0, y1 - y0).to_image()
}

/// Copies `frame` onto a canvas of the given size, top left aligned, so a
/// recording keeps one size when its target changes size.
fn fit(frame: &RgbaImage, width: u32, height: u32) -> RgbaImage {
    if frame.dimensions() == (width, height) {
        return frame.clone();
    }
    let mut out = RgbaImage::new(width, height);
    image::imageops::replace(&mut out, frame, 0, 0);
    out
}

enum Sink {
    Ffmpeg(Child),
    Pngs(PathBuf),
}

/// A recording in progress. Frames are stamped at a fixed rate, whatever
/// the time they took to draw, so a recording replays the same way.
pub struct Recorder {
    path: PathBuf,
    fps: u32,
    size: Option<(u32, u32)>,
    frames: u32,
    sink: Option<Sink>,
    force_pngs: bool,
}

impl Recorder {
    /// A recording to `path` (`.mp4`); the encoder starts with the first
    /// frame, when the size is known.
    pub fn new(path: impl Into<PathBuf>, fps: u32) -> Self {
        Recorder {
            path: path.into(),
            fps: fps.max(1),
            size: None,
            frames: 0,
            sink: None,
            force_pngs: false,
        }
    }

    /// Keeps numbered PNG files even when `ffmpeg` is there.
    pub fn pngs(mut self) -> Self {
        self.force_pngs = true;
        self
    }

    pub fn frames(&self) -> u32 {
        self.frames
    }

    fn png_dir(&self) -> PathBuf {
        self.path.with_extension("frames")
    }

    fn open(&mut self, width: u32, height: u32) -> Result<Sink, String> {
        if !self.force_pngs {
            let child = Command::new("ffmpeg")
                .args(["-loglevel", "error", "-y", "-f", "rawvideo"])
                .args(["-pix_fmt", "rgba", "-s", &format!("{width}x{height}")])
                .args(["-r", &self.fps.to_string(), "-i", "-"])
                // Even sizes for yuv420p.
                .args(["-vf", "pad=ceil(iw/2)*2:ceil(ih/2)*2"])
                .args(["-c:v", "libx264", "-pix_fmt", "yuv420p"])
                .arg(&self.path)
                .stdin(Stdio::piped())
                .spawn();
            if let Ok(child) = child {
                return Ok(Sink::Ffmpeg(child));
            }
        }
        let dir = self.png_dir();
        std::fs::create_dir_all(&dir).map_err(|e| format!("{}: {e}", dir.display()))?;
        Ok(Sink::Pngs(dir))
    }

    pub fn push(&mut self, frame: &RgbaImage) -> Result<(), String> {
        let (w, h) = *self.size.get_or_insert(frame.dimensions());
        if self.sink.is_none() {
            self.sink = Some(self.open(w, h)?);
        }
        let frame = fit(frame, w, h);
        match self.sink.as_mut().expect("opened") {
            Sink::Ffmpeg(child) => child
                .stdin
                .as_mut()
                .expect("piped")
                .write_all(frame.as_raw())
                .map_err(|e| format!("ffmpeg: {e}"))?,
            Sink::Pngs(dir) => {
                let file = dir.join(format!("{:05}.png", self.frames));
                frame
                    .save(&file)
                    .map_err(|e| format!("{}: {e}", file.display()))?;
            }
        }
        self.frames += 1;
        Ok(())
    }

    /// Ends the recording; says where it went.
    pub fn finish(mut self) -> Result<PathBuf, String> {
        match self.sink.take() {
            Some(Sink::Ffmpeg(mut child)) => {
                drop(child.stdin.take());
                let status = child.wait().map_err(|e| format!("ffmpeg: {e}"))?;
                if !status.success() {
                    return Err(format!("ffmpeg failed: {status}"));
                }
                Ok(self.path.clone())
            }
            Some(Sink::Pngs(dir)) => Ok(dir),
            None => Err("no frame was recorded".into()),
        }
    }
}

/// Saves a picture, making its directory.
pub fn save(image: &RgbaImage, path: &Path) -> Result<(), String> {
    if let Some(dir) = path.parent().filter(|d| !d.as_os_str().is_empty()) {
        std::fs::create_dir_all(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    }
    image
        .save(path)
        .map_err(|e| format!("{}: {e}", path.display()))
}

/// Saves float image data before any display transform, as a 16-bit
/// PNG: linear values in the working space, 0 to 1 over the 16 bits,
/// clipped (a scene picture for comparisons, not for viewing).
pub fn save_scene(scene: &helios_core::Image, path: &Path) -> Result<(), String> {
    let r = scene.region();
    let n = scene.channels() as usize;
    let mut out = image::ImageBuffer::<image::Rgba<u16>, Vec<u16>>::new(r.width, r.height);
    for (p, src) in out.pixels_mut().zip(scene.data().chunks_exact(n)) {
        let c = |i: usize| (src[i.min(n - 1)].clamp(0.0, 1.0) * 65535.0).round() as u16;
        *p = image::Rgba([c(0), c(1), c(2), if n >= 4 { c(3) } else { 65535 }]);
    }
    if let Some(dir) = path.parent().filter(|d| !d.as_os_str().is_empty()) {
        std::fs::create_dir_all(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    }
    out.save(path)
        .map_err(|e| format!("{}: {e}", path.display()))
}
