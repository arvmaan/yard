//! Images pasted into a Yard web terminal.
//!
//! The agents run on the Yard host and cannot read the browser's clipboard,
//! so the browser uploads the image bytes and Yard writes them to a private
//! per-worker directory under its data root. The browser then types the
//! returned absolute path into the pane, where Claude Code or Codex reads it.
//!
//! Layout: `<root>/<worker>/<yyyymmdd>-<uuid>.<ext>`, directories `0700`,
//! files `0600`. The root carries a marker file and must be new, empty or
//! already marked, so Yard never adopts a directory with other content. Each
//! write prunes images older than the retention window in every worker
//! directory and keeps at most `max_files_per_worker` in the one it writes
//! to; only regular files with Yard-generated names are removed. Below the
//! root, every directory and file is opened relative to a descriptor with
//! `O_NOFOLLOW`, so nothing here follows a symbolic link.

use std::{
    ffi::CString,
    fmt::Write as _,
    fs, io,
    io::Write as _,
    os::{
        fd::{AsFd, OwnedFd},
        unix::fs::DirBuilderExt as _,
    },
    path::{Path, PathBuf},
    time::{Duration, SystemTime},
};

use rustix::{
    fs::{
        AtFlags, CWD, Dir, FileType, Mode, OFlags, fchmod, fstat, mkdirat, openat, statat, unlinkat,
    },
    io::Errno,
};
use sha2::{Digest as _, Sha256};

use thiserror::Error;

/// Default upload limit: 10 MiB.
pub const DEFAULT_IMAGE_PASTE_MAX_BYTES: u64 = 10 * 1024 * 1024;
/// Default retention for pasted images, in days.
pub const DEFAULT_IMAGE_PASTE_RETENTION_DAYS: u32 = 7;
/// Default number of pasted images kept per worker.
pub const DEFAULT_IMAGE_PASTE_MAX_FILES: usize = 200;

/// The image formats Yard accepts, each checked against its magic bytes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PastedImageType {
    Png,
    Jpeg,
    Gif,
    Webp,
}

impl PastedImageType {
    /// Parse a `Content-Type` value, ignoring parameters and case.
    #[must_use]
    pub fn from_content_type(value: &str) -> Option<Self> {
        let essence = value.split(';').next().unwrap_or_default().trim();
        match essence.to_ascii_lowercase().as_str() {
            "image/png" => Some(Self::Png),
            "image/jpeg" | "image/jpg" => Some(Self::Jpeg),
            "image/gif" => Some(Self::Gif),
            "image/webp" => Some(Self::Webp),
            _ => None,
        }
    }

    /// Whether `bytes` start with this format's signature.
    #[must_use]
    pub fn matches(self, bytes: &[u8]) -> bool {
        match self {
            Self::Png => bytes.starts_with(b"\x89PNG\r\n\x1a\n"),
            Self::Jpeg => bytes.starts_with(&[0xFF, 0xD8, 0xFF]),
            Self::Gif => bytes.starts_with(b"GIF87a") || bytes.starts_with(b"GIF89a"),
            Self::Webp => bytes.len() >= 12 && &bytes[..4] == b"RIFF" && &bytes[8..12] == b"WEBP",
        }
    }

    #[must_use]
    pub fn extension(self) -> &'static str {
        match self {
            Self::Png => "png",
            Self::Jpeg => "jpg",
            Self::Gif => "gif",
            Self::Webp => "webp",
        }
    }

    #[must_use]
    pub fn mime(self) -> &'static str {
        match self {
            Self::Png => "image/png",
            Self::Jpeg => "image/jpeg",
            Self::Gif => "image/gif",
            Self::Webp => "image/webp",
        }
    }
}

/// Where pasted images go and how many are kept.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ImagePasteSettings {
    /// Absolute root; `None` disables uploads (tests and embedders that did
    /// not configure a data root).
    root: Option<PathBuf>,
    pub max_bytes: u64,
    pub retention: Duration,
    pub max_files_per_worker: usize,
}

impl ImagePasteSettings {
    /// Uploads are refused with `image_paste_not_configured`.
    #[must_use]
    pub fn not_configured() -> Self {
        Self {
            root: None,
            max_bytes: DEFAULT_IMAGE_PASTE_MAX_BYTES,
            retention: days(DEFAULT_IMAGE_PASTE_RETENTION_DAYS),
            max_files_per_worker: DEFAULT_IMAGE_PASTE_MAX_FILES,
        }
    }

    /// Uploads go under `root` with the default limits.
    #[must_use]
    pub fn under(root: PathBuf) -> Self {
        Self {
            root: Some(root),
            ..Self::not_configured()
        }
    }

    /// Settings from [`crate::config::ImagePasteConfig`].
    #[must_use]
    pub fn from_config(config: &crate::config::ImagePasteConfig) -> Self {
        Self {
            root: Some(config.path.clone()),
            max_bytes: config.max_bytes,
            retention: days(config.retention_days),
            max_files_per_worker: config.max_files_per_worker,
        }
    }

    #[must_use]
    pub fn root(&self) -> Option<&Path> {
        self.root.as_deref()
    }
}

fn days(days: u32) -> Duration {
    Duration::from_secs(u64::from(days) * 24 * 60 * 60)
}

/// One stored image.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SavedImage {
    pub path: PathBuf,
    pub name: String,
    pub bytes: u64,
    pub image_type: PastedImageType,
}

#[derive(Debug, Error)]
pub enum ImagePasteError {
    #[error("image paste is not configured on this Yard")]
    NotConfigured,
    #[error("the pasted data is empty")]
    Empty,
    #[error("unsupported image type '{0}'; paste a PNG, JPEG, GIF or WebP image")]
    UnsupportedType(String),
    #[error("the image data is not a valid {0} file")]
    TypeMismatch(&'static str),
    #[error("the image is {}; the limit is {}", human_size(*bytes), human_size(*limit))]
    TooLarge { bytes: u64, limit: u64 },
    #[error("'{0}' is not a private directory Yard can write pasted images to")]
    UnsafeDirectory(PathBuf),
    #[error("the pasted image could not be stored: {0}")]
    Io(#[from] io::Error),
}

/// `bytes` as B, KiB or MiB (one decimal) for error messages.
fn human_size(bytes: u64) -> String {
    const KIB: u64 = 1024;
    const MIB: u64 = 1024 * 1024;
    if bytes < KIB {
        format!("{bytes} B")
    } else if bytes < MIB {
        format!("{} KiB", bytes.div_ceil(KIB))
    } else {
        format!("{}.{} MiB", bytes / MIB, (bytes % MIB) * 10 / MIB)
    }
}

/// Validate and store one pasted image for `worker_id`.
///
/// # Errors
///
/// Returns [`ImagePasteError`] when uploads are not configured, the type is
/// unsupported or does not match the bytes, the image is too large, or a
/// directory on the way is not a real private directory.
pub fn save_pasted_image(
    settings: &ImagePasteSettings,
    worker_id: &str,
    content_type: &str,
    bytes: &[u8],
    now: SystemTime,
) -> Result<SavedImage, ImagePasteError> {
    let root = settings.root().ok_or(ImagePasteError::NotConfigured)?;
    let image_type = PastedImageType::from_content_type(content_type)
        .ok_or_else(|| ImagePasteError::UnsupportedType(content_type.to_owned()))?;
    let length = u64::try_from(bytes.len()).unwrap_or(u64::MAX);
    if length == 0 {
        return Err(ImagePasteError::Empty);
    }
    if length > settings.max_bytes {
        return Err(ImagePasteError::TooLarge {
            bytes: length,
            limit: settings.max_bytes,
        });
    }
    if !image_type.matches(bytes) {
        return Err(ImagePasteError::TypeMismatch(image_type.mime()));
    }
    if let Some(parent) = root.parent() {
        fs::DirBuilder::new()
            .recursive(true)
            .mode(0o700)
            .create(parent)?;
    }
    let root_fd = open_root(root)?;
    let worker_name = worker_directory_name(worker_id);
    let worker_dir = root.join(&worker_name);
    let dir_fd = open_private_dir(&root_fd, &worker_name, &worker_dir)?;
    prune(&root_fd, &worker_name, settings, now);

    let date = chrono::DateTime::<chrono::Utc>::from(now).format("%Y%m%d");
    let name = format!("{date}-{}.{}", uuid::Uuid::now_v7(), image_type.extension());
    let path = worker_dir.join(&name);
    let file = openat(
        &dir_fd,
        name.as_str(),
        OFlags::WRONLY | OFlags::CREATE | OFlags::EXCL | OFlags::NOFOLLOW | OFlags::CLOEXEC,
        Mode::from_raw_mode(0o600),
    )
    .map_err(io::Error::from)?;
    let mut file = fs::File::from(file);
    let written = file.write_all(bytes).and_then(|()| file.sync_all());
    if let Err(error) = written {
        drop(file);
        let _ = unlinkat(&dir_fd, name.as_str(), AtFlags::empty());
        return Err(error.into());
    }
    Ok(SavedImage {
        path,
        name,
        bytes: length,
        image_type,
    })
}

/// A directory name for a worker id: the id itself when it is a plain name
/// of at most 128 characters; otherwise the id with every other character
/// replaced by `_`, shortened, and suffixed with a hash of the full id so two
/// different ids never share a directory.
#[must_use]
pub fn worker_directory_name(worker_id: &str) -> String {
    const MAX: usize = 128;
    const HASH_HEX: usize = 12;
    let plain = !worker_id.is_empty()
        && worker_id.len() <= MAX
        && worker_id
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_'));
    if plain {
        return worker_id.to_owned();
    }
    let sanitized: String = worker_id
        .chars()
        .take(MAX - HASH_HEX - 1)
        .map(|c| {
            if c.is_ascii_alphanumeric() || matches!(c, '-' | '_') {
                c
            } else {
                '_'
            }
        })
        .collect();
    let digest = Sha256::digest(worker_id.as_bytes());
    let hash = digest
        .iter()
        .take(HASH_HEX / 2)
        .fold(String::new(), |mut hash, byte| {
            let _ = write!(hash, "{byte:02x}");
            hash
        });
    if sanitized.is_empty() {
        format!("_-{hash}")
    } else {
        format!("{sanitized}-{hash}")
    }
}

/// The file Yard writes into a paste root it owns. A root that already holds
/// other files but no marker is not Yard's, so Yard neither tightens its mode
/// nor prunes anything in it.
pub const IMAGE_PASTE_ROOT_MARKER: &str = ".yard-image-pastes";

/// Open (creating if needed) the paste root as a private directory and make
/// sure it is dedicated to pasted images: it must be new, empty, or already
/// carry [`IMAGE_PASTE_ROOT_MARKER`].
///
/// Initialisation is idempotent under concurrency: pastes in this process are
/// serialised while the marker is checked and created (the first pastes run in
/// parallel), and a marker another process created first (`EEXIST`) is
/// accepted once it is confirmed to be a regular file.
fn open_root(root: &Path) -> Result<OwnedFd, ImagePasteError> {
    static ROOT_INIT: std::sync::Mutex<()> = std::sync::Mutex::new(());
    let (root_fd, mode) = open_owned_dir(CWD, root, root)?;
    {
        let _init = ROOT_INIT
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        ensure_root_marker(&root_fd, root)?;
    }
    tighten(&root_fd, mode)?;
    Ok(root_fd)
}

fn ensure_root_marker(root_fd: &OwnedFd, root: &Path) -> Result<(), ImagePasteError> {
    let marker_is_file =
        || match statat(root_fd, IMAGE_PASTE_ROOT_MARKER, AtFlags::SYMLINK_NOFOLLOW) {
            Ok(stat) if FileType::from_raw_mode(stat.st_mode) == FileType::RegularFile => Ok(true),
            Ok(_) => Err(ImagePasteError::UnsafeDirectory(root.to_path_buf())),
            Err(Errno::NOENT) => Ok(false),
            Err(error) => Err(io::Error::from(error).into()),
        };
    if marker_is_file()? {
        return Ok(());
    }
    if !directory_names(root_fd)?.is_empty() {
        // Another process may have just initialised it.
        if marker_is_file()? {
            return Ok(());
        }
        return Err(ImagePasteError::UnsafeDirectory(root.to_path_buf()));
    }
    let marker = match openat(
        root_fd,
        IMAGE_PASTE_ROOT_MARKER,
        OFlags::WRONLY | OFlags::CREATE | OFlags::EXCL | OFlags::NOFOLLOW | OFlags::CLOEXEC,
        Mode::from_raw_mode(0o600),
    ) {
        Ok(marker) => marker,
        Err(Errno::EXIST) if marker_is_file()? => return Ok(()),
        Err(Errno::EXIST) => return Err(ImagePasteError::UnsafeDirectory(root.to_path_buf())),
        Err(error) => return Err(io::Error::from(error).into()),
    };
    let mut marker = fs::File::from(marker);
    marker.write_all(b"Images pasted into Yard web terminals. Yard prunes this directory.\n")?;
    Ok(())
}

/// Create `name` under `parent` as a `0700` directory, or accept an existing
/// real directory owned by this user, and open it without following a
/// symbolic link. The mode is checked and tightened on the open descriptor,
/// so a swapped-in symlink can never redirect the write or the `chmod`.
fn open_private_dir<Fd: AsFd, P: rustix::path::Arg + Copy>(
    parent: Fd,
    name: P,
    shown: &Path,
) -> Result<OwnedFd, ImagePasteError> {
    let (fd, mode) = open_owned_dir(parent, name, shown)?;
    tighten(&fd, mode)?;
    Ok(fd)
}

fn tighten(fd: &OwnedFd, mode: u32) -> Result<(), ImagePasteError> {
    if mode & 0o777 != 0o700 {
        fchmod(fd, Mode::from_raw_mode(0o700)).map_err(io::Error::from)?;
    }
    Ok(())
}

/// Create or open `name` under `parent` as a real directory owned by this
/// user, returning the descriptor and its current mode.
fn open_owned_dir<Fd: AsFd, P: rustix::path::Arg + Copy>(
    parent: Fd,
    name: P,
    shown: &Path,
) -> Result<(OwnedFd, u32), ImagePasteError> {
    match mkdirat(&parent, name, Mode::from_raw_mode(0o700)) {
        Ok(()) | Err(Errno::EXIST) => {}
        Err(error) => return Err(io::Error::from(error).into()),
    }
    let unsafe_dir = || ImagePasteError::UnsafeDirectory(shown.to_path_buf());
    let fd = match openat(
        &parent,
        name,
        OFlags::RDONLY | OFlags::DIRECTORY | OFlags::NOFOLLOW | OFlags::CLOEXEC,
        Mode::empty(),
    ) {
        Ok(fd) => fd,
        Err(Errno::LOOP | Errno::NOTDIR) => return Err(unsafe_dir()),
        Err(error) => return Err(io::Error::from(error).into()),
    };
    let stat = fstat(&fd).map_err(io::Error::from)?;
    if FileType::from_raw_mode(stat.st_mode) != FileType::Directory
        || stat.st_uid != rustix::process::geteuid().as_raw()
    {
        return Err(unsafe_dir());
    }
    #[allow(
        clippy::useless_conversion,
        reason = "st_mode is u16 on macOS, u32 on Linux"
    )]
    let mode = u32::from(stat.st_mode);
    Ok((fd, mode))
}

/// Whether `name` is one Yard generated: `<yyyymmdd>-<uuid>.<png|jpg|gif|webp>`.
#[must_use]
pub fn is_pasted_image_name(name: &str) -> bool {
    let Some((stem, extension)) = name.rsplit_once('.') else {
        return false;
    };
    if !matches!(extension, "png" | "jpg" | "gif" | "webp") {
        return false;
    }
    let Some((date, id)) = stem.split_once('-') else {
        return false;
    };
    date.len() == 8
        && date.bytes().all(|byte| byte.is_ascii_digit())
        && id.len() == 36
        && uuid::Uuid::parse_str(id).is_ok()
}

/// Remove expired images in every worker directory, then trim the one about
/// to receive a new image to `max_files_per_worker - 1` so it holds at most
/// `max_files_per_worker` afterwards. Best effort: a failure to prune never
/// blocks a paste. Only regular files directly inside a real worker
/// directory whose names Yard generated are ever removed, and every lookup
/// and removal is relative to a descriptor opened without following links.
fn prune(root_fd: &OwnedFd, worker_name: &str, settings: &ImagePasteSettings, now: SystemTime) {
    let Some(cutoff) = now.checked_sub(settings.retention) else {
        return;
    };
    let Ok(workers) = directory_names(root_fd) else {
        return;
    };
    for worker in workers {
        let Ok(dir) = openat(
            root_fd,
            worker.as_c_str(),
            OFlags::RDONLY | OFlags::DIRECTORY | OFlags::NOFOLLOW | OFlags::CLOEXEC,
            Mode::empty(),
        ) else {
            continue;
        };
        let mut kept = Vec::new();
        for (file, modified) in pasted_images(&dir) {
            if modified < cutoff {
                let _ = unlinkat(&dir, file.as_c_str(), AtFlags::empty());
            } else {
                kept.push((file, modified));
            }
        }
        if worker.as_bytes() == worker_name.as_bytes() {
            let keep = settings.max_files_per_worker.saturating_sub(1);
            if kept.len() > keep {
                kept.sort_by(|left, right| left.1.cmp(&right.1).then_with(|| left.0.cmp(&right.0)));
                let excess = kept.len() - keep;
                for (file, _) in kept.drain(..excess) {
                    let _ = unlinkat(&dir, file.as_c_str(), AtFlags::empty());
                }
            }
        }
    }
}

/// Every entry name in `dir` except `.` and `..`.
fn directory_names(dir: &OwnedFd) -> io::Result<Vec<CString>> {
    let mut names = Vec::new();
    for entry in Dir::read_from(dir).map_err(io::Error::from)? {
        let entry = entry.map_err(io::Error::from)?;
        let name = entry.file_name();
        if name != c"." && name != c".." {
            names.push(name.to_owned());
        }
    }
    Ok(names)
}

/// Regular files in `dir` with Yard-generated names, and their mtimes.
fn pasted_images(dir: &OwnedFd) -> Vec<(CString, SystemTime)> {
    let Ok(names) = directory_names(dir) else {
        return Vec::new();
    };
    names
        .into_iter()
        .filter(|name| name.to_str().is_ok_and(is_pasted_image_name))
        .filter_map(|name| {
            let stat = statat(dir, name.as_c_str(), AtFlags::SYMLINK_NOFOLLOW).ok()?;
            if FileType::from_raw_mode(stat.st_mode) != FileType::RegularFile {
                return None;
            }
            let seconds = u64::try_from(stat.st_mtime).ok()?;
            let nanos = u32::try_from(stat.st_mtime_nsec).unwrap_or(0);
            Some((
                name,
                SystemTime::UNIX_EPOCH + Duration::new(seconds, nanos.min(999_999_999)),
            ))
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use std::{
        fs,
        os::unix::fs::PermissionsExt as _,
        path::Path,
        time::{Duration, SystemTime},
    };

    use tempfile::TempDir;

    use super::{
        IMAGE_PASTE_ROOT_MARKER, ImagePasteError, ImagePasteSettings, PastedImageType,
        is_pasted_image_name, save_pasted_image, worker_directory_name,
    };

    const PNG: &[u8] = b"\x89PNG\r\n\x1a\n\0\0\0\rIHDR";
    const DAY: Duration = Duration::from_secs(24 * 60 * 60);

    fn settings(temp: &TempDir) -> ImagePasteSettings {
        ImagePasteSettings::under(temp.path().join("pastes"))
    }

    fn mode(path: &Path) -> u32 {
        fs::symlink_metadata(path).unwrap().permissions().mode() & 0o777
    }

    fn age(path: &Path, modified: SystemTime) {
        fs::File::options()
            .write(true)
            .open(path)
            .unwrap()
            .set_modified(modified)
            .unwrap();
    }

    #[test]
    fn too_large_messages_use_readable_sizes() {
        let message = |bytes, limit| ImagePasteError::TooLarge { bytes, limit }.to_string();
        assert_eq!(message(17, 16), "the image is 17 B; the limit is 16 B");
        assert_eq!(
            message(1500, 1024),
            "the image is 2 KiB; the limit is 1 KiB"
        );
        assert_eq!(
            message(11 * 1024 * 1024 + 512 * 1024, 10 * 1024 * 1024),
            "the image is 11.5 MiB; the limit is 10.0 MiB"
        );
    }

    #[test]
    fn content_types_and_signatures() {
        assert_eq!(
            PastedImageType::from_content_type("Image/PNG; charset=binary"),
            Some(PastedImageType::Png)
        );
        assert_eq!(
            PastedImageType::from_content_type("image/jpg"),
            Some(PastedImageType::Jpeg)
        );
        assert_eq!(PastedImageType::from_content_type("image/svg+xml"), None);
        assert_eq!(PastedImageType::from_content_type(""), None);
        assert!(PastedImageType::Png.matches(PNG));
        assert!(PastedImageType::Jpeg.matches(&[0xFF, 0xD8, 0xFF, 0xE0]));
        assert!(PastedImageType::Gif.matches(b"GIF89a.."));
        assert!(PastedImageType::Gif.matches(b"GIF87a.."));
        assert!(PastedImageType::Webp.matches(b"RIFF\x10\0\0\0WEBPVP8 "));
        assert!(!PastedImageType::Webp.matches(b"RIFF\x10\0\0\0WAVE"));
        assert!(!PastedImageType::Png.matches(b"GIF89a"));
        assert!(!PastedImageType::Jpeg.matches(&[0xFF, 0xD8]));
    }

    #[test]
    fn concurrent_first_pastes_into_a_fresh_root_all_succeed() {
        for _round in 0..20 {
            let temp = TempDir::new().unwrap();
            let settings = settings(&temp);
            let now = SystemTime::UNIX_EPOCH + Duration::from_secs(1_760_659_200);
            let barrier = std::sync::Barrier::new(4);
            std::thread::scope(|scope| {
                let pastes = (0..4)
                    .map(|index| {
                        let (settings, barrier) = (&settings, &barrier);
                        scope.spawn(move || {
                            barrier.wait();
                            save_pasted_image(
                                settings,
                                &format!("worker-{index}"),
                                "image/png",
                                PNG,
                                now,
                            )
                        })
                    })
                    .collect::<Vec<_>>();
                for paste in pastes {
                    let saved = paste.join().unwrap();
                    assert!(saved.is_ok(), "{:?}", saved.err());
                }
            });
            assert!(
                temp.path()
                    .join("pastes")
                    .join(IMAGE_PASTE_ROOT_MARKER)
                    .is_file()
            );
        }
    }

    #[test]
    fn stores_a_private_file_under_the_worker_directory() {
        let temp = TempDir::new().unwrap();
        let settings = settings(&temp);
        let now = SystemTime::UNIX_EPOCH + Duration::from_secs(1_760_659_200);
        let saved = save_pasted_image(&settings, "worker-1", "image/png", PNG, now).unwrap();
        let worker_dir = temp.path().join("pastes/worker-1");
        assert!(saved.path.is_absolute());
        assert_eq!(saved.path.parent().unwrap(), worker_dir);
        assert!(saved.name.starts_with("20251017-"), "{}", saved.name);
        assert_eq!(Path::new(&saved.name).extension().unwrap(), "png");
        assert_eq!(saved.bytes, u64::try_from(PNG.len()).unwrap());
        assert_eq!(fs::read(&saved.path).unwrap(), PNG);
        assert_eq!(mode(&saved.path), 0o600);
        assert_eq!(mode(&worker_dir), 0o700);
        assert_eq!(mode(&temp.path().join("pastes")), 0o700);
    }

    #[test]
    fn refuses_bad_types_sizes_and_unconfigured_roots() {
        let temp = TempDir::new().unwrap();
        let mut settings = settings(&temp);
        let now = SystemTime::now();
        assert!(matches!(
            save_pasted_image(&settings, "w", "image/jpeg", PNG, now),
            Err(ImagePasteError::TypeMismatch("image/jpeg"))
        ));
        assert!(matches!(
            save_pasted_image(&settings, "w", "text/plain", PNG, now),
            Err(ImagePasteError::UnsupportedType(_))
        ));
        assert!(matches!(
            save_pasted_image(&settings, "w", "image/png", &[], now),
            Err(ImagePasteError::Empty)
        ));
        settings.max_bytes = 8;
        assert!(matches!(
            save_pasted_image(&settings, "w", "image/png", PNG, now),
            Err(ImagePasteError::TooLarge {
                bytes: 16,
                limit: 8
            })
        ));
        assert!(matches!(
            save_pasted_image(
                &ImagePasteSettings::not_configured(),
                "w",
                "image/png",
                PNG,
                now
            ),
            Err(ImagePasteError::NotConfigured)
        ));
        // Nothing was written for any refused paste.
        assert!(!temp.path().join("pastes").exists());
    }

    #[test]
    fn prunes_expired_images_everywhere_and_caps_the_worker_directory() {
        let temp = TempDir::new().unwrap();
        let mut settings = settings(&temp);
        settings.max_files_per_worker = 3;
        let now = SystemTime::now();
        let first = save_pasted_image(&settings, "a", "image/png", PNG, now).unwrap();
        let other = save_pasted_image(&settings, "b", "image/png", PNG, now).unwrap();
        let stale_other = save_pasted_image(&settings, "b", "image/png", PNG, now).unwrap();
        age(&stale_other.path, now - 8 * DAY);
        age(&first.path, now - 2 * DAY);
        let mut kept = Vec::new();
        for _ in 0..3 {
            kept.push(save_pasted_image(&settings, "a", "image/png", PNG, now).unwrap());
        }
        // Worker `a` holds the cap; its oldest image went first.
        assert!(!first.path.exists());
        assert!(kept.iter().all(|saved| saved.path.exists()));
        assert_eq!(
            fs::read_dir(temp.path().join("pastes/a")).unwrap().count(),
            3
        );
        // Worker `b` lost only its expired image.
        assert!(other.path.exists());
        assert!(!stale_other.path.exists());
    }

    #[test]
    fn pruning_leaves_anything_but_regular_files_alone() {
        let temp = TempDir::new().unwrap();
        let settings = settings(&temp);
        let now = SystemTime::now();
        let saved = save_pasted_image(&settings, "a", "image/png", PNG, now).unwrap();
        let outside = temp.path().join("outside.png");
        fs::write(&outside, PNG).unwrap();
        age(&outside, now - 30 * DAY);
        let link = temp.path().join("pastes/a/link.png");
        std::os::unix::fs::symlink(&outside, &link).unwrap();
        let nested = temp.path().join("pastes/a/nested");
        fs::create_dir(&nested).unwrap();
        age(&saved.path, now - 30 * DAY);
        save_pasted_image(&settings, "a", "image/png", PNG, now).unwrap();
        assert!(!saved.path.exists());
        assert!(outside.exists());
        assert!(fs::symlink_metadata(&link).is_ok());
        assert!(nested.is_dir());
    }

    #[test]
    fn refuses_symlinked_directories() {
        let temp = TempDir::new().unwrap();
        let elsewhere = temp.path().join("elsewhere");
        fs::create_dir(&elsewhere).unwrap();
        let root = temp.path().join("pastes");
        let settings = ImagePasteSettings::under(root.clone());
        let now = SystemTime::now();
        save_pasted_image(&settings, "v", "image/png", PNG, now).unwrap();
        std::os::unix::fs::symlink(&elsewhere, root.join("w")).unwrap();
        assert!(matches!(
            save_pasted_image(&settings, "w", "image/png", PNG, now),
            Err(ImagePasteError::UnsafeDirectory(path)) if path == root.join("w")
        ));
        let linked_root = temp.path().join("linked");
        std::os::unix::fs::symlink(&elsewhere, &linked_root).unwrap();
        assert!(matches!(
            save_pasted_image(
                &ImagePasteSettings::under(linked_root),
                "x",
                "image/png",
                PNG,
                now
            ),
            Err(ImagePasteError::UnsafeDirectory(_))
        ));
        assert_eq!(fs::read_dir(&elsewhere).unwrap().count(), 0);
    }

    #[test]
    fn tightens_an_existing_root_and_names_workers_safely() {
        let temp = TempDir::new().unwrap();
        let root = temp.path().join("pastes");
        fs::create_dir(&root).unwrap();
        fs::set_permissions(&root, fs::Permissions::from_mode(0o755)).unwrap();
        let saved = save_pasted_image(
            &ImagePasteSettings::under(root.clone()),
            "../escape",
            "image/png",
            PNG,
            SystemTime::now(),
        )
        .unwrap();
        assert_eq!(mode(&root), 0o700);
        assert!(root.join(IMAGE_PASTE_ROOT_MARKER).is_file());
        let escaped = saved.path.parent().unwrap();
        assert_eq!(escaped.parent().unwrap(), root);
        assert!(
            escaped
                .file_name()
                .unwrap()
                .to_str()
                .unwrap()
                .starts_with("___escape-"),
            "{}",
            escaped.display()
        );
        assert_eq!(worker_directory_name("worker_01-a"), "worker_01-a");
        assert_eq!(worker_directory_name(&"x".repeat(128)), "x".repeat(128));
        assert!(worker_directory_name("").starts_with("_-"));
        assert!(worker_directory_name("a/b.c").starts_with("a_b_c-"));
        // Ids that sanitise alike, or share a long prefix, still get their own directory.
        assert_ne!(worker_directory_name("a.b"), worker_directory_name("a_b"));
        assert_ne!(worker_directory_name("a.b"), worker_directory_name("a,b"));
        let long = |tail: &str| worker_directory_name(&format!("{}{tail}", "x".repeat(200)));
        assert_ne!(long("1"), long("2"));
        assert!(long("1").len() <= 128);
        for name in [
            worker_directory_name("../.."),
            worker_directory_name("."),
            long("/"),
        ] {
            assert!(
                name.chars()
                    .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_')),
                "{name}"
            );
        }
    }

    #[test]
    fn only_generated_names_count_as_pasted_images() {
        assert!(is_pasted_image_name(
            "20261019-0199f0c1-7a2b-7c3d-8e4f-a1b2c3d4e5f6.png"
        ));
        assert!(is_pasted_image_name(
            "20261019-0199f0c1-7a2b-7c3d-8e4f-a1b2c3d4e5f6.webp"
        ));
        for name in [
            "notes.png",
            "20261019-0199f0c1-7a2b-7c3d-8e4f-a1b2c3d4e5f6.txt",
            "20261019-0199f0c1-7a2b-7c3d-8e4f-a1b2c3d4e5f6.PNG",
            "2026101-0199f0c1-7a2b-7c3d-8e4f-a1b2c3d4e5f6.png",
            "20261019-not-a-uuid-at-all-zzzzzzzzzzzzzzzz.png",
            "20261019-0199f0c17a2b7c3d8e4fa1b2c3d4e5f6.png",
            IMAGE_PASTE_ROOT_MARKER,
        ] {
            assert!(!is_pasted_image_name(name), "{name}");
        }
    }

    #[test]
    fn retention_never_removes_files_yard_did_not_write() {
        let temp = TempDir::new().unwrap();
        let settings = settings(&temp);
        let now = SystemTime::now();
        let saved = save_pasted_image(&settings, "a", "image/png", PNG, now).unwrap();
        let worker_dir = temp.path().join("pastes/a");
        let note = worker_dir.join("notes.txt");
        let photo = worker_dir.join("holiday.png");
        let other = temp.path().join("pastes/b");
        fs::create_dir(&other).unwrap();
        let foreign = other.join("report.pdf");
        for path in [&note, &photo, &foreign] {
            fs::write(path, b"keep me").unwrap();
            age(path, now - 30 * DAY);
        }
        age(&saved.path, now - 30 * DAY);
        let mut capped = settings.clone();
        capped.max_files_per_worker = 1;
        save_pasted_image(&capped, "a", "image/png", PNG, now).unwrap();
        assert!(!saved.path.exists());
        assert!(note.exists() && photo.exists() && foreign.exists());
    }

    #[test]
    fn refuses_a_populated_root_that_yard_did_not_create() {
        let temp = TempDir::new().unwrap();
        // For example YARD_IMAGE_PASTE_PATH pointing at Yard's data root or $HOME.
        let root = temp.path().join("data");
        let documents = root.join("artifacts");
        fs::create_dir_all(&documents).unwrap();
        fs::set_permissions(&root, fs::Permissions::from_mode(0o755)).unwrap();
        let old = documents.join("20261019-0199f0c1-7a2b-7c3d-8e4f-a1b2c3d4e5f6.png");
        fs::write(&old, PNG).unwrap();
        let now = SystemTime::now();
        age(&old, now - 30 * DAY);
        assert!(matches!(
            save_pasted_image(&ImagePasteSettings::under(root.clone()), "w", "image/png", PNG, now),
            Err(ImagePasteError::UnsafeDirectory(path)) if path == root
        ));
        assert!(old.exists());
        assert_eq!(mode(&root), 0o755);
        assert!(!root.join(IMAGE_PASTE_ROOT_MARKER).exists());
        assert!(!root.join("w").exists());
    }
}
