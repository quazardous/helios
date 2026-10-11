//! Exports: the leaves of a network that write files.
//!
//! Writing a file is framework IO, never a module's (see
//! `docs/architecture.md`, "Data access"). An export renders a node over a
//! region, converts the result from the linear Rec.2020 working space to
//! sRGB, encodes it and writes it atomically: to a temporary file next to
//! the destination, then renamed, so a reader never sees half a file. Two
//! exports to the same destination never race: the caller runs them as
//! scheduler jobs holding the destination as a resource ([`destination`]).

use std::io::Cursor;
use std::path::{Path, PathBuf};
use std::sync::{Arc, OnceLock};

use helios_core::{
    Bus, ContentHash, Engine, Event, Graph, Image, JobId, JobSpec, NodeId, Priority, Region,
    Scheduler,
};
use helios_photo::color::{self, Mat3};
use image::codecs::jpeg::JpegEncoder;
use image::{DynamicImage, ImageBuffer, ImageFormat, Rgb};
use serde::{Deserialize, Serialize};

/// An output format.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "format", rename_all = "lowercase")]
pub enum Format {
    Jpeg {
        #[serde(default = "default_quality")]
        quality: u8,
    },
    Png {
        #[serde(default = "default_bits")]
        bits: u8,
    },
    /// 16-bit TIFF.
    Tiff,
}

fn default_quality() -> u8 {
    92
}

fn default_bits() -> u8 {
    8
}

impl Format {
    /// The format a file name asks for, by extension.
    pub fn from_path(path: &Path) -> Option<Self> {
        match path.extension()?.to_str()?.to_ascii_lowercase().as_str() {
            "jpg" | "jpeg" => Some(Format::Jpeg {
                quality: default_quality(),
            }),
            "png" => Some(Format::Png {
                bits: default_bits(),
            }),
            "tif" | "tiff" => Some(Format::Tiff),
            _ => None,
        }
    }
}

/// The scheduler resource that serializes writes to a path.
pub fn destination(path: &Path) -> String {
    format!("destination:{}", path.display())
}

/// Linear Rec.2020 to linear sRGB.
fn rec2020_to_srgb() -> Mat3 {
    let xyz_to_srgb = color::inverse(&color::SRGB_TO_XYZ).expect("sRGB primaries are invertible");
    color::mul(&xyz_to_srgb, &color::REC2020_TO_XYZ)
}

/// Encoded sRGB values in [0, 1], three per pixel, row by row.
fn to_srgb(image: &Image) -> Result<Vec<f64>, String> {
    let channels = image.channels() as usize;
    if channels != 1 && channels < 3 {
        return Err(format!("cannot export an image of {channels} channels"));
    }
    let m = rec2020_to_srgb();
    let mut out = Vec::with_capacity(image.region().area() * 3);
    for pixel in image.data().chunks_exact(channels) {
        let rgb = if channels == 1 {
            [f64::from(pixel[0]); 3]
        } else {
            [
                f64::from(pixel[0]),
                f64::from(pixel[1]),
                f64::from(pixel[2]),
            ]
        };
        for v in color::apply(&m, rgb) {
            out.push(color::srgb_encode(v.clamp(0.0, 1.0)));
        }
    }
    Ok(out)
}

/// Encodes an image (linear Rec.2020) into the bytes of a file.
pub fn encode(image: &Image, format: &Format) -> Result<Vec<u8>, String> {
    let (w, h) = (image.region().width, image.region().height);
    let srgb = to_srgb(image)?;
    let eight = || {
        let bytes: Vec<u8> = srgb.iter().map(|v| (v * 255.0).round() as u8).collect();
        ImageBuffer::<Rgb<u8>, _>::from_raw(w, h, bytes).expect("sizes match")
    };
    let sixteen = || {
        let words: Vec<u16> = srgb.iter().map(|v| (v * 65535.0).round() as u16).collect();
        ImageBuffer::<Rgb<u16>, _>::from_raw(w, h, words).expect("sizes match")
    };
    let mut out = Cursor::new(Vec::new());
    let result = match format {
        Format::Jpeg { quality } => DynamicImage::ImageRgb8(eight()).write_with_encoder(
            JpegEncoder::new_with_quality(&mut out, (*quality).clamp(1, 100)),
        ),
        Format::Png { bits: 8 } => {
            DynamicImage::ImageRgb8(eight()).write_to(&mut out, ImageFormat::Png)
        }
        Format::Png { bits: 16 } => {
            DynamicImage::ImageRgb16(sixteen()).write_to(&mut out, ImageFormat::Png)
        }
        Format::Png { bits } => return Err(format!("PNG has 8 or 16 bits, not {bits}")),
        Format::Tiff => DynamicImage::ImageRgb16(sixteen()).write_to(&mut out, ImageFormat::Tiff),
    };
    result.map_err(|e| e.to_string())?;
    Ok(out.into_inner())
}

/// Writes bytes so that `path` holds either its old content or the new one,
/// never a part: a temporary file next to it, then a rename.
pub fn write_atomic(path: &Path, bytes: &[u8]) -> Result<(), String> {
    let name = path
        .file_name()
        .ok_or_else(|| format!("{} is not a file path", path.display()))?;
    let temp = path.with_file_name(format!(
        ".{}.helios-{}",
        name.to_string_lossy(),
        std::process::id()
    ));
    let written = std::fs::write(&temp, bytes).and_then(|()| std::fs::rename(&temp, path));
    written.map_err(|e| {
        let _ = std::fs::remove_file(&temp);
        format!("cannot write {}: {e}", path.display())
    })
}

/// What an export wrote.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Exported {
    pub path: PathBuf,
    pub bytes: u64,
    /// Content hash of the file written.
    pub hash: ContentHash,
}

/// Renders a node's port over a region and writes it to `path`.
pub fn export(
    engine: &Engine,
    graph: &Graph,
    node: NodeId,
    port: &str,
    region: Region,
    path: &Path,
    format: &Format,
) -> Result<Exported, String> {
    let image = engine
        .render(graph, node, port, region)
        .map_err(|e| e.to_string())?;
    let bytes = encode(&image, format)?;
    write_atomic(path, &bytes)?;
    Ok(Exported {
        path: path.to_path_buf(),
        bytes: bytes.len() as u64,
        hash: ContentHash::of(&bytes),
    })
}

/// What to export: one port of one node, over a region, to a file.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Request {
    pub node: NodeId,
    pub port: String,
    pub region: Region,
    pub path: PathBuf,
    /// Read from the path's extension when absent.
    pub format: Option<Format>,
}

impl Request {
    pub fn format(&self) -> Result<Format, String> {
        self.format
            .clone()
            .or_else(|| Format::from_path(&self.path))
            .ok_or_else(|| format!("no format for {}", self.path.display()))
    }
}

/// A file was written.
#[derive(Serialize)]
pub struct Written {
    pub job: JobId,
    #[serde(flatten)]
    pub exported: Exported,
}

impl Event for Written {
    const KIND: &'static str = "export.written";
}

/// Queues an export as a batch job holding its destination: two exports to
/// the same file run one after the other, exports to different files run
/// side by side. The job announces the file on the bus once written.
pub fn submit(
    scheduler: &Scheduler,
    bus: Arc<Bus>,
    engine: Arc<Engine>,
    graph: Arc<Graph>,
    request: Request,
) -> Result<JobId, String> {
    let format = request.format()?;
    let path = request.path.clone();
    submit_with(scheduler, bus, &path, format, move || {
        engine
            .render(&graph, request.node, &request.port, request.region)
            .map_err(|e| e.to_string())
    })
}

/// [`submit`] for a caller that renders its own way, for instance from a
/// graph behind a lock it takes only once the job runs.
pub fn submit_with(
    scheduler: &Scheduler,
    bus: Arc<Bus>,
    path: &Path,
    format: Format,
    render: impl FnOnce() -> Result<Image, String> + Send + 'static,
) -> Result<JobId, String> {
    let dest = destination(path);
    scheduler.ensure_resource(&dest, 1);
    let spec = JobSpec::new(&format!("export {}", path.display()))
        .priority(Priority::Batch)
        .needs(&dest, 1);
    let path = path.to_path_buf();
    let id = Arc::new(OnceLock::new());
    let job_id = id.clone();
    let job = scheduler.submit_spec(spec, move |cancel| {
        if cancel.is_cancelled() {
            return Err("cancelled".into());
        }
        let bytes = encode(&render()?, &format)?;
        // Cancelled while rendering: nothing is written.
        if cancel.is_cancelled() {
            return Err("cancelled".into());
        }
        write_atomic(&path, &bytes)?;
        let exported = Exported {
            hash: ContentHash::of(&bytes),
            bytes: bytes.len() as u64,
            path,
        };
        // A worker can start the job before `submit_spec` returns its id.
        let job = *job_id.wait();
        bus.publish("export", None, &Written { job, exported });
        Ok(())
    });
    let _ = id.set(job);
    Ok(job)
}
