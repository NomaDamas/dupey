//! Image fingerprinting against real encoded files: every case writes
//! actual PNG/JPEG/BMP bytes and decodes them back through the public API.

use std::io::Cursor;
use std::path::{Path, PathBuf};

use dupey_core::visual::{
    cluster_images, decode_cost, fingerprint, max_distance, PerceptualHash, ScannedImage,
    DEFAULT_IMAGE_THRESHOLD,
};
use dupey_core::{Format, Relation};
use image::codecs::jpeg::JpegEncoder;
use image::imageops::{self, FilterType};
use image::{DynamicImage, ImageFormat, Rgb, RgbImage, Rgba, RgbaImage};

fn tmp(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join("dupey-visual-tests");
    std::fs::create_dir_all(&dir).unwrap();
    dir.join(format!("{}-{name}", std::process::id()))
}

/// Deterministic pseudo-random bytes, so fixtures never differ between runs.
struct Lcg(u64);
impl Lcg {
    fn next(&mut self) -> u32 {
        self.0 = self
            .0
            .wrapping_mul(6364136223846793005)
            .wrapping_add(1442695040888963407);
        (self.0 >> 33) as u32
    }
}

/// A photo-like scene: smooth colour field plus seeded blobs, rectangles and
/// a little sensor noise. Different seeds give structurally unrelated images.
fn scene(seed: u64, w: u32, h: u32) -> RgbImage {
    let mut rng = Lcg(seed ^ 0x9E37_79B9_7F4A_7C15);
    let (fx, fy) = (1.0 + (rng.next() % 5) as f32, 1.0 + (rng.next() % 5) as f32);
    let phase = (rng.next() % 628) as f32 / 100.0;
    let base = [
        (rng.next() % 200) as f32,
        (rng.next() % 200) as f32,
        (rng.next() % 200) as f32,
    ];
    let mut img = RgbImage::from_fn(w, h, |x, y| {
        let u = x as f32 / w as f32;
        let v = y as f32 / h as f32;
        let wave = ((u * fx * std::f32::consts::TAU + phase).sin()
            * (v * fy * std::f32::consts::TAU).cos()
            + 1.0)
            * 28.0;
        Rgb([
            (base[0] + wave) as u8,
            (base[1] + wave * 0.8) as u8,
            (base[2] + wave * 1.2) as u8,
        ])
    });
    for _ in 0..7 {
        let (x0, y0) = (rng.next() % w, rng.next() % h);
        let (bw, bh) = (w / 6 + rng.next() % (w / 3), h / 6 + rng.next() % (h / 3));
        let colour = [
            (rng.next() % 256) as u8,
            (rng.next() % 256) as u8,
            (rng.next() % 256) as u8,
        ];
        for y in y0..(y0 + bh).min(h) {
            for x in x0..(x0 + bw).min(w) {
                img.put_pixel(x, y, Rgb(colour));
            }
        }
    }
    for p in img.pixels_mut() {
        let n = (rng.next() % 5) as i16 - 2;
        for c in &mut p.0 {
            *c = (*c as i16 + n).clamp(0, 255) as u8;
        }
    }
    img
}

fn save(img: &DynamicImage, name: &str, format: ImageFormat) -> PathBuf {
    let path = tmp(name);
    img.save_with_format(&path, format).unwrap();
    path
}

fn save_jpeg(img: &RgbImage, name: &str, quality: u8) -> PathBuf {
    let mut bytes = Vec::new();
    JpegEncoder::new_with_quality(Cursor::new(&mut bytes), quality)
        .encode_image(img)
        .unwrap();
    let path = tmp(name);
    std::fs::write(&path, bytes).unwrap();
    path
}

fn hash_of(path: &Path) -> PerceptualHash {
    fingerprint(path)
        .unwrap()
        .hash
        .expect("a detailed image has a perceptual hash")
}

#[test]
fn same_pixels_in_png_and_bmp_are_exact_equal() {
    let img = DynamicImage::ImageRgb8(scene(1, 160, 120));
    let png = fingerprint(&save(&img, "same.png", ImageFormat::Png)).unwrap();
    let bmp = fingerprint(&save(&img, "same.bmp", ImageFormat::Bmp)).unwrap();
    assert_eq!(png.exact_hash, bmp.exact_hash);
    assert_eq!((png.width, png.height), (160, 120));
    assert_eq!(png.hash, bmp.hash);
}

#[test]
fn one_changed_pixel_breaks_exact_but_stays_near() {
    let mut img = scene(2, 160, 120);
    let a = fingerprint(&save(
        &DynamicImage::ImageRgb8(img.clone()),
        "px-a.png",
        ImageFormat::Png,
    ))
    .unwrap();
    img.put_pixel(3, 3, Rgb([255, 0, 255]));
    let b = fingerprint(&save(
        &DynamicImage::ImageRgb8(img),
        "px-b.png",
        ImageFormat::Png,
    ))
    .unwrap();
    assert_ne!(a.exact_hash, b.exact_hash);
    assert!(a.hash.unwrap().distance(b.hash.unwrap()) <= 4);
}

#[test]
fn opaque_alpha_channel_does_not_change_the_exact_hash() {
    let rgb = scene(3, 96, 96);
    let rgba = RgbaImage::from_fn(96, 96, |x, y| {
        let p = rgb.get_pixel(x, y).0;
        Rgba([p[0], p[1], p[2], 255])
    });
    let a = fingerprint(&save(
        &DynamicImage::ImageRgb8(rgb),
        "alpha-a.png",
        ImageFormat::Png,
    ))
    .unwrap();
    let b = fingerprint(&save(
        &DynamicImage::ImageRgba8(rgba),
        "alpha-b.png",
        ImageFormat::Png,
    ))
    .unwrap();
    assert_eq!(a.exact_hash, b.exact_hash);
}

#[test]
fn downscaled_copy_is_a_near_duplicate() {
    let big = scene(4, 640, 480);
    let small = imageops::resize(&big, 200, 150, FilterType::Lanczos3);
    let a = hash_of(&save(
        &DynamicImage::ImageRgb8(big),
        "big.png",
        ImageFormat::Png,
    ));
    let b = hash_of(&save(
        &DynamicImage::ImageRgb8(small),
        "small.png",
        ImageFormat::Png,
    ));
    let sim = a.similarity(b);
    assert!(
        sim >= DEFAULT_IMAGE_THRESHOLD,
        "resize similarity {sim:.3} below {DEFAULT_IMAGE_THRESHOLD}"
    );
}

#[test]
fn heavy_jpeg_recompression_is_a_near_duplicate() {
    let img = scene(5, 480, 360);
    let original = hash_of(&save(
        &DynamicImage::ImageRgb8(img.clone()),
        "jpeg-src.png",
        ImageFormat::Png,
    ));
    for quality in [90u8, 60, 30] {
        let jpg = hash_of(&save_jpeg(&img, &format!("q{quality}.jpg"), quality));
        let sim = original.similarity(jpg);
        assert!(
            sim >= DEFAULT_IMAGE_THRESHOLD,
            "q{quality} similarity {sim:.3}"
        );
    }
}

#[test]
fn brightness_shift_is_a_near_duplicate() {
    let img = scene(6, 320, 240);
    let brighter = RgbImage::from_fn(320, 240, |x, y| {
        let p = img.get_pixel(x, y).0;
        Rgb(p.map(|c| c.saturating_add(18)))
    });
    let a = hash_of(&save(
        &DynamicImage::ImageRgb8(img),
        "br-a.png",
        ImageFormat::Png,
    ));
    let b = hash_of(&save(
        &DynamicImage::ImageRgb8(brighter),
        "br-b.png",
        ImageFormat::Png,
    ));
    assert!(a.similarity(b) >= DEFAULT_IMAGE_THRESHOLD);
}

#[test]
fn unrelated_scenes_are_far_apart() {
    let hashes: Vec<PerceptualHash> = (100..112)
        .map(|seed| {
            hash_of(&save(
                &DynamicImage::ImageRgb8(scene(seed, 256, 192)),
                &format!("rand-{seed}.png"),
                ImageFormat::Png,
            ))
        })
        .collect();
    for (i, a) in hashes.iter().enumerate() {
        for b in &hashes[i + 1..] {
            assert!(
                a.similarity(*b) < DEFAULT_IMAGE_THRESHOLD,
                "unrelated scenes scored {:.3}",
                a.similarity(*b)
            );
        }
    }
}

/// JPEG with an EXIF Orientation tag spliced in after SOI.
fn with_orientation(jpeg: &[u8], orientation: u8) -> Vec<u8> {
    let mut exif = b"Exif\0\0MM\0\x2a\0\0\0\x08".to_vec();
    exif.extend_from_slice(&[0, 1]); // one IFD entry
    exif.extend_from_slice(&[0x01, 0x12, 0, 3, 0, 0, 0, 1, 0, orientation, 0, 0]);
    exif.extend_from_slice(&[0, 0, 0, 0]); // no next IFD
    let mut out = vec![0xFF, 0xD8, 0xFF, 0xE1];
    out.extend_from_slice(&((exif.len() + 2) as u16).to_be_bytes());
    out.extend_from_slice(&exif);
    out.extend_from_slice(&jpeg[2..]);
    out
}

#[test]
fn exif_orientation_is_applied_before_hashing() {
    // Asymmetric on purpose: a quarter turn changes the hash a lot.
    let upright = scene(7, 320, 200);
    let stored_sideways = imageops::rotate270(&upright); // 200 x 320 on disk
    let mut sideways_jpeg = Vec::new();
    JpegEncoder::new_with_quality(Cursor::new(&mut sideways_jpeg), 92)
        .encode_image(&stored_sideways)
        .unwrap();
    // Orientation 6: display by rotating 90 degrees clockwise.
    let tagged = tmp("tagged.jpg");
    std::fs::write(&tagged, with_orientation(&sideways_jpeg, 6)).unwrap();
    let plain = tmp("plain.jpg");
    std::fs::write(&plain, &sideways_jpeg).unwrap();
    let reference = save_jpeg(&upright, "upright.jpg", 92);

    let tagged_fp = fingerprint(&tagged).unwrap();
    assert_eq!(
        (tagged_fp.width, tagged_fp.height),
        (320, 200),
        "oriented dims"
    );
    let sim_tagged = tagged_fp.hash.unwrap().similarity(hash_of(&reference));
    let sim_plain = hash_of(&plain).similarity(hash_of(&reference));
    assert!(
        sim_tagged >= DEFAULT_IMAGE_THRESHOLD,
        "tagged {sim_tagged:.3}"
    );
    assert!(sim_plain < DEFAULT_IMAGE_THRESHOLD, "plain {sim_plain:.3}");
}

#[test]
fn flat_images_have_no_perceptual_hash_but_still_match_exactly() {
    let black = DynamicImage::ImageRgb8(RgbImage::from_pixel(64, 64, Rgb([0, 0, 0])));
    let white = DynamicImage::ImageRgb8(RgbImage::from_pixel(64, 64, Rgb([255, 255, 255])));
    let b1 = fingerprint(&save(&black, "flat-b1.png", ImageFormat::Png)).unwrap();
    let b2 = fingerprint(&save(&black, "flat-b2.bmp", ImageFormat::Bmp)).unwrap();
    let w = fingerprint(&save(&white, "flat-w.png", ImageFormat::Png)).unwrap();
    assert!(b1.hash.is_none() && w.hash.is_none());
    assert_eq!(b1.exact_hash, b2.exact_hash);
    assert_ne!(b1.exact_hash, w.exact_hash);

    let images: Vec<ScannedImage> = [("b1", &b1), ("b2", &b2), ("w", &w)]
        .iter()
        .map(|(n, f)| ScannedImage::new(PathBuf::from(n), f))
        .collect();
    let families = cluster_images(&images, DEFAULT_IMAGE_THRESHOLD);
    assert_eq!(families.len(), 1, "black/white must not merge");
    assert_eq!(families[0].members.len(), 2);
    assert_eq!(families[0].edges[0].relation, Relation::Exact);
}

#[test]
fn transparent_pixels_composite_over_white() {
    // Same visible picture; the hidden RGB under alpha = 0 differs.
    let mut a = RgbaImage::from_pixel(128, 128, Rgba([255, 0, 0, 0]));
    let mut b = RgbaImage::from_pixel(128, 128, Rgba([0, 0, 255, 0]));
    for y in 20..80 {
        for x in 30..100 {
            a.put_pixel(x, y, Rgba([10, 10, 10, 255]));
            b.put_pixel(x, y, Rgba([10, 10, 10, 255]));
        }
    }
    let fa = fingerprint(&save(
        &DynamicImage::ImageRgba8(a),
        "tr-a.png",
        ImageFormat::Png,
    ))
    .unwrap();
    let fb = fingerprint(&save(
        &DynamicImage::ImageRgba8(b),
        "tr-b.png",
        ImageFormat::Png,
    ))
    .unwrap();
    assert_eq!(fa.hash, fb.hash, "invisible RGB must not move the hash");
}

#[test]
fn tiny_images_are_hashed_without_panicking() {
    for (w, h) in [(1, 1), (3, 2), (9, 40), (40, 9), (31, 33)] {
        let img = scene(8, w.max(8), h.max(8));
        let small = imageops::crop_imm(&img, 0, 0, w, h).to_image();
        let fp = fingerprint(&save(
            &DynamicImage::ImageRgb8(small),
            &format!("tiny-{w}x{h}.png"),
            ImageFormat::Png,
        ))
        .unwrap();
        assert_eq!((fp.width, fp.height), (w, h));
    }
}

#[test]
fn corrupt_and_non_image_files_are_errors_not_panics() {
    let img = DynamicImage::ImageRgb8(scene(9, 128, 96));
    let good = save(&img, "trunc-src.png", ImageFormat::Png);
    let bytes = std::fs::read(&good).unwrap();
    let truncated = tmp("truncated.png");
    std::fs::write(&truncated, &bytes[..bytes.len() / 2]).unwrap();
    assert!(fingerprint(&truncated).is_err());
    let garbage = tmp("garbage.jpg");
    std::fs::write(&garbage, b"definitely not a jpeg").unwrap();
    assert!(fingerprint(&garbage).is_err());
    assert!(fingerprint(&tmp("missing.png")).is_err());
}

#[test]
fn content_decides_the_codec_not_the_extension() {
    let img = DynamicImage::ImageRgb8(scene(10, 96, 96));
    let real = fingerprint(&save(&img, "named-right.png", ImageFormat::Png)).unwrap();
    let wrong = fingerprint(&save(&img, "named-wrong.jpg", ImageFormat::Png)).unwrap();
    assert_eq!(real.exact_hash, wrong.exact_hash);
}

#[test]
fn decode_cost_reads_only_the_header() {
    let img = DynamicImage::ImageRgb8(scene(11, 300, 200));
    let path = save(&img, "cost.png", ImageFormat::Png);
    let cost = decode_cost(&path);
    assert!(
        cost >= 300 * 200 * 4,
        "cost {cost} must cover decoded pixels"
    );
    assert!(cost <= 300 * 200 * 16, "cost {cost} is wildly pessimistic");
    assert_eq!(decode_cost(&tmp("missing-cost.png")), 0);
}

#[test]
fn image_extensions_map_to_image_formats() {
    for (name, format) in [
        ("a.png", Format::Png),
        ("a.JPG", Format::Jpeg),
        ("a.jpeg", Format::Jpeg),
        ("a.gif", Format::Gif),
        ("a.bmp", Format::Bmp),
        ("a.webp", Format::Webp),
        ("a.tif", Format::Tiff),
        ("a.TIFF", Format::Tiff),
    ] {
        assert_eq!(Format::from_path(Path::new(name)), Some(format), "{name}");
        assert!(format.is_image());
    }
    assert!(!Format::Docx.is_image());
    assert!(dupey_core::extract(Path::new("a.png")).is_err());
}

#[test]
fn threshold_maps_to_a_hamming_budget() {
    assert_eq!(max_distance(1.0), 0);
    assert_eq!(max_distance(0.90), 12);
    assert_eq!(max_distance(0.5), 64);
    assert_eq!(max_distance(0.0), 127, "never degenerate to all-pairs");
}

#[test]
fn perceptual_hash_hex_round_trips() {
    let h = PerceptualHash(0x0123_4567_89ab_cdef_fedc_ba98_7654_3210);
    assert_eq!(h.to_hex(), "0123456789abcdeffedcba9876543210");
    assert_eq!(PerceptualHash::from_hex(&h.to_hex()), Some(h));
    assert_eq!(PerceptualHash::from_hex("xyz"), None);
}

fn crc32(data: &[u8]) -> u32 {
    let mut crc = 0xFFFF_FFFFu32;
    for &b in data {
        crc ^= u32::from(b);
        for _ in 0..8 {
            crc = if crc & 1 == 1 {
                (crc >> 1) ^ 0xEDB8_8320
            } else {
                crc >> 1
            };
        }
    }
    !crc
}

#[test]
fn decompression_bomb_headers_are_refused_before_decoding() {
    // A real 1x1 PNG whose IHDR claims 60000 x 60000 (10.8 GB as RGB8):
    // a few dozen bytes on disk, enormous if a decoder believed it.
    let real = save(
        &DynamicImage::ImageRgb8(scene(12, 8, 8)),
        "bomb-src.png",
        ImageFormat::Png,
    );
    let mut bytes = std::fs::read(real).unwrap();
    let ihdr = 8 + 8; // signature + chunk length/type
    bytes[ihdr..ihdr + 4].copy_from_slice(&60_000u32.to_be_bytes());
    bytes[ihdr + 4..ihdr + 8].copy_from_slice(&60_000u32.to_be_bytes());
    let crc = crc32(&bytes[ihdr - 4..ihdr + 13]);
    bytes[ihdr + 13..ihdr + 17].copy_from_slice(&crc.to_be_bytes());
    let path = tmp("bomb.png");
    std::fs::write(&path, bytes).unwrap();
    let err = fingerprint(&path).unwrap_err().to_string();
    assert!(err.contains("too large"), "{err}");
    assert!(decode_cost(&path) > 1 << 30, "header cost still readable");
}

// ---- AVIF -----------------------------------------------------------------
//
// Fixtures in tests/fixtures/avif were encoded by libheif's `heif-enc` (an
// encoder independent of the decoder under test) from `scene.png`, a 192x128
// asymmetric picture. `orientN.avif` hold the scene pre-rotated so that the
// EXIF orientation N written by libheif as irot/imir displays it upright.

fn fixture(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/avif")
        .join(name)
}

#[test]
fn avif_extension_is_an_image_format_and_heic_is_not() {
    assert_eq!(Format::from_path(Path::new("a.avif")), Some(Format::Avif));
    assert!(Format::Avif.is_image());
    // HEVC carries patents: HEIC/HEIF stay unsupported.
    assert_eq!(Format::from_path(Path::new("a.heic")), None);
    assert_eq!(Format::from_path(Path::new("a.heif")), None);
}

#[test]
fn lossless_identity_matrix_avif_is_refused_instead_of_misdecoded() {
    // The decoder reads identity-matrix (GBR) planes as BT.601: colours come
    // out wrong, so the file is reported as an error.
    let err = fingerprint(&fixture("scene_lossless.avif"))
        .unwrap_err()
        .to_string();
    assert!(err.contains("identity"), "{err}");
}

#[test]
fn lossy_avif_is_a_near_duplicate_of_its_source() {
    let source = hash_of(&fixture("scene.png"));
    let lossy = hash_of(&fixture("scene_q60.avif"));
    assert!(source.similarity(lossy) >= DEFAULT_IMAGE_THRESHOLD);
    let unrelated = hash_of(&save(
        &DynamicImage::ImageRgb8(scene(21, 192, 128)),
        "avif-unrelated.png",
        ImageFormat::Png,
    ));
    assert!(unrelated.similarity(lossy) < DEFAULT_IMAGE_THRESHOLD);
}

#[test]
fn avif_irot_and_imir_display_the_upright_scene_for_all_eight_orientations() {
    let upright = hash_of(&fixture("scene.png"));
    for k in 1..=8 {
        let fp = fingerprint(&fixture(&format!("orient{k}.avif"))).unwrap();
        assert_eq!((fp.width, fp.height), (192, 128), "orientation {k} dims");
        let sim = upright.similarity(fp.hash.unwrap());
        assert!(sim >= DEFAULT_IMAGE_THRESHOLD, "orientation {k}: {sim:.3}");
    }
}

#[test]
fn avif_alpha_is_composited_over_white() {
    let source = hash_of(&fixture("alpha.png"));
    let avif = hash_of(&fixture("alpha.avif"));
    assert!(source.similarity(avif) >= DEFAULT_IMAGE_THRESHOLD);
}

#[test]
fn monochrome_avif_matches_its_gray_source() {
    let source = hash_of(&fixture("gray.png"));
    let avif = hash_of(&fixture("gray.avif"));
    assert!(source.similarity(avif) >= DEFAULT_IMAGE_THRESHOLD);
}

#[test]
fn avif_is_recognised_by_content_not_extension() {
    let bytes = std::fs::read(fixture("scene_q60.avif")).unwrap();
    let renamed = tmp("really-avif.png");
    std::fs::write(&renamed, bytes).unwrap();
    assert_eq!(
        fingerprint(&renamed).unwrap().exact_hash,
        fingerprint(&fixture("scene_q60.avif")).unwrap().exact_hash
    );
}

#[test]
fn damaged_avif_is_an_error_not_a_panic() {
    let bytes = std::fs::read(fixture("scene_q60.avif")).unwrap();
    let truncated = tmp("truncated.avif");
    std::fs::write(&truncated, &bytes[..bytes.len() / 2]).unwrap();
    assert!(fingerprint(&truncated).is_err());
    let mut flipped = bytes.clone();
    for b in flipped.iter_mut().skip(bytes.len() / 2).take(64) {
        *b ^= 0xFF;
    }
    let corrupt = tmp("corrupt.avif");
    std::fs::write(&corrupt, flipped).unwrap();
    // Damaged AV1 data may decode to garbage or fail; it must not panic.
    let _ = fingerprint(&corrupt);
    let garbage = tmp("garbage.avif");
    std::fs::write(&garbage, b"not an avif at all").unwrap();
    assert!(fingerprint(&garbage).is_err());
}

#[test]
fn hevc_heif_is_not_decoded_even_when_named_avif() {
    // Same container, brand rewritten to `heic`: this build has no HEVC
    // decoder and must say so instead of guessing.
    let mut bytes = std::fs::read(fixture("scene_q60.avif")).unwrap();
    let ftyp = bytes.windows(4).position(|w| w == b"ftyp").unwrap();
    bytes[ftyp + 4..ftyp + 8].copy_from_slice(b"heic");
    for brand in bytes[ftyp + 8..ftyp + 40].chunks_exact_mut(4) {
        if brand == b"avif" {
            brand.copy_from_slice(b"mif1");
        }
    }
    let path = tmp("hevc.avif");
    std::fs::write(&path, bytes).unwrap();
    assert!(fingerprint(&path).is_err());
}

#[test]
fn avif_decode_cost_comes_from_the_header() {
    let cost = decode_cost(&fixture("scene_q60.avif"));
    assert!(cost >= 192 * 128 * 4, "cost {cost}");
    assert!(cost <= 192 * 128 * 32, "cost {cost}");
    assert_eq!(decode_cost(&tmp("missing.avif")), 0);
}

#[test]
fn dim_sixteen_bit_images_keep_their_structure() {
    // A 16-bit image that uses only the bottom 1.5% of the range (a dim
    // HDR/10-bit decode): squeezed into 8 bits it would have 4 levels left.
    let base = scene(30, 256, 192);
    let dim = image::ImageBuffer::<Rgb<u16>, Vec<u16>>::from_fn(256, 192, |x, y| {
        Rgb(base.get_pixel(x, y).0.map(|c| u16::from(c) * 4))
    });
    let path = tmp("dim16.png");
    DynamicImage::ImageRgb16(dim).save(&path).unwrap();
    let reference = hash_of(&save(
        &DynamicImage::ImageRgb8(base),
        "dim16-ref.png",
        ImageFormat::Png,
    ));
    let sim = reference.similarity(hash_of(&path));
    assert!(
        sim >= DEFAULT_IMAGE_THRESHOLD,
        "dim 16-bit similarity {sim:.3}"
    );
}

#[test]
fn svt_av1_encoded_avif_of_simple_content_matches_its_source() {
    // Only simple content: the decoder is known to mis-decode some photos
    // from SVT-AV1 streams (see the README), so this does not claim more.
    for name in ["scene_10bit.avif", "scene_svt8.avif"] {
        let sim = hash_of(&fixture("scene.png")).similarity(hash_of(&fixture(name)));
        assert!(sim >= DEFAULT_IMAGE_THRESHOLD, "{name}: {sim:.3}");
    }
}

#[test]
fn avif_that_crashes_the_decoder_is_an_error_not_a_panic() {
    // A real 9 KB ImageMagick/libheif-written screenshot. On aarch64 the
    // decoder indexes out of bounds in its NEON loop-restoration code; one
    // such file must not take down a whole scan. Other CPUs run different
    // SIMD code and decode it fine, so only the call returning is required
    // there.
    let result = fingerprint(&fixture("decoder_panic.avif"));
    if cfg!(target_arch = "aarch64") {
        let err = result.expect_err("the aarch64 decoder is known to crash on this file");
        assert!(err.to_string().contains("decoder crashed"), "{err}");
    } else if let Ok(fp) = result {
        assert_eq!((fp.width, fp.height), (800, 489));
    }
}

#[test]
fn avif_decoding_does_not_depend_on_the_callers_stack_size() {
    // Windows gives the main thread 1 MiB (macOS/Linux: 8 MiB) and the AV1
    // decoder's frames are large, so `fingerprint`/`compare` called from
    // `main` overflowed there. Decoding must bring its own stack.
    let path = fixture("scene_q60.avif");
    let result = std::thread::Builder::new()
        .stack_size(256 * 1024)
        .spawn(move || fingerprint(&path).map(|f| (f.width, f.height)))
        .unwrap()
        .join()
        .expect("decoding overflowed a 256 KiB caller stack");
    assert_eq!(result.unwrap(), (192, 128));
}
