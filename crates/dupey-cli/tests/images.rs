//! `dupey` binary against real image files: scan, fingerprint and compare.

use std::io::Cursor;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{Duration, UNIX_EPOCH};

use image::codecs::jpeg::JpegEncoder;
use image::imageops::{self, FilterType};
use image::{DynamicImage, ImageFormat, Rgb, RgbImage};

fn dupey() -> Command {
    Command::new(env!("CARGO_BIN_EXE_dupey"))
}

fn fresh_dir(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("dupey-cli-images-{name}"));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

/// Seeded structured picture; different seeds are unrelated scenes.
fn scene(seed: u64, w: u32, h: u32) -> RgbImage {
    let mut state = seed.wrapping_mul(0x9E37_79B9_7F4A_7C15) | 1;
    let mut next = move || {
        state = state
            .wrapping_mul(6364136223846793005)
            .wrapping_add(1442695040888963407);
        (state >> 33) as u32
    };
    let (fx, fy) = (1.0 + (next() % 5) as f32, 1.0 + (next() % 5) as f32);
    let base = [
        (next() % 200) as f32,
        (next() % 200) as f32,
        (next() % 200) as f32,
    ];
    let mut img = RgbImage::from_fn(w, h, |x, y| {
        let (u, v) = (x as f32 / w as f32, y as f32 / h as f32);
        let wave =
            ((u * fx * std::f32::consts::TAU).sin() * (v * fy * std::f32::consts::TAU).cos() + 1.0)
                * 28.0;
        Rgb([
            (base[0] + wave) as u8,
            (base[1] + wave * 0.8) as u8,
            (base[2] + wave * 1.2) as u8,
        ])
    });
    for _ in 0..7 {
        let (x0, y0) = (next() % w, next() % h);
        let (bw, bh) = (w / 6 + next() % (w / 3), h / 6 + next() % (h / 3));
        let colour = Rgb([
            (next() % 256) as u8,
            (next() % 256) as u8,
            (next() % 256) as u8,
        ]);
        for y in y0..(y0 + bh).min(h) {
            for x in x0..(x0 + bw).min(w) {
                img.put_pixel(x, y, colour);
            }
        }
    }
    img
}

fn save(dir: &Path, name: &str, img: &RgbImage, format: ImageFormat) -> PathBuf {
    let path = dir.join(name);
    DynamicImage::ImageRgb8(img.clone())
        .save_with_format(&path, format)
        .unwrap();
    path
}

fn save_jpeg(dir: &Path, name: &str, img: &RgbImage, quality: u8) -> PathBuf {
    let mut bytes = Vec::new();
    JpegEncoder::new_with_quality(Cursor::new(&mut bytes), quality)
        .encode_image(img)
        .unwrap();
    let path = dir.join(name);
    std::fs::write(&path, bytes).unwrap();
    path
}

fn set_mtime(path: &Path, unix_secs: u64) {
    std::fs::OpenOptions::new()
        .write(true)
        .open(path)
        .unwrap()
        .set_modified(UNIX_EPOCH + Duration::from_secs(unix_secs))
        .unwrap();
}

fn scan_json(dir: &Path, extra: &[&str]) -> serde_json::Value {
    let out = dupey()
        .arg("scan")
        .arg(dir)
        .arg("--json")
        .args(extra)
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "scan failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    serde_json::from_slice(&out.stdout).unwrap()
}

fn names(family: &serde_json::Value) -> Vec<String> {
    let mut v: Vec<String> = family["files"]
        .as_array()
        .unwrap()
        .iter()
        .map(|f| {
            Path::new(f.as_str().unwrap())
                .file_name()
                .unwrap()
                .to_string_lossy()
                .into_owned()
        })
        .collect();
    v.sort();
    v
}

fn file<'a>(v: &'a serde_json::Value, name: &str) -> &'a serde_json::Value {
    v["files"]
        .as_array()
        .unwrap()
        .iter()
        .find(|f| Path::new(f["path"].as_str().unwrap()).file_name().unwrap() == name)
        .unwrap_or_else(|| panic!("{name} not in scan output"))
}

/// A photo, its lossless re-save, a downscaled JPEG, and strangers.
fn photo_dir(name: &str) -> PathBuf {
    let dir = fresh_dir(name);
    let photo = scene(1, 480, 360);
    save(&dir, "photo.png", &photo, ImageFormat::Png);
    save(&dir, "photo_copy.bmp", &photo, ImageFormat::Bmp);
    let small = imageops::resize(&photo, 240, 180, FilterType::Lanczos3);
    save_jpeg(&dir, "photo_small.jpg", &small, 70);
    save(&dir, "other.png", &scene(2, 480, 360), ImageFormat::Png);
    save(&dir, "stranger.webp", &scene(3, 300, 300), ImageFormat::Png);
    dir
}

#[test]
fn scan_groups_exact_and_near_images_and_leaves_strangers_alone() {
    let dir = photo_dir("groups");
    let v = scan_json(&dir, &[]);
    let families = v["families"].as_array().unwrap();
    assert_eq!(families.len(), 1, "{families:#?}");
    let family = &families[0];
    assert_eq!(
        names(family),
        ["photo.png", "photo_copy.bmp", "photo_small.jpg"]
    );
    assert_eq!(family["relation"], "mixed", "exact copy + near resize");

    let member = |name: &str| {
        family["members"]
            .as_array()
            .unwrap()
            .iter()
            .find(|m| Path::new(m["path"].as_str().unwrap()).file_name().unwrap() == name)
            .unwrap()
    };
    assert_eq!(member("photo_small.jpg")["relation"], "near");
    assert!(member("photo_small.jpg")["near_score"].as_f64().unwrap() >= 0.9);
    assert!(member("photo_small.jpg")["hamming"].as_u64().unwrap() <= 12);
    assert!(member("photo_small.jpg")["jaccard"].is_null());
    let exact_member = family["members"]
        .as_array()
        .unwrap()
        .iter()
        .find(|m| m["relation"] == "exact")
        .expect("one member joined by exact pixels");
    assert_eq!(exact_member["hamming"], 0);

    let edges = family["edges"].as_array().unwrap();
    assert!(edges.iter().any(|e| e["relation"] == "exact"));
    assert!(edges
        .iter()
        .any(|e| e["relation"] == "near" && e["hamming"].is_u64()));
    assert_eq!(v["image_threshold"], 0.9);
}

#[test]
fn scan_reports_image_file_entries() {
    let dir = photo_dir("entries");
    let v = scan_json(&dir, &[]);
    let png = file(&v, "photo.png");
    let bmp = file(&v, "photo_copy.bmp");
    assert_eq!(png["format"], "png");
    assert_eq!(bmp["format"], "bmp");
    assert_eq!(file(&v, "photo_small.jpg")["format"], "jpeg");
    assert_eq!(png["content_hash"], bmp["content_hash"], "same pixels");
    assert_eq!(png["image_hash"], bmp["image_hash"]);
    let hash = png["image_hash"].as_str().unwrap();
    assert_eq!(hash.len(), 32);
    assert!(hash.chars().all(|c| c.is_ascii_hexdigit()));
    assert!(png["fuzzy"].is_null(), "MinHash is for text only");
    assert_eq!(png["signals"]["width"], 480);
    assert_eq!(png["signals"]["height"], 360);
    assert!(png["signals"]["chars"].is_null());
    assert_eq!(file(&v, "photo_small.jpg")["signals"]["width"], 240);
}

#[test]
fn image_threshold_flag_controls_near_matches() {
    let dir = photo_dir("threshold");
    let strict = scan_json(&dir, &["--image-threshold", "1.0"]);
    let families = strict["families"].as_array().unwrap();
    assert_eq!(families.len(), 1);
    assert_eq!(names(&families[0]), ["photo.png", "photo_copy.bmp"]);
    assert_eq!(families[0]["relation"], "exact");
    assert_eq!(strict["image_threshold"], 1.0);

    let loose = scan_json(&dir, &["--image-threshold", "0.5"]);
    assert!(loose["families"][0]["files"].as_array().unwrap().len() >= 3);
}

#[test]
fn flat_images_only_match_when_pixel_identical() {
    let dir = fresh_dir("flat");
    let black = RgbImage::from_pixel(80, 80, Rgb([0, 0, 0]));
    let white = RgbImage::from_pixel(80, 80, Rgb([255, 255, 255]));
    save(&dir, "black1.png", &black, ImageFormat::Png);
    save(&dir, "black2.bmp", &black, ImageFormat::Bmp);
    save(&dir, "white.png", &white, ImageFormat::Png);
    let v = scan_json(&dir, &[]);
    let families = v["families"].as_array().unwrap();
    assert_eq!(families.len(), 1);
    assert_eq!(names(&families[0]), ["black1.png", "black2.bmp"]);
    assert_eq!(families[0]["relation"], "exact");
    assert!(file(&v, "white.png")["image_hash"].is_null());
}

#[test]
fn images_and_documents_form_separate_families_with_unique_ids() {
    let dir = photo_dir("mixed-corpus");
    std::fs::write(dir.join("a.txt"), "quarterly plan\n".repeat(40)).unwrap();
    std::fs::write(dir.join("b.txt"), "quarterly plan\n".repeat(40)).unwrap();
    let v = scan_json(&dir, &[]);
    let families = v["families"].as_array().unwrap();
    assert_eq!(families.len(), 2);
    let mut ids: Vec<u64> = families.iter().map(|f| f["id"].as_u64().unwrap()).collect();
    ids.sort_unstable();
    ids.dedup();
    assert_eq!(ids.len(), 2, "ids must be unique across kinds");
    let text = families
        .iter()
        .find(|f| names(f).contains(&"a.txt".to_string()))
        .unwrap();
    assert_eq!(names(text), ["a.txt", "b.txt"]);
    assert!(text["members"][0]["hamming"].is_null());
    assert!(
        text["edges"][0]["jaccard"].is_f64(),
        "text evidence unchanged"
    );
}

#[test]
fn newest_image_is_the_pick_by_filesystem_mtime() {
    let dir = photo_dir("pick");
    set_mtime(&dir.join("photo.png"), 1_700_000_000);
    set_mtime(&dir.join("photo_copy.bmp"), 1_700_500_000);
    set_mtime(&dir.join("photo_small.jpg"), 1_700_100_000);
    let v = scan_json(&dir, &[]);
    let pick = &v["families"][0]["pick"];
    assert!(pick["ranked"][0]["path"]
        .as_str()
        .unwrap()
        .ends_with("photo_copy.bmp"));
    assert_eq!(pick["reasons"][0]["name"], "fs_mtime");
}

#[test]
fn corrupt_image_is_reported_and_the_scan_continues() {
    let dir = photo_dir("corrupt");
    let bytes = std::fs::read(dir.join("other.png")).unwrap();
    std::fs::write(dir.join("broken.png"), &bytes[..bytes.len() / 3]).unwrap();
    std::fs::write(dir.join("fake.jpg"), b"not an image").unwrap();
    let v = scan_json(&dir, &[]);
    let errors: Vec<String> = v["errors"]
        .as_array()
        .unwrap()
        .iter()
        .map(|e| {
            Path::new(e["path"].as_str().unwrap())
                .file_name()
                .unwrap()
                .to_string_lossy()
                .into_owned()
        })
        .collect();
    assert!(errors.contains(&"broken.png".to_string()), "{errors:?}");
    assert!(errors.contains(&"fake.jpg".to_string()), "{errors:?}");
    assert_eq!(v["families"].as_array().unwrap().len(), 1);
}

#[test]
fn human_summary_shows_hamming_evidence() {
    let dir = photo_dir("human");
    let out = dupey().arg("scan").arg(&dir).output().unwrap();
    assert!(out.status.success());
    let text = String::from_utf8_lossy(&out.stdout);
    assert!(text.contains("hamming"), "{text}");
    assert!(text.contains("photo_small.jpg"));
}

#[test]
fn fingerprint_prints_image_identity() {
    let dir = photo_dir("fingerprint");
    let out = dupey()
        .arg("fingerprint")
        .arg(dir.join("photo.png"))
        .output()
        .unwrap();
    assert!(out.status.success());
    let text = String::from_utf8_lossy(&out.stdout);
    assert!(text.contains("format\tPng"), "{text}");
    assert!(text.contains("width\t480"));
    assert!(text.contains("height\t360"));
    let scan = scan_json(&dir, &[]);
    let hash = file(&scan, "photo.png")["image_hash"].as_str().unwrap();
    assert!(text.contains(&format!("image_hash\t{hash}")));
    let exact = file(&scan, "photo.png")["content_hash"].as_str().unwrap();
    assert!(text.contains(&format!("exact\t{exact}")));
}

#[test]
fn compare_reports_exact_hamming_and_similarity() {
    let dir = photo_dir("compare");
    let run = |a: &str, b: &str| {
        let out = dupey()
            .arg("compare")
            .arg(dir.join(a))
            .arg(dir.join(b))
            .output()
            .unwrap();
        assert!(
            out.status.success(),
            "{}",
            String::from_utf8_lossy(&out.stderr)
        );
        String::from_utf8_lossy(&out.stdout).into_owned()
    };
    let same = run("photo.png", "photo_copy.bmp");
    assert!(same.contains("exact_equal\ttrue"), "{same}");
    assert!(same.contains("hamming\t0"));
    assert!(same.contains("similarity\t1.0000"));
    let near = run("photo.png", "photo_small.jpg");
    assert!(near.contains("exact_equal\tfalse"));
    assert!(near.contains("similarity\t0.9"), "{near}");
    let far = run("photo.png", "other.png");
    assert!(far.contains("exact_equal\tfalse"));
    let similarity: f64 = far
        .lines()
        .find_map(|l| l.strip_prefix("similarity\t"))
        .unwrap()
        .parse()
        .unwrap();
    assert!(similarity < 0.9, "{far}");
}

#[test]
fn compare_refuses_to_mix_an_image_with_a_document() {
    let dir = photo_dir("compare-mixed");
    std::fs::write(dir.join("a.txt"), "hello\n").unwrap();
    let out = dupey()
        .arg("compare")
        .arg(dir.join("photo.png"))
        .arg(dir.join("a.txt"))
        .output()
        .unwrap();
    assert!(!out.status.success());
    assert!(String::from_utf8_lossy(&out.stderr).contains("image"));
}

#[test]
fn many_images_scan_into_the_expected_families() {
    // 40 scenes, each saved twice (png + bmp): 40 exact families, none merged.
    let dir = fresh_dir("many");
    for seed in 0..40u64 {
        let img = scene(1000 + seed, 128, 96);
        save(&dir, &format!("s{seed}.png"), &img, ImageFormat::Png);
        save(&dir, &format!("s{seed}_dup.bmp"), &img, ImageFormat::Bmp);
    }
    let v = scan_json(&dir, &[]);
    let families = v["families"].as_array().unwrap();
    assert_eq!(families.len(), 40);
    assert!(families
        .iter()
        .all(|f| f["files"].as_array().unwrap().len() == 2));
    assert!(families.iter().all(|f| f["relation"] == "exact"));
}

fn avif_fixture(name: &str) -> PathBuf {
    // `DUPEY_AVIF_FIXTURES` lets a test binary built on one machine run on
    // another (the cross-compiled Windows run), where the baked-in manifest
    // path does not exist.
    let base = std::env::var_os("DUPEY_AVIF_FIXTURES")
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            Path::new(env!("CARGO_MANIFEST_DIR")).join("../dupey-core/tests/fixtures/avif")
        });
    base.join(name)
}

fn avif_dir(name: &str) -> PathBuf {
    let dir = fresh_dir(name);
    for (from, to) in [
        ("scene.png", "scene.png"),
        ("scene_q60.avif", "scene_q60.avif"),
        ("orient6.avif", "phone_photo.avif"),
        ("orient5.avif", "phone_photo_transposed.avif"),
    ] {
        std::fs::copy(avif_fixture(from), dir.join(to)).unwrap();
    }
    save(&dir, "other.png", &scene(9, 192, 128), ImageFormat::Png);
    dir
}

#[test]
fn scan_groups_avif_with_its_png_source_including_rotated_avifs() {
    let dir = avif_dir("avif-scan");
    let v = scan_json(&dir, &[]);
    let families = v["families"].as_array().unwrap();
    assert_eq!(families.len(), 1, "{families:#?}");
    assert_eq!(
        names(&families[0]),
        [
            "phone_photo.avif",
            "phone_photo_transposed.avif",
            "scene.png",
            "scene_q60.avif"
        ]
    );
    let avif = file(&v, "scene_q60.avif");
    assert_eq!(avif["format"], "avif");
    assert_eq!(avif["image_hash"].as_str().unwrap().len(), 32);
    assert_eq!(avif["signals"]["width"], 192);
    assert_eq!(file(&v, "phone_photo.avif")["signals"]["height"], 128);
}

#[test]
fn compare_and_fingerprint_work_on_avif() {
    let dir = avif_dir("avif-compare");
    let out = dupey()
        .arg("compare")
        .arg(dir.join("scene.png"))
        .arg(dir.join("scene_q60.avif"))
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let text = String::from_utf8_lossy(&out.stdout);
    assert!(text.contains("exact_equal\tfalse"), "{text}");
    let sim: f64 = text
        .lines()
        .find_map(|l| l.strip_prefix("similarity\t"))
        .unwrap()
        .parse()
        .unwrap();
    assert!(sim >= 0.9, "{text}");
    let out = dupey()
        .arg("fingerprint")
        .arg(dir.join("scene_q60.avif"))
        .output()
        .unwrap();
    let text = String::from_utf8_lossy(&out.stdout);
    assert!(
        text.contains("format\tAvif") && text.contains("width\t192"),
        "{text}"
    );
}

#[test]
fn unsupported_avif_and_heic_are_handled_without_stopping_the_scan() {
    let dir = avif_dir("avif-errors");
    // Lossless identity-matrix AVIF: refused, reported.
    std::fs::copy(
        avif_fixture("scene_lossless.avif"),
        dir.join("lossless.avif"),
    )
    .unwrap();
    std::fs::write(dir.join("broken.avif"), b"\0\0\0\x18ftypavif\0\0\0\0avif").unwrap();
    // HEIC is never scanned.
    std::fs::write(dir.join("photo.heic"), b"heic bytes").unwrap();
    let v = scan_json(&dir, &[]);
    let errors: Vec<String> = v["errors"]
        .as_array()
        .unwrap()
        .iter()
        .map(|e| e["path"].as_str().unwrap().to_string())
        .collect();
    assert!(
        errors.iter().any(|p| p.ends_with("lossless.avif")),
        "{errors:?}"
    );
    assert!(
        errors.iter().any(|p| p.ends_with("broken.avif")),
        "{errors:?}"
    );
    assert!(!scanned_paths(&v).iter().any(|p| p.ends_with("photo.heic")));
    assert!(!errors.iter().any(|p| p.ends_with("photo.heic")));
    assert_eq!(v["families"].as_array().unwrap().len(), 1);
}

fn scanned_paths(v: &serde_json::Value) -> Vec<String> {
    v["files"]
        .as_array()
        .unwrap()
        .iter()
        .map(|f| f["path"].as_str().unwrap().to_string())
        .collect()
}

#[test]
fn images_only_skips_documents_entirely() {
    let dir = photo_dir("images-only");
    std::fs::write(dir.join("a.txt"), "quarterly plan\n".repeat(40)).unwrap();
    std::fs::write(dir.join("b.txt"), "quarterly plan\n".repeat(40)).unwrap();
    let v = scan_json(&dir, &["--images-only"]);
    let files = v["files"].as_array().unwrap();
    assert!(files
        .iter()
        .all(|f| !f["path"].as_str().unwrap().ends_with(".txt")));
    assert_eq!(files.len(), 5, "the five images only");
    let families = v["families"].as_array().unwrap();
    assert_eq!(families.len(), 1, "no text family");
    assert_eq!(
        names(&families[0]),
        ["photo.png", "photo_copy.bmp", "photo_small.jpg"]
    );
}

#[test]
fn scan_json_lists_skipped_cloud_placeholders_separately_from_errors() {
    // A normal local folder skips nothing; the field is always present so
    // callers can rely on it.
    let dir = photo_dir("skipped-empty");
    let v = scan_json(&dir, &[]);
    assert_eq!(v["skipped"], serde_json::json!([]), "{v:#}");
    assert!(v["errors"].as_array().unwrap().is_empty());
}

#[test]
fn fingerprint_and_compare_refuse_nothing_for_local_files() {
    let dir = photo_dir("local-ok");
    for cmd in ["fingerprint", "compare"] {
        let mut c = dupey();
        c.arg(cmd).arg(dir.join("photo.png"));
        if cmd == "compare" {
            c.arg(dir.join("photo_copy.bmp"));
        }
        assert!(c.output().unwrap().status.success(), "{cmd}");
    }
}

/// Real Cloud Files API placeholders, the same kind OneDrive Files On-Demand
/// creates. `Attributes` cannot be set to these bits from PowerShell or the
/// .NET enum, so the test registers a sync root and creates placeholders
/// through `cldapi.dll`, which is what a sync provider does.
#[cfg(windows)]
fn create_cloud_placeholders(root: &Path, names: &[&str]) {
    let list = names
        .iter()
        .map(|n| format!("'{n}'"))
        .collect::<Vec<_>>()
        .join(",");
    let script = format!(
        r#"
$ErrorActionPreference = 'Stop'
Add-Type -TypeDefinition @"
using System;
using System.Runtime.InteropServices;
public static class CF {{
  [StructLayout(LayoutKind.Sequential, CharSet=CharSet.Unicode)]
  public struct SYNC_REG {{ public uint StructSize; [MarshalAs(UnmanagedType.LPWStr)] public string ProviderName; [MarshalAs(UnmanagedType.LPWStr)] public string ProviderVersion; public IntPtr SyncRootIdentity; public uint SyncRootIdentityLength; public IntPtr FileIdentity; public uint FileIdentityLength; public Guid ProviderId; }}
  [StructLayout(LayoutKind.Sequential)] public struct HYDRATION {{ public ushort Primary; public ushort Modifier; }}
  [StructLayout(LayoutKind.Sequential)] public struct SYNC_POLICIES {{ public uint StructSize; public HYDRATION Hydration; public HYDRATION Population; public uint InSync; public uint HardLink; public uint PlaceholderManagement; }}
  [DllImport("cldapi.dll", CharSet=CharSet.Unicode)] public static extern int CfRegisterSyncRoot(string path, ref SYNC_REG reg, ref SYNC_POLICIES pol, uint flags);
  [StructLayout(LayoutKind.Sequential)] public struct FS_META {{ public long CreationTime; public long LastAccessTime; public long LastWriteTime; public long ChangeTime; public uint FileAttributes; public uint Pad; public long FileSize; }}
  [StructLayout(LayoutKind.Sequential, CharSet=CharSet.Unicode)] public struct PH_CREATE {{ [MarshalAs(UnmanagedType.LPWStr)] public string RelativeFileName; public FS_META FsMetadata; public IntPtr FileIdentity; public uint FileIdentityLength; public uint Flags; public int Result; public long CreateUsn; }}
  [DllImport("cldapi.dll", CharSet=CharSet.Unicode)] public static extern int CfCreatePlaceholders(string baseDir, [In, Out] PH_CREATE[] entries, uint count, uint flags, out uint processed);
}}
"@
$root = '{root}'
$id = [Runtime.InteropServices.Marshal]::StringToHGlobalUni('dupey-test')
$reg = New-Object CF+SYNC_REG
$reg.StructSize = [Runtime.InteropServices.Marshal]::SizeOf($reg)
$reg.ProviderName = 'DupeyTest'; $reg.ProviderVersion = '1.0'
$reg.SyncRootIdentity = $id; $reg.SyncRootIdentityLength = 20; $reg.ProviderId = [Guid]::NewGuid()
$pol = New-Object CF+SYNC_POLICIES
$pol.StructSize = [Runtime.InteropServices.Marshal]::SizeOf($pol)
$pol.Hydration = New-Object CF+HYDRATION; $pol.Hydration.Primary = 2
$pol.Population = New-Object CF+HYDRATION; $pol.Population.Primary = 2
$hr = [CF]::CfRegisterSyncRoot($root, [ref]$reg, [ref]$pol, 2)
if ($hr -ne 0) {{ throw ('CfRegisterSyncRoot 0x{{0:x}}' -f $hr) }}
$entries = @()
foreach ($n in @({list})) {{
  $e = New-Object CF+PH_CREATE
  $e.RelativeFileName = $n
  $e.FsMetadata = New-Object CF+FS_META
  $e.FsMetadata.FileAttributes = 0x20
  $e.FsMetadata.FileSize = 123456
  $now = [DateTime]::UtcNow.ToFileTimeUtc()
  $e.FsMetadata.CreationTime = $now; $e.FsMetadata.LastWriteTime = $now; $e.FsMetadata.LastAccessTime = $now; $e.FsMetadata.ChangeTime = $now
  $e.FileIdentity = $id; $e.FileIdentityLength = 20; $e.Flags = 1
  $entries += $e
}}
$processed = 0
$hr = [CF]::CfCreatePlaceholders($root, $entries, $entries.Length, 0, [ref]$processed)
if ($hr -ne 0 -or $processed -ne $entries.Length) {{ throw ('CfCreatePlaceholders 0x{{0:x}} processed={{1}}' -f $hr, $processed) }}
"#,
        root = root.display(),
        list = list
    );
    let out = Command::new("powershell")
        .args([
            "-NoProfile",
            "-ExecutionPolicy",
            "Bypass",
            "-Command",
            &script,
        ])
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "cannot create cloud placeholders: {}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
}

#[cfg(windows)]
#[test]
fn windows_cloud_placeholders_are_skipped_not_read() {
    // A sync root holds the placeholders; real images live next to it.
    let dir = photo_dir("win-cloud");
    let cloud = dir.join("cloud");
    std::fs::create_dir_all(&cloud).unwrap();
    create_cloud_placeholders(&cloud, &["online_only.png", "online_only2.jpg"]);
    save(
        &cloud,
        "local_in_sync_root.png",
        &scene(50, 128, 96),
        ImageFormat::Png,
    );
    let v = scan_json(&dir, &[]);
    let skipped: Vec<String> = v["skipped"]
        .as_array()
        .unwrap()
        .iter()
        .map(|e| {
            assert_eq!(e["reason"], "cloud_placeholder");
            Path::new(e["path"].as_str().unwrap())
                .file_name()
                .unwrap()
                .to_string_lossy()
                .into_owned()
        })
        .collect();
    assert_eq!(skipped.len(), 2, "{skipped:?} {v:#}");
    assert!(skipped.contains(&"online_only.png".to_string()));
    assert!(skipped.contains(&"online_only2.jpg".to_string()));
    // Nothing was opened, so nothing failed to decode.
    assert!(v["errors"].as_array().unwrap().is_empty(), "{v:#}");
    assert!(!scanned_paths_of(&v)
        .iter()
        .any(|p| p.contains("online_only")));
    // Local files, even inside the sync root, still scan and cluster.
    assert!(scanned_paths_of(&v)
        .iter()
        .any(|p| p.ends_with("local_in_sync_root.png")));
    assert_eq!(v["families"].as_array().unwrap().len(), 1);
    // Naming a placeholder directly is refused, not downloaded.
    let out = dupey()
        .arg("fingerprint")
        .arg(cloud.join("online_only.png"))
        .output()
        .unwrap();
    assert!(!out.status.success());
    assert!(String::from_utf8_lossy(&out.stderr).contains("cloud placeholder"));
}

#[cfg(windows)]
fn scanned_paths_of(v: &serde_json::Value) -> Vec<String> {
    v["files"]
        .as_array()
        .unwrap()
        .iter()
        .map(|f| f["path"].as_str().unwrap().to_string())
        .collect()
}

/// OneDrive for Mac keeps some placeholders as zero-filled stubs that carry
/// no `SF_DATALESS` flag, only this extended attribute.
#[cfg(target_os = "macos")]
#[test]
fn macos_onedrive_stub_files_are_skipped_not_reported_as_corrupt() {
    let dir = photo_dir("mac-onedrive-stub");
    // A zero-filled "PNG": exactly what such a stub looks like when read.
    let stub = dir.join("stub.png");
    std::fs::write(&stub, vec![0u8; 4096]).unwrap();
    let real_but_unmarked = dir.join("unmarked_zero.png");
    std::fs::write(&real_but_unmarked, vec![0u8; 4096]).unwrap();
    let out = Command::new("xattr")
        .args(["-w", "com.microsoft.OneDrive.Macbox.PhData", "DENO"])
        .arg(&stub)
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );

    let v = scan_json(&dir, &[]);
    let skipped: Vec<String> = v["skipped"]
        .as_array()
        .unwrap()
        .iter()
        .map(|e| e["path"].as_str().unwrap().to_string())
        .collect();
    assert_eq!(skipped.len(), 1, "{skipped:?}");
    assert!(skipped[0].ends_with("stub.png"));
    // Without the marker the same bytes are an honest decode error: only
    // the placeholder marker earns a skip.
    let errors: Vec<String> = v["errors"]
        .as_array()
        .unwrap()
        .iter()
        .map(|e| e["path"].as_str().unwrap().to_string())
        .collect();
    assert_eq!(errors.len(), 1, "{errors:?}");
    assert!(errors[0].ends_with("unmarked_zero.png"));
    let out = dupey().arg("fingerprint").arg(&stub).output().unwrap();
    assert!(!out.status.success());
    assert!(String::from_utf8_lossy(&out.stderr).contains("cloud placeholder"));
}

/// A folder the scan cannot read must show up in `errors[]`: silently
/// dropping it would let a whole subtree look like "no duplicates here".
#[cfg(unix)]
#[test]
fn unreadable_directories_are_reported_not_silently_dropped() {
    use std::os::unix::fs::PermissionsExt;
    let dir = photo_dir("unreadable-dir");
    let locked = dir.join("locked_folder");
    std::fs::create_dir_all(&locked).unwrap();
    std::fs::copy(dir.join("photo.png"), locked.join("hidden_copy.png")).unwrap();
    std::fs::set_permissions(&locked, std::fs::Permissions::from_mode(0o000)).unwrap();
    // Root ignores permission bits; the check is only meaningful otherwise.
    let readable = std::fs::read_dir(&locked).is_ok();

    let v = scan_json(&dir, &[]);
    std::fs::set_permissions(&locked, std::fs::Permissions::from_mode(0o755)).unwrap();
    if readable {
        return;
    }
    let errors: Vec<String> = v["errors"]
        .as_array()
        .unwrap()
        .iter()
        .map(|e| e["path"].as_str().unwrap().to_string())
        .collect();
    assert_eq!(errors.len(), 1, "{errors:?}");
    assert!(errors[0].ends_with("locked_folder"), "{errors:?}");
    // The rest of the tree is still scanned.
    assert_eq!(v["families"].as_array().unwrap().len(), 1);
}
