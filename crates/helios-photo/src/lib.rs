//! The photo domain of Helios.
//!
//! The core knows nothing about photography (see `docs/module-contract.md`,
//! "Layers"). This crate gives modules the photo vocabulary:
//!
//! - [`cfa`]: the color filter array of a sensor (Bayer, X-Trans);
//! - [`camera`]: what the camera recorded (levels, white balance, color
//!   matrix, exposure), offered to modules as declared context fields;
//! - [`color`]: color spaces and the working space of the pipe;
//! - [`compare`]: how far two images are, in Delta E 2000.
//!
//! A raw image is an ordinary one-channel core image holding the mosaic; its
//! [`camera::CameraInfo`] travels as context. No IO here: files are read by
//! the framework's loaders.

pub mod camera;
pub mod cfa;
pub mod color;
pub mod compare;

pub use camera::CameraInfo;
pub use cfa::{Cfa, Channel};
