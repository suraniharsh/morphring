//! Conversion engines for MorphRing. Every engine wraps an external program
//! (ImageMagick, FFmpeg, LibreOffice) that is found at runtime, and implements
//! `morphring_core::Converter`.

pub mod discovery;
pub mod ffmpeg;
pub mod magick;
pub mod process;
pub mod registry;
pub mod soffice;
