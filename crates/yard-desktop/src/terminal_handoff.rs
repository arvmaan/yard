use std::{
    fs::{self, File, OpenOptions},
    io::{self, Write},
    os::unix::fs::{MetadataExt, OpenOptionsExt, PermissionsExt},
    path::{Path, PathBuf},
    time::{Duration, SystemTime},
};

#[cfg(target_os = "macos")]
use std::process::Command;
#[cfg(target_os = "macos")]
use yard_desktop::terminal_handoff_script;

const DIRECTORY_MODE: u32 = 0o700;
const WRITING_MODE: u32 = 0o600;
const EXECUTABLE_MODE: u32 = 0o700;
const HANDOFF_TTL: Duration = Duration::from_secs(24 * 60 * 60);
const MAX_HANDOFFS: usize = 16;
const MAX_CREATE_ATTEMPTS: usize = 32;
#[cfg(target_os = "macos")]
const ERROR: &str = "could not open the secure Terminal handoff";

#[cfg(target_os = "macos")]
pub(super) fn launch(herdr: &Path, arguments: &[String]) -> Result<(), String> {
    let directory = handoff_directory()?;
    prepare_directory(&directory).map_err(|_| ERROR.to_owned())?;
    cleanup(&directory, SystemTime::now()).map_err(|_| ERROR.to_owned())?;
    let script = terminal_handoff_script(herdr, arguments);
    let path = create_handoff(&directory, script.as_bytes()).map_err(|_| ERROR.to_owned())?;
    match Command::new("/usr/bin/open")
        .args(["-a", "Terminal"])
        .arg(&path)
        .status()
    {
        Ok(status) if status.success() => Ok(()),
        _ => {
            let _ = remove_owned_regular_file(&path);
            Err(ERROR.to_owned())
        }
    }
}

#[cfg(target_os = "macos")]
fn handoff_directory() -> Result<PathBuf, String> {
    let home = std::env::var_os("HOME").ok_or_else(|| ERROR.to_owned())?;
    Ok(PathBuf::from(home).join("Library/Caches/dev.yard.desktop/terminal-handoffs"))
}

fn prepare_directory(path: &Path) -> io::Result<()> {
    let parent = path
        .parent()
        .ok_or_else(|| io::Error::from(io::ErrorKind::InvalidInput))?;
    fs::create_dir_all(parent)?;
    match fs::symlink_metadata(path) {
        Ok(metadata) => validate_directory(path, &metadata),
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            fs::create_dir(path)?;
            fs::set_permissions(path, fs::Permissions::from_mode(DIRECTORY_MODE))?;
            validate_directory(path, &fs::symlink_metadata(path)?)
        }
        Err(error) => Err(error),
    }
}

fn validate_directory(path: &Path, metadata: &fs::Metadata) -> io::Result<()> {
    if !metadata.file_type().is_dir()
        || metadata.file_type().is_symlink()
        || metadata.uid() != rustix::process::getuid().as_raw()
        || metadata.permissions().mode() & 0o777 != DIRECTORY_MODE
    {
        return Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            "unsafe handoff directory",
        ));
    }
    let canonical = path.canonicalize()?;
    if canonical != path {
        return Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            "unexpected handoff directory path",
        ));
    }
    Ok(())
}

fn create_handoff(directory: &Path, contents: &[u8]) -> io::Result<PathBuf> {
    if eligible_handoffs(directory)?.len() >= MAX_HANDOFFS {
        return Err(io::Error::other("handoff limit reached"));
    }
    for attempt in 0..MAX_CREATE_ATTEMPTS {
        let name = format!(
            "handoff-{}-{}-{attempt}.command",
            std::process::id(),
            SystemTime::now()
                .duration_since(SystemTime::UNIX_EPOCH)
                .unwrap_or_default()
                .as_nanos()
        );
        let path = directory.join(name);
        match open_new(&path) {
            Ok(mut file) => {
                let result = (|| {
                    file.write_all(contents)?;
                    file.sync_all()?;
                    file.set_permissions(fs::Permissions::from_mode(EXECUTABLE_MODE))?;
                    file.sync_all()
                })();
                drop(file);
                if let Err(error) = result {
                    let _ = remove_owned_regular_file(&path);
                    return Err(error);
                }
                return Ok(path);
            }
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {}
            Err(error) => return Err(error),
        }
    }
    Err(io::Error::new(
        io::ErrorKind::AlreadyExists,
        "could not reserve handoff file",
    ))
}

fn open_new(path: &Path) -> io::Result<File> {
    let file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(WRITING_MODE)
        .custom_flags(no_follow_flag())
        .open(path)?;
    if file.metadata()?.permissions().mode() & 0o777 != WRITING_MODE {
        return Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            "unsafe handoff file mode",
        ));
    }
    Ok(file)
}

#[cfg(any(target_os = "macos", target_os = "ios", target_os = "freebsd"))]
const fn no_follow_flag() -> i32 {
    0x0000_0100 // O_NOFOLLOW
}

#[cfg(target_os = "linux")]
const fn no_follow_flag() -> i32 {
    0x0002_0000 // O_NOFOLLOW
}

fn cleanup(directory: &Path, now: SystemTime) -> io::Result<()> {
    let mut files = eligible_handoffs(directory)?;
    for (path, modified) in &files {
        if now.duration_since(*modified).unwrap_or_default() >= HANDOFF_TTL {
            remove_owned_regular_file(path)?;
        }
    }
    files = eligible_handoffs(directory)?;
    files.sort_by_key(|(_, modified)| *modified);
    let remove_count = files.len().saturating_sub(MAX_HANDOFFS - 1);
    for (path, _) in files.into_iter().take(remove_count) {
        remove_owned_regular_file(&path)?;
    }
    Ok(())
}

fn eligible_handoffs(directory: &Path) -> io::Result<Vec<(PathBuf, SystemTime)>> {
    let uid = rustix::process::getuid().as_raw();
    let mut files = Vec::new();
    for entry in fs::read_dir(directory)? {
        let entry = entry?;
        let name = entry.file_name();
        let name = name.to_string_lossy();
        if !name.starts_with("handoff-") || !name.ends_with(".command") {
            continue;
        }
        let metadata = fs::symlink_metadata(entry.path())?;
        if metadata.file_type().is_file()
            && !metadata.file_type().is_symlink()
            && metadata.uid() == uid
        {
            files.push((entry.path(), metadata.modified()?));
        }
    }
    Ok(files)
}

fn remove_owned_regular_file(path: &Path) -> io::Result<()> {
    let metadata = fs::symlink_metadata(path)?;
    if metadata.file_type().is_file()
        && !metadata.file_type().is_symlink()
        && metadata.uid() == rustix::process::getuid().as_raw()
    {
        fs::remove_file(path)
    } else {
        Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            "refusing unsafe handoff removal",
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::symlink;

    struct TestDirectory(PathBuf);

    impl TestDirectory {
        fn new(name: &str) -> Self {
            let path = std::env::temp_dir().join(format!(
                "yard-handoff-{name}-{}-{}",
                std::process::id(),
                SystemTime::now()
                    .duration_since(SystemTime::UNIX_EPOCH)
                    .unwrap()
                    .as_nanos()
            ));
            fs::create_dir(&path).unwrap();
            fs::set_permissions(&path, fs::Permissions::from_mode(DIRECTORY_MODE)).unwrap();
            Self(path)
        }
    }

    impl Drop for TestDirectory {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn handoff_is_owner_only_then_executable_and_create_new_refuses_symlink() {
        let temp = TestDirectory::new("create");
        let path = create_handoff(&temp.0, b"#!/bin/sh\nexit 0\n").unwrap();
        let metadata = fs::metadata(&path).unwrap();
        assert_eq!(metadata.permissions().mode() & 0o777, EXECUTABLE_MODE);
        assert_eq!(metadata.uid(), rustix::process::getuid().as_raw());

        let target = temp.0.join("target");
        fs::write(&target, b"untouched").unwrap();
        let link = temp.0.join("collision.command");
        symlink(&target, &link).unwrap();
        assert!(open_new(&link).is_err());
        assert_eq!(fs::read(&target).unwrap(), b"untouched");
    }

    #[test]
    fn directory_refuses_symlink_and_permissive_mode() {
        let temp = TestDirectory::new("directory");
        let actual = temp.0.join("actual");
        fs::create_dir(&actual).unwrap();
        fs::set_permissions(&actual, fs::Permissions::from_mode(DIRECTORY_MODE)).unwrap();
        let link = temp.0.join("link");
        symlink(&actual, &link).unwrap();
        assert!(prepare_directory(&link).is_err());
        fs::set_permissions(&actual, fs::Permissions::from_mode(0o755)).unwrap();
        assert!(prepare_directory(&actual).is_err());
    }

    #[test]
    fn cleanup_removes_expired_owned_regular_handoff() {
        let temp = TestDirectory::new("ttl");
        let stale = temp.0.join("handoff-stale.command");
        fs::write(&stale, b"x").unwrap();
        cleanup(&temp.0, SystemTime::now() + HANDOFF_TTL).unwrap();
        assert!(!stale.exists());
    }

    #[test]
    fn cleanup_removes_only_owned_regular_handoff_files_and_bounds_count() {
        let temp = TestDirectory::new("cleanup");
        for index in 0..MAX_HANDOFFS + 2 {
            fs::write(temp.0.join(format!("handoff-{index}.command")), b"x").unwrap();
        }
        fs::write(temp.0.join("unrelated.txt"), b"keep").unwrap();
        let target = temp.0.join("target");
        fs::write(&target, b"keep").unwrap();
        symlink(&target, temp.0.join("handoff-link.command")).unwrap();

        cleanup(&temp.0, SystemTime::now()).unwrap();
        assert_eq!(eligible_handoffs(&temp.0).unwrap().len(), MAX_HANDOFFS - 1);
        assert!(temp.0.join("unrelated.txt").exists());
        assert!(temp.0.join("handoff-link.command").exists());
        assert_eq!(fs::read(target).unwrap(), b"keep");
    }
}
