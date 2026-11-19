//! Image duplicates: exact pixel match plus a fast perceptual near match.
//!
//! The same rule as for documents applies: never hash raw file bytes. A PNG
//! re-saved as BMP, or a JPEG with its EXIF rewritten, is the same picture,
//! so the image is decoded first (EXIF orientation applied, transparency
//! composited over white where only luminance is needed) and the pixels are
//! what get hashed.
//!
//! Two layers, both computed in a single streaming pass over the decoded
//! rows, so no second copy of the image is ever built:
//!
//! * **exact**: SHA-256 over the dimensions and every pixel as RGBA8.
//! * **near**: a 128-bit [`PerceptualHash`]: a 64-bit DCT hash (low
//!   frequencies of a 32x32 luminance thumbnail, thresholded at their
//!   median) next to a 64-bit gradient hash (left-vs-right neighbours on a
//!   9x8 grid). The DCT half survives resizing, recompression and gamma;
//!   the gradient half rejects structurally different images the DCT half
//!   alone can confuse. Similarity is `1 - hamming / 128`.
//!
//! Near candidates come from multi-index hashing: split the 128 bits into
//! `k + 1` disjoint chunks; two hashes within `k` bits must agree exactly on
//! at least one chunk (pigeonhole), so a handful of sorted-table lookups
//! replaces the all-pairs comparison and loses no pair within `k`. Every
//! candidate is then verified by an exact popcount.
//!
//! Images with no detail (solid colours, blank scans) carry no perceptual
//! hash: their thumbnails cannot be told apart, so they only ever match by
//! exact pixels. Rotation, mirroring and cropping are not detected.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use image::metadata::Orientation;
use image::{DynamicImage, ImageDecoder, ImageReader};
use rayon::prelude::*;
use sha2::{Digest, Sha256};
use zenpixels::{AlphaMode, PixelFormat};

use crate::error::{Error, Result};
use crate::family::{Family, FamilyEdge, FamilyMember, Relation, UnionFind};

/// Perceptual similarity (`1 - hamming / 128`) required for a `near` image
/// match. Equals 12 differing bits of 128.
pub const DEFAULT_IMAGE_THRESHOLD: f64 = 0.90;
/// Width of a [`PerceptualHash`].
pub const IMAGE_HASH_BITS: u32 = 128;

/// Side of the luminance thumbnail the DCT hash is taken from.
const DCT_SIDE: usize = 32;
/// Low-frequency block kept from the DCT: 8x8 coefficients = 64 bits.
const DCT_KEEP: usize = 8;
/// Gradient grid: 9 columns x 8 rows gives 8 x 8 left-vs-right comparisons.
const GRAD_COLS: usize = 9;
const GRAD_ROWS: usize = 8;
/// Thumbnails whose luminance varies less than this (standard deviation on
/// a 0..255 scale) carry no structure worth hashing.
const FLAT_STDDEV: f64 = 1.5;
/// Decoded-image transient per pixel: the decoded buffer, an oriented copy
/// when EXIF demands a quarter turn, and the per-row scratch buffers.
const TRANSIENT_BYTES_PER_PIXEL: u64 = 8;
/// Largest decoded image `fingerprint` will materialise (header-declared
/// size), so a tiny file that claims gigapixels is refused, not allocated.
const MAX_DECODED_BYTES: u64 = 1 << 30;
/// Decoded-image transient per pixel for AVIF: the AV1 frame planes, the
/// RGB output, and an oriented copy (measured ~7.5 B/px peak at 12 MP).
const AVIF_TRANSIENT_BYTES_PER_PIXEL: u64 = 12;
/// Domain tag so an image hash never collides with a text hash.
const EXACT_DOMAIN: &[u8] = b"dupey-image-v1";

/// 128-bit perceptual hash: high 64 bits DCT, low 64 bits gradient.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct PerceptualHash(pub u128);

impl PerceptualHash {
    /// Differing bits, 0..=128.
    pub fn distance(self, other: Self) -> u32 {
        (self.0 ^ other.0).count_ones()
    }

    /// `1 - distance / 128`: 1.0 for identical hashes.
    pub fn similarity(self, other: Self) -> f64 {
        1.0 - f64::from(self.distance(other)) / f64::from(IMAGE_HASH_BITS)
    }

    /// 32 lowercase hex digits.
    pub fn to_hex(self) -> String {
        format!("{:032x}", self.0)
    }

    pub fn from_hex(s: &str) -> Option<Self> {
        if s.len() != 32 {
            return None;
        }
        u128::from_str_radix(s, 16).ok().map(Self)
    }
}

impl serde::Serialize for PerceptualHash {
    fn serialize<S: serde::Serializer>(
        &self,
        serializer: S,
    ) -> std::result::Result<S::Ok, S::Error> {
        serializer.serialize_str(&self.to_hex())
    }
}

/// What one image contributes to a scan.
#[derive(Debug, Clone)]
pub struct ImageFingerprint {
    /// Dimensions after EXIF orientation.
    pub width: u32,
    pub height: u32,
    /// SHA-256 (hex) of the oriented RGBA8 pixels.
    pub exact_hash: String,
    /// `None` for images with no detail to hash.
    pub hash: Option<PerceptualHash>,
}

fn reader(path: &Path) -> Result<ImageReader<std::io::BufReader<std::fs::File>>> {
    let io = |source| Error::Io {
        path: path.to_path_buf(),
        source,
    };
    ImageReader::open(path)
        .map_err(io)?
        .with_guessed_format()
        .map_err(io)
}

fn decode_error(path: &Path, e: image::ImageError) -> Error {
    Error::Extract {
        path: path.to_path_buf(),
        message: e.to_string(),
    }
}

/// Upper bound on the transient bytes [`fingerprint`] needs, from the image
/// header alone (no pixels are decoded). 0 when the header is unreadable;
/// [`fingerprint`] then reports the real error.
pub fn decode_cost(path: &Path) -> u64 {
    if is_avif(path) {
        return avif_probe(path)
            .map(|info| {
                u64::from(info.width) * u64::from(info.height) * AVIF_TRANSIENT_BYTES_PER_PIXEL
            })
            .unwrap_or(0);
    }
    reader(path)
        .and_then(|r| r.into_dimensions().map_err(|e| decode_error(path, e)))
        .map(|(w, h)| u64::from(w) * u64::from(h) * TRANSIENT_BYTES_PER_PIXEL)
        .unwrap_or(0)
}

/// Hamming budget for a similarity threshold: pairs within it are `near`.
///
/// Capped at 127 bits: a budget of the full width would make every pair a
/// candidate.
pub fn max_distance(threshold: f64) -> u32 {
    ((1.0 - threshold) * f64::from(IMAGE_HASH_BITS) + 1e-9)
        .floor()
        .clamp(0.0, f64::from(IMAGE_HASH_BITS - 1)) as u32
}

/// Decode `path` and compute its exact and perceptual fingerprints.
///
/// The codec is chosen from the file's content, not its extension. Animated
/// formats contribute their first frame. AVIF is decoded by a pure-Rust AV1
/// decoder; HEIC/HEIF (HEVC) is deliberately not supported.
pub fn fingerprint(path: &Path) -> Result<ImageFingerprint> {
    let img = if is_avif(path) {
        decode_avif_guarded(path, MAX_DECODED_BYTES)?
    } else {
        decode_raster(path)?
    };
    Ok(fingerprint_pixels(&img))
}

/// Decode a PNG/JPEG/GIF/BMP/WebP/TIFF with EXIF orientation applied.
fn decode_raster(path: &Path) -> Result<DynamicImage> {
    let mut decoder = reader(path)?
        .into_decoder()
        .map_err(|e| decode_error(path, e))?;
    let declared = decoder.total_bytes();
    if declared > MAX_DECODED_BYTES {
        return Err(too_large(path, declared, MAX_DECODED_BYTES));
    }
    let orientation = decoder.orientation().unwrap_or(Orientation::NoTransforms);
    let mut img = DynamicImage::from_decoder(decoder).map_err(|e| decode_error(path, e))?;
    if orientation != Orientation::NoTransforms {
        img.apply_orientation(orientation);
    }
    Ok(img)
}

fn too_large(path: &Path, declared: u64, limit: u64) -> Error {
    Error::Extract {
        path: path.to_path_buf(),
        message: format!("image too large to decode: {declared} bytes of pixels, limit {limit}"),
    }
}

/// Whether the file starts with an `ftyp` box naming `avif` or `avis`.
/// HEIC/HEIF carry other brands and are left to the raster path, which
/// reports them as unsupported.
fn is_avif(path: &Path) -> bool {
    use std::io::Read;
    let mut head = [0u8; 64];
    let Ok(mut file) = std::fs::File::open(path) else {
        return false;
    };
    let n = file.read(&mut head).unwrap_or(0);
    if n < 16 || &head[4..8] != b"ftyp" {
        return false;
    }
    let size = u32::from_be_bytes([head[0], head[1], head[2], head[3]]) as usize;
    let end = size.clamp(16, n);
    // major brand, then (skipping minor version) compatible brands.
    let major = &head[8..12];
    let compatible = head[16..end].chunks_exact(4);
    std::iter::once(major)
        .chain(compatible)
        .any(|b| b == b"avif" || b == b"avis")
}

/// One-line decoder error text. The decoder's errors carry a multi-line
/// `at <source file>:<line>` trace that names the machine that built it;
/// only the cause belongs in `errors[]`.
fn avif_message(e: &dyn std::fmt::Display) -> String {
    let text = e.to_string();
    format!(
        "avif: {}",
        text.lines().next().unwrap_or("decode error").trim()
    )
}

fn avif_decoder(path: &Path, data: &[u8], max_bytes: u64) -> Result<zenavif::ManagedAvifDecoder> {
    let avif_error = |e: &dyn std::fmt::Display| Error::Extract {
        path: path.to_path_buf(),
        message: avif_message(e),
    };
    // One thread: the scan already decodes many files in parallel.
    // Backstop for headers that lie: twice what the byte cap allows, so the
    // explicit size check above it reports ordinary oversize files.
    let max_pixels = u32::try_from(max_bytes / 3 * 2).unwrap_or(u32::MAX);
    let config = zenavif::DecoderConfig::new()
        .threads(1)
        .frame_size_limit(max_pixels);
    zenavif::ManagedAvifDecoder::new(data, &config).map_err(|e| avif_error(&e))
}

fn avif_probe(path: &Path) -> Result<zenavif::ImageInfo> {
    let data = std::fs::read(path).map_err(|source| Error::Io {
        path: path.to_path_buf(),
        source,
    })?;
    avif_decoder(path, &data, MAX_DECODED_BYTES)?
        .probe_info()
        .map_err(|e| Error::Extract {
            path: path.to_path_buf(),
            message: avif_message(&e),
        })
}

/// Stack for the AV1 decoder. Its frame and tile state lives on the stack and
/// overflows a 256 KiB caller; Windows' 1 MiB main thread overflows on real
/// photos. A dedicated thread makes the result independent of the caller.
const AVIF_DECODE_STACK_BYTES: usize = 64 << 20;

/// [`decode_avif`] on its own thread, with decoder panics turned into errors.
///
/// The AV1 decoder indexes out of bounds on some real files (ImageMagick
/// 4:4:4 screenshots hit its loop-restoration SIMD). One such file must
/// cost one `errors[]` entry, not the whole scan.
fn decode_avif_guarded(path: &Path, max_bytes: u64) -> Result<DynamicImage> {
    let crashed = || Error::Extract {
        path: path.to_path_buf(),
        message: "avif: decoder crashed on this file".into(),
    };
    std::thread::scope(|scope| {
        std::thread::Builder::new()
            .stack_size(AVIF_DECODE_STACK_BYTES)
            .spawn_scoped(scope, || decode_avif(path, max_bytes))
            .map_err(|e| Error::Io {
                path: path.to_path_buf(),
                source: e,
            })?
            .join()
            .unwrap_or_else(|_| Err(crashed()))
    })
}

/// Decode an AVIF to 8/16-bit pixels with `irot`/`imir` applied. Rotation
/// comes first, then the mirror, the order the container defines.
fn decode_avif(path: &Path, max_bytes: u64) -> Result<DynamicImage> {
    let err = |message: String| Error::Extract {
        path: path.to_path_buf(),
        message,
    };
    let data = std::fs::read(path).map_err(|source| Error::Io {
        path: path.to_path_buf(),
        source,
    })?;
    let mut decoder = avif_decoder(path, &data, max_bytes)?;
    let info = decoder.probe_info().map_err(|e| err(avif_message(&e)))?;
    let channels = if info.has_alpha { 4 } else { 3 };
    let sample = if info.bit_depth > 8 { 2 } else { 1 };
    let declared = u64::from(info.width) * u64::from(info.height) * channels * sample;
    if declared > max_bytes {
        return Err(too_large(path, declared, max_bytes));
    }
    let (pixels, info) = decoder
        .decode_full(&zenavif::Unstoppable)
        .map_err(|e| err(avif_message(&e)))?;
    drop(data);
    // zenavif converts an identity matrix (lossless RGB stored as GBR
    // planes) as if it were BT.601, which silently corrupts colours. A wrong
    // pixel hash is worse than no hash: refuse the file.
    if info.matrix_coefficients == zenavif::MatrixCoefficients::IDENTITY {
        return Err(err(
            "avif: lossless RGB (identity matrix) is not supported by the decoder".into(),
        ));
    }
    let mut img = avif_to_image(pixels).map_err(err)?;
    let rotation = match info.rotation.map_or(0, |r| r.angle) {
        90 => Orientation::Rotate270,
        180 => Orientation::Rotate180,
        270 => Orientation::Rotate90,
        _ => Orientation::NoTransforms,
    };
    // libheif and libavif write axis 1 for a left-right flip (EXIF 2) and
    // axis 0 for a top-bottom flip; heif-dec agrees on the fixtures.
    let mirror = match info.mirror.map(|m| m.axis) {
        Some(0) => Orientation::FlipVertical,
        Some(_) => Orientation::FlipHorizontal,
        None => Orientation::NoTransforms,
    };
    for step in [rotation, mirror] {
        if step != Orientation::NoTransforms {
            img.apply_orientation(step);
        }
    }
    Ok(img)
}

/// Move the decoder's pixel buffer into a `DynamicImage` without a copy
/// when its rows are tightly packed.
fn avif_to_image(pixels: zenavif::PixelBuffer) -> std::result::Result<DynamicImage, String> {
    let (w, h) = (pixels.width(), pixels.height());
    let descriptor = pixels.descriptor();
    // Straight alpha is guaranteed by the decoder; premultiplied input is
    // un-premultiplied before it gets here.
    debug_assert!(descriptor.alpha != Some(AlphaMode::Premultiplied));
    let format = descriptor.format;
    let row_bytes = w as usize * format.bytes_per_pixel();
    let stride = pixels.stride();
    let mut bytes = if stride == row_bytes {
        pixels.into_vec()
    } else {
        let slice = pixels.as_slice();
        let mut packed = Vec::with_capacity(row_bytes * h as usize);
        for y in 0..h {
            packed.extend_from_slice(&slice.row(y)[..row_bytes]);
        }
        packed
    };
    bytes.truncate(row_bytes * h as usize);
    let u16s = |bytes: &[u8]| -> Vec<u16> {
        bytes
            .chunks_exact(2)
            .map(|b| u16::from_ne_bytes([b[0], b[1]]))
            .collect()
    };
    let bad = || format!("avif: unexpected pixel buffer for {w}x{h} {format:?}");
    Ok(match format {
        PixelFormat::Rgb8 => {
            DynamicImage::ImageRgb8(image::RgbImage::from_raw(w, h, bytes).ok_or_else(bad)?)
        }
        PixelFormat::Rgba8 => {
            DynamicImage::ImageRgba8(image::RgbaImage::from_raw(w, h, bytes).ok_or_else(bad)?)
        }
        PixelFormat::Gray8 => {
            DynamicImage::ImageLuma8(image::GrayImage::from_raw(w, h, bytes).ok_or_else(bad)?)
        }
        PixelFormat::GrayA8 => {
            DynamicImage::ImageLumaA8(image::GrayAlphaImage::from_raw(w, h, bytes).ok_or_else(bad)?)
        }
        PixelFormat::Rgb16 => DynamicImage::ImageRgb16(
            image::ImageBuffer::from_raw(w, h, u16s(&bytes)).ok_or_else(bad)?,
        ),
        PixelFormat::Rgba16 => DynamicImage::ImageRgba16(
            image::ImageBuffer::from_raw(w, h, u16s(&bytes)).ok_or_else(bad)?,
        ),
        PixelFormat::Gray16 => DynamicImage::ImageLuma16(
            image::ImageBuffer::from_raw(w, h, u16s(&bytes)).ok_or_else(bad)?,
        ),
        PixelFormat::GrayA16 => DynamicImage::ImageLumaA16(
            image::ImageBuffer::from_raw(w, h, u16s(&bytes)).ok_or_else(bad)?,
        ),
        other => return Err(format!("avif: unsupported pixel format {other:?}")),
    })
}

#[derive(Clone, Copy)]
enum Layout {
    L,
    La,
    Rgb,
    Rgba,
}

/// Raw samples of a decoded image: 8-bit bytes or 16-bit words.
enum Samples<'a> {
    Eight(Layout, &'a [u8]),
    Sixteen(Layout, &'a [u16]),
}

impl Layout {
    fn channels(self) -> usize {
        match self {
            Self::L => 1,
            Self::La => 2,
            Self::Rgb => 3,
            Self::Rgba => 4,
        }
    }
}

fn luma(r: u8, g: u8, b: u8) -> u8 {
    // Rec. 709 weights scaled to 256.
    ((u32::from(r) * 54 + u32::from(g) * 183 + u32::from(b) * 19) >> 8) as u8
}

fn luma16(r: u16, g: u16, b: u16) -> u16 {
    ((u32::from(r) * 54 + u32::from(g) * 183 + u32::from(b) * 19) >> 8) as u16
}

fn over_white16(c: u16, alpha: u16) -> u16 {
    let (c, a) = (u64::from(c), u64::from(alpha));
    ((c * a + 65535 * (65535 - a) + 32767) / 65535) as u16
}

fn over_white(c: u8, alpha: u8) -> u8 {
    let (c, a) = (u32::from(c), u32::from(alpha));
    ((c * a + 255 * (255 - a) + 127) / 255) as u8
}

/// One pass over the rows: SHA-256 of the RGBA8 pixels and both luminance
/// grids, with nothing materialised but one row of scratch.
fn fingerprint_pixels(img: &DynamicImage) -> ImageFingerprint {
    let converted;
    let samples = match img {
        DynamicImage::ImageLuma8(b) => Samples::Eight(Layout::L, b.as_raw()),
        DynamicImage::ImageLumaA8(b) => Samples::Eight(Layout::La, b.as_raw()),
        DynamicImage::ImageRgb8(b) => Samples::Eight(Layout::Rgb, b.as_raw()),
        DynamicImage::ImageRgba8(b) => Samples::Eight(Layout::Rgba, b.as_raw()),
        DynamicImage::ImageLuma16(b) => Samples::Sixteen(Layout::L, b.as_raw()),
        DynamicImage::ImageLumaA16(b) => Samples::Sixteen(Layout::La, b.as_raw()),
        DynamicImage::ImageRgb16(b) => Samples::Sixteen(Layout::Rgb, b.as_raw()),
        DynamicImage::ImageRgba16(b) => Samples::Sixteen(Layout::Rgba, b.as_raw()),
        other => {
            // Float images: quantise once to RGBA8.
            converted = other.to_rgba8();
            Samples::Eight(Layout::Rgba, converted.as_raw())
        }
    };
    let (w, h) = (img.width() as usize, img.height() as usize);

    let mut hasher = Sha256::new();
    hasher.update(EXACT_DOMAIN);
    hasher.update(img.width().to_le_bytes());
    hasher.update(img.height().to_le_bytes());
    let mut thumb = Grid::new(w, h, DCT_SIDE, DCT_SIDE);
    let mut grad = Grid::new(w, h, GRAD_COLS, GRAD_ROWS);
    let mut rgba = vec![0u8; w * 4];
    // Luminance keeps the source's own precision (0..=255 for 8-bit images,
    // 0..=65535 for 16-bit ones), so a dim 10-bit decode is not crushed
    // into a handful of 8-bit levels before it is hashed.
    let mut lum = vec![0u16; w];
    let flat_stddev = match samples {
        Samples::Eight(..) => FLAT_STDDEV,
        // The same 1.5 levels, in units of a 10-bit sample (the lowest depth
        // that reaches this path): 64 sixteen-bit counts per level.
        Samples::Sixteen(..) => FLAT_STDDEV * 64.0,
    };

    for y in 0..h {
        match samples {
            Samples::Eight(layout, bytes) => {
                let stride = w * layout.channels();
                let row = &bytes[y * stride..(y + 1) * stride];
                match layout {
                    Layout::L => {
                        for (x, &l) in row.iter().enumerate() {
                            rgba[x * 4..x * 4 + 4].copy_from_slice(&[l, l, l, 255]);
                            lum[x] = u16::from(l);
                        }
                    }
                    Layout::La => {
                        for (x, px) in row.chunks_exact(2).enumerate() {
                            rgba[x * 4..x * 4 + 4].copy_from_slice(&[px[0], px[0], px[0], px[1]]);
                            lum[x] = u16::from(over_white(px[0], px[1]));
                        }
                    }
                    Layout::Rgb => {
                        for (x, px) in row.chunks_exact(3).enumerate() {
                            rgba[x * 4..x * 4 + 4].copy_from_slice(&[px[0], px[1], px[2], 255]);
                            lum[x] = u16::from(luma(px[0], px[1], px[2]));
                        }
                    }
                    Layout::Rgba => {
                        rgba.copy_from_slice(row);
                        for (x, px) in row.chunks_exact(4).enumerate() {
                            lum[x] = u16::from(if px[3] == 255 {
                                luma(px[0], px[1], px[2])
                            } else {
                                luma(
                                    over_white(px[0], px[3]),
                                    over_white(px[1], px[3]),
                                    over_white(px[2], px[3]),
                                )
                            });
                        }
                    }
                }
            }
            Samples::Sixteen(layout, words) => {
                let stride = w * layout.channels();
                let row = &words[y * stride..(y + 1) * stride];
                let hi = |v: u16| (v >> 8) as u8;
                match layout {
                    Layout::L => {
                        for (x, &l) in row.iter().enumerate() {
                            rgba[x * 4..x * 4 + 4].copy_from_slice(&[hi(l), hi(l), hi(l), 255]);
                            lum[x] = l;
                        }
                    }
                    Layout::La => {
                        for (x, px) in row.chunks_exact(2).enumerate() {
                            rgba[x * 4..x * 4 + 4].copy_from_slice(&[
                                hi(px[0]),
                                hi(px[0]),
                                hi(px[0]),
                                hi(px[1]),
                            ]);
                            lum[x] = over_white16(px[0], px[1]);
                        }
                    }
                    Layout::Rgb => {
                        for (x, px) in row.chunks_exact(3).enumerate() {
                            rgba[x * 4..x * 4 + 4].copy_from_slice(&[
                                hi(px[0]),
                                hi(px[1]),
                                hi(px[2]),
                                255,
                            ]);
                            lum[x] = luma16(px[0], px[1], px[2]);
                        }
                    }
                    Layout::Rgba => {
                        for (x, px) in row.chunks_exact(4).enumerate() {
                            rgba[x * 4..x * 4 + 4].copy_from_slice(&[
                                hi(px[0]),
                                hi(px[1]),
                                hi(px[2]),
                                hi(px[3]),
                            ]);
                            lum[x] = if px[3] == u16::MAX {
                                luma16(px[0], px[1], px[2])
                            } else {
                                luma16(
                                    over_white16(px[0], px[3]),
                                    over_white16(px[1], px[3]),
                                    over_white16(px[2], px[3]),
                                )
                            };
                        }
                    }
                }
            }
        }
        hasher.update(&rgba);
        thumb.push_row(y, &lum);
        grad.push_row(y, &lum);
    }

    let hash = dct_hash(&thumb.finish(), flat_stddev).map(|dct| {
        let gradient = gradient_hash(&grad.finish());
        PerceptualHash(u128::from(dct) << 64 | u128::from(gradient))
    });
    ImageFingerprint {
        width: img.width(),
        height: img.height(),
        exact_hash: hex::encode(hasher.finalize()),
        hash,
    }
}

/// Area-averaging downscale of a luminance image to `cols x rows` cells,
/// fed one row at a time. Every source pixel lands in the cell it overlaps;
/// when the source is smaller than the grid, cells repeat source pixels.
struct Grid {
    cols: usize,
    rows: usize,
    /// Half-open source-column range of each cell column.
    x_range: Vec<(usize, usize)>,
    /// Half-open range of cell rows each source row feeds.
    y_cells: Vec<(usize, usize)>,
    /// Sum over source rows of the per-row cell averages.
    acc: Vec<f64>,
    /// Source rows accumulated into each cell row.
    seen: Vec<u32>,
}

impl Grid {
    fn new(w: usize, h: usize, cols: usize, rows: usize) -> Self {
        let span = |i: usize, n: usize, len: usize| {
            let lo = i * len / n;
            (lo, ((i + 1) * len / n).max(lo + 1).min(len))
        };
        let x_range = (0..cols).map(|c| span(c, cols, w)).collect();
        let mut y_cells = vec![(usize::MAX, 0usize); h];
        for r in 0..rows {
            let (lo, hi) = span(r, rows, h);
            for cell in &mut y_cells[lo..hi] {
                cell.0 = cell.0.min(r);
                cell.1 = cell.1.max(r + 1);
            }
        }
        Self {
            cols,
            rows,
            x_range,
            y_cells,
            acc: vec![0.0; cols * rows],
            seen: vec![0; rows],
        }
    }

    fn push_row(&mut self, y: usize, lum: &[u16]) {
        let (r0, r1) = self.y_cells[y];
        if r0 >= r1 {
            return;
        }
        for c in 0..self.cols {
            let (x0, x1) = self.x_range[c];
            let sum: u32 = lum[x0..x1].iter().map(|&v| u32::from(v)).sum();
            let avg = f64::from(sum) / (x1 - x0) as f64;
            for r in r0..r1 {
                self.acc[r * self.cols + c] += avg;
            }
        }
        for r in r0..r1 {
            self.seen[r] += 1;
        }
    }

    fn finish(&self) -> Vec<f64> {
        let mut out = self.acc.clone();
        for r in 0..self.rows {
            // Every cell row is fed by at least one source row: spans are
            // never empty.
            let n = f64::from(self.seen[r].max(1));
            for v in &mut out[r * self.cols..(r + 1) * self.cols] {
                *v /= n;
            }
        }
        out
    }
}

/// 64-bit DCT hash of a 32x32 luminance thumbnail, or `None` when it is
/// flat. The DC term is left out (it only encodes brightness); every other
/// kept coefficient contributes one bit: above the median of those, or not.
fn dct_hash(thumb: &[f64], flat_stddev: f64) -> Option<u64> {
    let n = DCT_SIDE as f64;
    let mean = thumb.iter().sum::<f64>() / thumb.len() as f64;
    let variance = thumb.iter().map(|v| (v - mean).powi(2)).sum::<f64>() / thumb.len() as f64;
    if variance.sqrt() < flat_stddev {
        return None;
    }
    // basis[u][x] = cos(pi * (2x + 1) * u / 2N)
    let mut basis = [[0.0f64; DCT_SIDE]; DCT_KEEP];
    for (u, row) in basis.iter_mut().enumerate() {
        for (x, v) in row.iter_mut().enumerate() {
            *v = (std::f64::consts::PI * (2.0 * x as f64 + 1.0) * u as f64 / (2.0 * n)).cos();
        }
    }
    // Rows first: tmp[u][x] = sum_y basis[u][y] * thumb[y][x].
    let mut tmp = [[0.0f64; DCT_SIDE]; DCT_KEEP];
    for u in 0..DCT_KEEP {
        for y in 0..DCT_SIDE {
            let b = basis[u][y];
            for x in 0..DCT_SIDE {
                tmp[u][x] += b * thumb[y * DCT_SIDE + x];
            }
        }
    }
    let mut coef = [0.0f64; DCT_KEEP * DCT_KEEP];
    for u in 0..DCT_KEEP {
        for v in 0..DCT_KEEP {
            coef[u * DCT_KEEP + v] = (0..DCT_SIDE).map(|x| tmp[u][x] * basis[v][x]).sum();
        }
    }
    let mut ac: Vec<f64> = coef[1..].to_vec();
    ac.sort_by(|a, b| a.total_cmp(b));
    let median = ac[ac.len() / 2];
    let mut bits = 0u64;
    for (i, &c) in coef.iter().enumerate().skip(1) {
        if c > median {
            bits |= 1 << i;
        }
    }
    Some(bits)
}

/// 64-bit gradient hash: bit `r * 8 + c` is set when cell `(r, c)` is
/// brighter than its right neighbour.
fn gradient_hash(grid: &[f64]) -> u64 {
    let mut bits = 0u64;
    for r in 0..GRAD_ROWS {
        for c in 0..GRAD_COLS - 1 {
            if grid[r * GRAD_COLS + c] > grid[r * GRAD_COLS + c + 1] {
                bits |= 1 << (r * (GRAD_COLS - 1) + c);
            }
        }
    }
    bits
}

/// One scanned image ready for clustering.
#[derive(Debug, Clone)]
pub struct ScannedImage {
    pub path: PathBuf,
    pub exact_hash: String,
    pub hash: Option<PerceptualHash>,
}

impl ScannedImage {
    pub fn new(path: PathBuf, fp: &ImageFingerprint) -> Self {
        Self {
            path,
            exact_hash: fp.exact_hash.clone(),
            hash: fp.hash,
        }
    }
}

/// A verified image pair, by index.
#[derive(Debug, Clone, Copy)]
struct ImageEdge {
    relation: Relation,
    a: usize,
    b: usize,
    distance: Option<u32>,
}

impl ImageEdge {
    fn similarity(&self) -> f64 {
        self.distance
            .map_or(1.0, |d| 1.0 - f64::from(d) / f64::from(IMAGE_HASH_BITS))
    }

    /// Exact beats near, then the closer pair wins.
    fn strength(&self) -> f64 {
        let base = if self.relation == Relation::Exact {
            2.0
        } else {
            1.0
        };
        base + self.similarity()
    }
}

/// Group images into families of 2+ members: pixel-identical copies
/// (`exact`) and perceptual matches within `threshold` (`near`).
///
/// Perceptual matches are transitive, as for documents: a chain of similar
/// frames is one family, with every merging pair kept as an edge. Family ids
/// start at 0 and follow the order of each family's first image.
pub fn cluster_images(images: &[ScannedImage], threshold: f64) -> Vec<Family> {
    let k = max_distance(threshold);
    let mut uf = UnionFind::new(images.len());
    let mut edges: Vec<ImageEdge> = Vec::new();

    // Exact: identical decoded pixels. The first image of a group stands in
    // for it in the perceptual stage.
    let mut by_hash: HashMap<&str, Vec<usize>> = HashMap::new();
    for (i, img) in images.iter().enumerate() {
        by_hash.entry(&img.exact_hash).or_default().push(i);
    }
    let mut reps: Vec<usize> = Vec::new();
    for (i, img) in images.iter().enumerate() {
        let group = &by_hash[img.exact_hash.as_str()];
        if group[0] != i {
            uf.union(group[0], i);
            edges.push(ImageEdge {
                relation: Relation::Exact,
                a: group[0],
                b: i,
                distance: images[group[0]].hash.map(|_| 0),
            });
        } else if img.hash.is_some() {
            reps.push(i);
        }
    }

    // Near: multi-index candidates over the representatives, verified by
    // popcount, merged in ascending pair order so the result never depends
    // on thread scheduling.
    let hashes: Vec<u128> = reps
        .iter()
        .map(|&i| images[i].hash.map_or(0, |h| h.0))
        .collect();
    let index = MultiIndex::build(&hashes, k);
    const CHUNK: usize = 1024;
    for start in (0..reps.len()).step_by(CHUNK) {
        let end = (start + CHUNK).min(reps.len());
        let found: Vec<Vec<(usize, u32)>> = (start..end)
            .into_par_iter()
            .map(|p| index.within(&hashes, p, k))
            .collect();
        for (p, partners) in (start..end).zip(found) {
            for (q, distance) in partners {
                let (a, b) = (reps[p], reps[q]);
                if uf.find(a) != uf.find(b) {
                    uf.union(a, b);
                    edges.push(ImageEdge {
                        relation: Relation::Near,
                        a,
                        b,
                        distance: Some(distance),
                    });
                }
            }
        }
    }

    assemble(images, &mut uf, edges)
}

fn assemble(images: &[ScannedImage], uf: &mut UnionFind, edges: Vec<ImageEdge>) -> Vec<Family> {
    let mut components: HashMap<usize, Vec<usize>> = HashMap::new();
    for i in 0..images.len() {
        components.entry(uf.find(i)).or_default().push(i);
    }
    let mut groups: Vec<Vec<usize>> = components.into_values().filter(|g| g.len() >= 2).collect();
    groups.sort_by_key(|g| g[0]);

    let mut family_of: Vec<Option<usize>> = vec![None; images.len()];
    for (id, group) in groups.iter().enumerate() {
        for &i in group {
            family_of[i] = Some(id);
        }
    }
    let mut by_family: Vec<Vec<ImageEdge>> = vec![Vec::new(); groups.len()];
    for edge in edges {
        if let Some(id) = family_of[edge.a] {
            by_family[id].push(edge);
        }
    }

    groups
        .into_iter()
        .zip(by_family)
        .enumerate()
        .map(|(id, (group, mut edges))| {
            edges.sort_by(|x, y| {
                y.strength()
                    .total_cmp(&x.strength())
                    .then_with(|| (x.a, x.b).cmp(&(y.a, y.b)))
            });
            let members = group.iter().map(|&i| member(images, i, &edges)).collect();
            Family {
                id: id as u32,
                members,
                edges: edges
                    .iter()
                    .map(|e| FamilyEdge {
                        relation: e.relation,
                        a: images[e.a].path.clone(),
                        b: images[e.b].path.clone(),
                        near_score: e.similarity(),
                        jaccard: None,
                        containment: None,
                        hamming: e.distance,
                    })
                    .collect(),
            }
        })
        .collect()
}

/// Describe a member by its strongest incident edge. `edges` is sorted
/// strongest first.
fn member(images: &[ScannedImage], i: usize, edges: &[ImageEdge]) -> FamilyMember {
    let edge = edges.iter().find(|e| e.a == i || e.b == i);
    FamilyMember {
        path: images[i].path.clone(),
        exact_hash: images[i].exact_hash.clone(),
        relation: edge.map_or(Relation::Near, |e| e.relation),
        joined_with: edge.map(|e| images[if e.a == i { e.b } else { e.a }].path.clone()),
        near_score: edge.map(ImageEdge::similarity),
        jaccard: None,
        containment: None,
        hamming: edge.and_then(|e| e.distance),
    }
}

/// Multi-index hashing over 128-bit hashes.
///
/// The bits are split into `k + 1` disjoint chunks. Two hashes at Hamming
/// distance `<= k` differ in at most `k` chunks, so at least one chunk is
/// identical: looking up each chunk of a query in a table sorted by that
/// chunk finds every pair within `k`, whatever the corpus size.
struct MultiIndex {
    /// Bit range `[lo, hi)` of each chunk.
    bounds: Vec<(u32, u32)>,
    /// Per chunk: `(chunk value, hash position)`, sorted.
    tables: Vec<Vec<(u64, u32)>>,
}

fn chunk_value(hash: u128, lo: u32, hi: u32) -> u64 {
    let width = hi - lo;
    let mask = if width == 64 {
        u64::MAX
    } else {
        (1u64 << width) - 1
    };
    (hash >> lo) as u64 & mask
}

impl MultiIndex {
    fn build(hashes: &[u128], k: u32) -> Self {
        // At least two chunks so a chunk never needs more than 64 bits.
        let chunks = (k as usize + 1).clamp(2, IMAGE_HASH_BITS as usize);
        let bounds: Vec<(u32, u32)> = (0..chunks)
            .map(|c| {
                (
                    (c as u32 * IMAGE_HASH_BITS) / chunks as u32,
                    ((c as u32 + 1) * IMAGE_HASH_BITS) / chunks as u32,
                )
            })
            .collect();
        let tables = bounds
            .par_iter()
            .map(|&(lo, hi)| {
                let mut table: Vec<(u64, u32)> = hashes
                    .iter()
                    .enumerate()
                    .map(|(p, &h)| (chunk_value(h, lo, hi), p as u32))
                    .collect();
                table.sort_unstable();
                table
            })
            .collect();
        Self { bounds, tables }
    }

    /// Positions `q > p` within `k` bits of `hashes[p]`, ascending, with
    /// their distances.
    fn within(&self, hashes: &[u128], p: usize, k: u32) -> Vec<(usize, u32)> {
        let query = hashes[p];
        let mut found: Vec<u32> = Vec::new();
        for (&(lo, hi), table) in self.bounds.iter().zip(&self.tables) {
            let key = chunk_value(query, lo, hi);
            // Entries are sorted by (value, position): start past `p`.
            let start = table.partition_point(|&e| e < (key, p as u32 + 1));
            found.extend(
                table[start..]
                    .iter()
                    .take_while(|&&(value, _)| value == key)
                    .map(|&(_, q)| q),
            );
        }
        found.sort_unstable();
        found.dedup();
        found
            .into_iter()
            .filter_map(|q| {
                let d = (query ^ hashes[q as usize]).count_ones();
                (d <= k).then_some((q as usize, d))
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Lcg(u64);
    impl Lcg {
        fn next(&mut self) -> u64 {
            self.0 = self
                .0
                .wrapping_mul(6364136223846793005)
                .wrapping_add(1442695040888963407);
            self.0
        }
        fn hash(&mut self) -> u128 {
            u128::from(self.next()) << 64 | u128::from(self.next())
        }
    }

    fn image(i: usize, hash: Option<u128>, exact: &str) -> ScannedImage {
        ScannedImage {
            path: PathBuf::from(format!("img{i}")),
            exact_hash: exact.to_string(),
            hash: hash.map(PerceptualHash),
        }
    }

    /// Partition into components by testing every pair: the definition the
    /// multi-index must reproduce.
    fn brute_force(images: &[ScannedImage], k: u32) -> Vec<Vec<usize>> {
        let mut uf = UnionFind::new(images.len());
        for i in 0..images.len() {
            for j in i + 1..images.len() {
                let same = images[i].exact_hash == images[j].exact_hash;
                let close = match (images[i].hash, images[j].hash) {
                    (Some(a), Some(b)) => a.distance(b) <= k,
                    _ => false,
                };
                if same || close {
                    uf.union(i, j);
                }
            }
        }
        let mut groups: HashMap<usize, Vec<usize>> = HashMap::new();
        for i in 0..images.len() {
            groups.entry(uf.find(i)).or_default().push(i);
        }
        let mut out: Vec<Vec<usize>> = groups.into_values().filter(|g| g.len() >= 2).collect();
        out.sort();
        out
    }

    fn partition(families: &[Family]) -> Vec<Vec<usize>> {
        let mut out: Vec<Vec<usize>> = families
            .iter()
            .map(|f| {
                let mut g: Vec<usize> = f
                    .members
                    .iter()
                    .map(|m| m.path.to_str().unwrap()[3..].parse().unwrap())
                    .collect();
                g.sort_unstable();
                g
            })
            .collect();
        out.sort();
        out
    }

    /// Planted neighbours at every distance up to `2k`, loners, exact
    /// copies and hashless images.
    fn corpus(seed: u64, k: u32) -> Vec<ScannedImage> {
        let mut rng = Lcg(seed);
        let mut images = Vec::new();
        let push = |hash: Option<u128>, exact: String, images: &mut Vec<ScannedImage>| {
            let i = images.len();
            images.push(image(i, hash, &exact));
        };
        for cluster in 0..60u32 {
            let base = rng.hash();
            for member in 0..4 {
                let mut h = base;
                let flips = rng.next() as u32 % (2 * k + 2);
                for _ in 0..flips {
                    h ^= 1u128 << (rng.next() % 128);
                }
                push(Some(h), format!("c{cluster}m{member}"), &mut images);
            }
        }
        for loner in 0..80 {
            push(Some(rng.hash()), format!("l{loner}"), &mut images);
        }
        // Exact copies sharing one pixel hash.
        let shared = rng.hash();
        for _ in 0..3 {
            push(Some(shared), "same".to_string(), &mut images);
        }
        // Flat images: no perceptual hash, exact only.
        for n in 0..3 {
            push(None, format!("flat{}", n % 2), &mut images);
        }
        images
    }

    #[test]
    fn multi_index_finds_exactly_the_pairs_brute_force_finds() {
        for k in [0u32, 1, 2, 5, 12, 20, 40] {
            let threshold = 1.0 - f64::from(k) / f64::from(IMAGE_HASH_BITS);
            assert_eq!(max_distance(threshold), k);
            for seed in 1..=4 {
                let images = corpus(seed * 7919 + u64::from(k), k);
                let got = partition(&cluster_images(&images, threshold));
                assert_eq!(got, brute_force(&images, k), "k={k} seed={seed}");
            }
        }
    }

    #[test]
    fn families_report_hamming_evidence() {
        let base = 0xF0F0_F0F0_F0F0_F0F0_0F0F_0F0F_0F0F_0F0Fu128;
        let images = vec![
            image(0, Some(base), "a"),
            image(1, Some(base ^ 0b111), "b"),
            image(2, Some(base), "c"),
        ];
        // 1 and 2 differ by 3 bits; 0 and 2 are the same perceptual hash but
        // different pixels.
        let families = cluster_images(&images, DEFAULT_IMAGE_THRESHOLD);
        assert_eq!(families.len(), 1);
        let family = &families[0];
        assert_eq!(family.members.len(), 3);
        assert!(family.edges.iter().all(|e| e.relation == Relation::Near));
        let best = &family.edges[0];
        assert_eq!(best.hamming, Some(0));
        assert_eq!(best.near_score, 1.0);
        assert!(best.jaccard.is_none() && best.containment.is_none());
        let far = family.edges.iter().find(|e| e.hamming == Some(3)).unwrap();
        assert!((far.near_score - (1.0 - 3.0 / 128.0)).abs() < 1e-12);
    }

    #[test]
    fn exact_pixels_beat_near_in_member_evidence() {
        let base = 0x1234_5678_9abc_def0_0fed_cba9_8765_4321u128;
        let images = vec![
            image(0, Some(base), "same"),
            image(1, Some(base), "same"),
            image(2, Some(base ^ 1), "other"),
        ];
        let families = cluster_images(&images, DEFAULT_IMAGE_THRESHOLD);
        assert_eq!(families.len(), 1);
        let by_path = |p: &str| {
            families[0]
                .members
                .iter()
                .find(|m| m.path == Path::new(p))
                .unwrap()
        };
        assert_eq!(by_path("img1").relation, Relation::Exact);
        assert_eq!(by_path("img2").relation, Relation::Near);
    }

    #[test]
    fn avif_over_the_byte_cap_is_refused_before_decoding() {
        let fixture =
            Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/avif/scene_q60.avif");
        // 192 x 128 x 3 = 73_728 bytes of pixels.
        assert!(decode_avif(&fixture, 73_728).is_ok());
        let err = decode_avif(&fixture, 73_727).unwrap_err().to_string();
        assert!(err.contains("too large"), "{err}");
    }

    #[test]
    fn nothing_to_cluster_yields_no_families() {
        assert!(cluster_images(&[], 0.9).is_empty());
        let one = vec![image(0, Some(1), "a")];
        assert!(cluster_images(&one, 0.9).is_empty());
    }

    #[test]
    fn grid_handles_sources_smaller_than_the_grid() {
        let mut grid = Grid::new(3, 2, DCT_SIDE, DCT_SIDE);
        grid.push_row(0, &[10, 20, 30]);
        grid.push_row(1, &[40, 50, 60]);
        let cells = grid.finish();
        assert_eq!(cells.len(), DCT_SIDE * DCT_SIDE);
        assert!(cells.iter().all(|v| (10.0..=60.0).contains(v)));
        // Top-left cell is the top-left source pixel; bottom-right likewise.
        assert_eq!(cells[0], 10.0);
        assert_eq!(cells[DCT_SIDE * DCT_SIDE - 1], 60.0);
    }

    #[test]
    fn grid_averages_blocks_of_a_larger_source() {
        let mut grid = Grid::new(64, 64, 32, 32);
        for y in 0..64 {
            let row: Vec<u16> = (0..64).map(|x| if x < 32 { 0 } else { 200 }).collect();
            grid.push_row(y, &row);
        }
        let cells = grid.finish();
        assert_eq!(cells[0], 0.0);
        assert_eq!(cells[31], 200.0);
    }
}
