use std::{
    ffi::{CStr, CString, OsStr},
    fs::File,
    io::{self, Write},
    os::{fd::OwnedFd, unix::ffi::OsStrExt},
    path::{Path, PathBuf},
    time::{Duration, SystemTime},
};

use rustix::fs::{self, AtFlags, FileType, Mode, OFlags};
#[cfg(target_os = "macos")]
use std::process::Command;
#[cfg(target_os = "macos")]
use yard_desktop::terminal_handoff_script;

const DIRECTORY_MODE: Mode = Mode::RUSR.union(Mode::WUSR).union(Mode::XUSR);
const WRITING_MODE: Mode = Mode::RUSR.union(Mode::WUSR);
const EXECUTABLE_MODE: Mode = DIRECTORY_MODE;
const HANDOFF_TTL: Duration = Duration::from_secs(24 * 60 * 60);
const MAX_HANDOFFS: usize = 16;
const MAX_CREATE_ATTEMPTS: usize = 32;
const DIRECTORY_FLAGS: OFlags = OFlags::RDONLY
    .union(OFlags::DIRECTORY)
    .union(OFlags::NOFOLLOW)
    .union(OFlags::CLOEXEC);
const FILE_FLAGS: OFlags = OFlags::WRONLY
    .union(OFlags::CREATE)
    .union(OFlags::EXCL)
    .union(OFlags::NOFOLLOW)
    .union(OFlags::CLOEXEC);
#[cfg(target_os = "macos")]
const ERROR: &str = "could not open the secure Terminal handoff";

struct HandoffDirectory {
    fd: OwnedFd,
    path: PathBuf,
}

struct CreatedHandoff {
    name: CString,
    path: PathBuf,
    identity: FileIdentity,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct FileIdentity {
    device: u64,
    inode: u64,
    uid: u32,
    links: u64,
}

#[cfg(target_os = "macos")]
pub(super) fn launch(herdr: &Path, arguments: &[String]) -> Result<(), String> {
    let directory = open_handoff_directory_from_home().map_err(|_| ERROR.to_owned())?;
    cleanup(&directory, SystemTime::now()).map_err(|_| ERROR.to_owned())?;
    let script = terminal_handoff_script(herdr, arguments);
    let handoff = create_handoff(&directory, script.as_bytes()).map_err(|_| ERROR.to_owned())?;
    if verify_created_handoff(&directory, &handoff).is_err() {
        let _ = remove_if_identity_matches(&directory, &handoff.name, handoff.identity);
        return Err(ERROR.to_owned());
    }
    match Command::new("/usr/bin/open")
        .args(["-a", "Terminal"])
        .arg(&handoff.path)
        .status()
    {
        Ok(status) if status.success() => Ok(()),
        _ => {
            let _ = remove_if_identity_matches(&directory, &handoff.name, handoff.identity);
            Err(ERROR.to_owned())
        }
    }
}

#[cfg(target_os = "macos")]
fn open_handoff_directory_from_home() -> io::Result<HandoffDirectory> {
    let home = std::env::var_os("HOME").ok_or_else(|| io::Error::from(io::ErrorKind::NotFound))?;
    open_handoff_directory(Path::new(&home))
}

fn open_handoff_directory(home: &Path) -> io::Result<HandoffDirectory> {
    if !home.is_absolute() {
        return Err(io::Error::from(io::ErrorKind::InvalidInput));
    }
    let mut fd = fs::openat(fs::CWD, home, DIRECTORY_FLAGS, Mode::empty())?;
    validate_directory_fd(&fd)?;
    let mut path = home.to_owned();
    for component in ["Library", "Caches", "dev.yard.desktop", "terminal-handoffs"] {
        let name = CStr::from_bytes_with_nul(match component {
            "Library" => b"Library\0",
            "Caches" => b"Caches\0",
            "dev.yard.desktop" => b"dev.yard.desktop\0",
            _ => b"terminal-handoffs\0",
        })
        .expect("static directory component is valid");
        fd = open_or_create_directory(&fd, name)?;
        validate_directory_fd(&fd)?;
        path.push(component);
    }
    Ok(HandoffDirectory { fd, path })
}

fn open_or_create_directory(parent: &OwnedFd, name: &CStr) -> io::Result<OwnedFd> {
    match fs::openat(parent, name, DIRECTORY_FLAGS, Mode::empty()) {
        Ok(fd) => Ok(fd),
        Err(rustix::io::Errno::NOENT) => {
            match fs::mkdirat(parent, name, DIRECTORY_MODE) {
                Ok(()) | Err(rustix::io::Errno::EXIST) => {}
                Err(error) => return Err(error.into()),
            }
            fs::openat(parent, name, DIRECTORY_FLAGS, Mode::empty()).map_err(Into::into)
        }
        Err(error) => Err(error.into()),
    }
}

fn validate_directory_fd(fd: &OwnedFd) -> io::Result<()> {
    let stat = fs::fstat(fd)?;
    if !directory_stat_is_safe(&stat, rustix::process::getuid().as_raw()) {
        return Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            "unsafe handoff directory ancestry",
        ));
    }
    Ok(())
}

fn directory_stat_is_safe(stat: &fs::Stat, uid: u32) -> bool {
    FileType::from_raw_mode(stat.st_mode).is_dir()
        && stat.st_uid == uid
        && !Mode::from_raw_mode(stat.st_mode).intersects(Mode::WGRP | Mode::WOTH)
}

fn create_handoff(directory: &HandoffDirectory, contents: &[u8]) -> io::Result<CreatedHandoff> {
    if eligible_handoffs(directory)?.len() >= MAX_HANDOFFS {
        return Err(io::Error::other("handoff limit reached"));
    }
    for attempt in 0..MAX_CREATE_ATTEMPTS {
        let name = CString::new(format!(
            "handoff-{}-{}-{attempt}.command",
            std::process::id(),
            SystemTime::now()
                .duration_since(SystemTime::UNIX_EPOCH)
                .unwrap_or_default()
                .as_nanos()
        ))
        .expect("generated handoff name has no NUL");
        match fs::openat(&directory.fd, name.as_c_str(), FILE_FLAGS, WRITING_MODE) {
            Ok(fd) => {
                let mut file = File::from(fd);
                let opened_stat = fs::fstat(&file)?;
                validate_file_stat(&opened_stat, WRITING_MODE)?;
                let opened_identity = FileIdentity::from_stat(&opened_stat);
                let result = (|| {
                    file.write_all(contents)?;
                    fs::fsync(&file)?;
                    fs::fchmod(&file, EXECUTABLE_MODE)?;
                    fs::fsync(&file)?;
                    let stat = fs::fstat(&file)?;
                    validate_file_stat(&stat, EXECUTABLE_MODE)?;
                    let identity = FileIdentity::from_stat(&stat);
                    if identity != opened_identity {
                        return Err(io::Error::new(
                            io::ErrorKind::PermissionDenied,
                            "handoff descriptor identity changed",
                        ));
                    }
                    Ok(identity)
                })();
                drop(file);
                match result {
                    Ok(identity) => {
                        return Ok(CreatedHandoff {
                            path: directory.path.join(OsStr::from_bytes(name.as_bytes())),
                            name,
                            identity,
                        });
                    }
                    Err(error) => {
                        let _ = remove_if_identity_matches(directory, &name, opened_identity);
                        return Err(error);
                    }
                }
            }
            Err(rustix::io::Errno::EXIST) => {}
            Err(error) => return Err(error.into()),
        }
    }
    Err(io::Error::new(
        io::ErrorKind::AlreadyExists,
        "could not reserve handoff file",
    ))
}

impl FileIdentity {
    fn from_stat(stat: &fs::Stat) -> Self {
        Self {
            device: stat.st_dev,
            inode: stat.st_ino,
            uid: stat.st_uid,
            links: stat.st_nlink,
        }
    }

    fn matches(self, stat: &fs::Stat) -> bool {
        self == Self::from_stat(stat)
    }
}

fn validate_file_stat(stat: &fs::Stat, expected_mode: Mode) -> io::Result<()> {
    if !FileType::from_raw_mode(stat.st_mode).is_file()
        || stat.st_uid != rustix::process::getuid().as_raw()
        || stat.st_nlink != 1
        || Mode::from_raw_mode(stat.st_mode) != expected_mode
    {
        return Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            "unsafe handoff file identity",
        ));
    }
    Ok(())
}

fn verify_created_handoff(
    directory: &HandoffDirectory,
    handoff: &CreatedHandoff,
) -> io::Result<()> {
    validate_directory_fd(&directory.fd)?;
    let stat = fs::statat(
        &directory.fd,
        handoff.name.as_c_str(),
        AtFlags::SYMLINK_NOFOLLOW,
    )?;
    validate_file_stat(&stat, EXECUTABLE_MODE)?;
    if !handoff.identity.matches(&stat) {
        return Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            "handoff pathname identity changed",
        ));
    }
    let path_stat = fs::lstat(&handoff.path)?;
    validate_file_stat(&path_stat, EXECUTABLE_MODE)?;
    if !handoff.identity.matches(&path_stat) {
        return Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            "handoff launch path identity changed",
        ));
    }
    Ok(())
}

fn cleanup(directory: &HandoffDirectory, now: SystemTime) -> io::Result<()> {
    let mut files = eligible_handoffs(directory)?;
    for file in &files {
        if file.expired(now) {
            remove_if_identity_matches(directory, &file.name, file.identity)?;
        }
    }
    files = eligible_handoffs(directory)?;
    files.sort_by_key(|file| file.modified_seconds);
    let remove_count = files.len().saturating_sub(MAX_HANDOFFS - 1);
    for file in files.into_iter().take(remove_count) {
        remove_if_identity_matches(directory, &file.name, file.identity)?;
    }
    Ok(())
}

struct ExistingHandoff {
    name: CString,
    identity: FileIdentity,
    modified_seconds: i64,
}

impl ExistingHandoff {
    fn expired(&self, now: SystemTime) -> bool {
        let modified = if self.modified_seconds >= 0 {
            SystemTime::UNIX_EPOCH + Duration::from_secs(self.modified_seconds.unsigned_abs())
        } else {
            SystemTime::UNIX_EPOCH
        };
        now.duration_since(modified).unwrap_or_default() >= HANDOFF_TTL
    }
}

fn eligible_handoffs(directory: &HandoffDirectory) -> io::Result<Vec<ExistingHandoff>> {
    validate_directory_fd(&directory.fd)?;
    let uid = rustix::process::getuid().as_raw();
    let mut files = Vec::new();
    for entry in fs::Dir::read_from(&directory.fd)? {
        let entry = entry?;
        let name = entry.file_name();
        let bytes = name.to_bytes();
        if !bytes.starts_with(b"handoff-") || !bytes.ends_with(b".command") {
            continue;
        }
        let stat = fs::statat(&directory.fd, name, AtFlags::SYMLINK_NOFOLLOW)?;
        if cleanup_stat_is_eligible(&stat, uid) {
            files.push(ExistingHandoff {
                name: name.to_owned(),
                identity: FileIdentity::from_stat(&stat),
                modified_seconds: stat.st_mtime,
            });
        }
    }
    Ok(files)
}

fn cleanup_stat_is_eligible(stat: &fs::Stat, uid: u32) -> bool {
    FileType::from_raw_mode(stat.st_mode).is_file() && stat.st_uid == uid && stat.st_nlink == 1
}

fn remove_if_identity_matches(
    directory: &HandoffDirectory,
    name: &CStr,
    identity: FileIdentity,
) -> io::Result<()> {
    validate_directory_fd(&directory.fd)?;
    let stat = fs::statat(&directory.fd, name, AtFlags::SYMLINK_NOFOLLOW)?;
    if !FileType::from_raw_mode(stat.st_mode).is_file()
        || stat.st_uid != rustix::process::getuid().as_raw()
        || stat.st_nlink != 1
        || !identity.matches(&stat)
    {
        return Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            "refusing changed handoff removal",
        ));
    }
    remove_entry(&directory.fd, name)
}

fn remove_entry(directory: &OwnedFd, name: &CStr) -> io::Result<()> {
    fs::unlinkat(directory, name, AtFlags::empty()).map_err(Into::into)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{fs as stdfs, os::unix::fs::symlink};

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
            stdfs::create_dir(&path).unwrap();
            stdfs::set_permissions(
                &path,
                <stdfs::Permissions as std::os::unix::fs::PermissionsExt>::from_mode(0o700),
            )
            .unwrap();
            Self(path)
        }

        fn handoffs(&self) -> HandoffDirectory {
            open_handoff_directory(&self.0).unwrap()
        }
    }

    impl Drop for TestDirectory {
        fn drop(&mut self) {
            let _ = stdfs::remove_dir_all(&self.0);
        }
    }

    fn cstring(value: &str) -> CString {
        CString::new(value).unwrap()
    }

    #[test]
    fn ancestry_is_private_owned_directories_and_refuses_symlinks_or_writable_parent() {
        let temp = TestDirectory::new("ancestry");
        let handoffs = temp.handoffs();
        for component in [
            temp.0.clone(),
            temp.0.join("Library"),
            temp.0.join("Library/Caches"),
            temp.0.join("Library/Caches/dev.yard.desktop"),
            handoffs.path.clone(),
        ] {
            let metadata = stdfs::symlink_metadata(component).unwrap();
            assert!(metadata.is_dir());
            assert_eq!(
                std::os::unix::fs::PermissionsExt::mode(&metadata.permissions()) & 0o022,
                0
            );
        }

        let unsafe_home = temp.0.join("unsafe-home");
        stdfs::create_dir(&unsafe_home).unwrap();
        stdfs::set_permissions(
            &unsafe_home,
            <stdfs::Permissions as std::os::unix::fs::PermissionsExt>::from_mode(0o777),
        )
        .unwrap();
        assert!(open_handoff_directory(&unsafe_home).is_err());

        let linked_home = temp.0.join("linked-home");
        symlink(&temp.0, &linked_home).unwrap();
        assert!(open_handoff_directory(&linked_home).is_err());
    }

    #[test]
    fn create_is_owner_only_descriptor_relative_and_refuses_collision_symlink() {
        let temp = TestDirectory::new("create");
        let directory = temp.handoffs();
        let handoff = create_handoff(&directory, b"#!/bin/sh\nexit 0\n").unwrap();
        verify_created_handoff(&directory, &handoff).unwrap();
        let stat = fs::statat(
            &directory.fd,
            handoff.name.as_c_str(),
            AtFlags::SYMLINK_NOFOLLOW,
        )
        .unwrap();
        assert_eq!(Mode::from_raw_mode(stat.st_mode), EXECUTABLE_MODE);
        assert_eq!(stat.st_uid, rustix::process::getuid().as_raw());
        assert_eq!(stat.st_nlink, 1);

        let target = temp.0.join("target");
        stdfs::write(&target, b"untouched").unwrap();
        let link_name = cstring("handoff-collision.command");
        symlink(&target, directory.path.join("handoff-collision.command")).unwrap();
        assert!(fs::openat(&directory.fd, &link_name, FILE_FLAGS, WRITING_MODE).is_err());
        assert_eq!(stdfs::read(target).unwrap(), b"untouched");
    }

    #[test]
    fn pathname_replacement_and_hardlink_identity_are_refused() {
        let temp = TestDirectory::new("identity");
        let directory = temp.handoffs();
        let handoff = create_handoff(&directory, b"one").unwrap();
        let replacement = directory.path.join("replacement");
        stdfs::write(&replacement, b"two").unwrap();
        stdfs::rename(&replacement, &handoff.path).unwrap();
        assert!(verify_created_handoff(&directory, &handoff).is_err());
        assert!(remove_if_identity_matches(&directory, &handoff.name, handoff.identity).is_err());
        assert_eq!(stdfs::read(&handoff.path).unwrap(), b"two");

        let hardlink_name = cstring("handoff-hardlink.command");
        let hardlink_path = directory.path.join("handoff-hardlink.command");
        stdfs::hard_link(&handoff.path, &hardlink_path).unwrap();
        let stat = fs::statat(
            &directory.fd,
            hardlink_name.as_c_str(),
            AtFlags::SYMLINK_NOFOLLOW,
        )
        .unwrap();
        assert_eq!(stat.st_nlink, 2);
        assert!(
            remove_if_identity_matches(&directory, &hardlink_name, FileIdentity::from_stat(&stat))
                .is_err()
        );
    }

    #[test]
    fn retained_directory_fd_does_not_follow_replaced_ancestor() {
        let temp = TestDirectory::new("ancestor-replacement");
        let directory = temp.handoffs();
        let original = temp.0.join("Library");
        let moved = temp.0.join("Library-moved");
        stdfs::rename(&original, &moved).unwrap();
        stdfs::create_dir(&original).unwrap();
        let handoff = create_handoff(&directory, b"safe").unwrap();
        assert!(
            moved
                .join("Caches/dev.yard.desktop/terminal-handoffs")
                .join(OsStr::from_bytes(handoff.name.as_bytes()))
                .exists()
        );
        assert!(!handoff.path.exists());
        assert!(verify_created_handoff(&directory, &handoff).is_err());
    }

    #[test]
    fn cleanup_removes_ttl_and_bounds_only_eligible_files() {
        let temp = TestDirectory::new("cleanup");
        let directory = temp.handoffs();
        let now = SystemTime::now();
        let stale = create_handoff(&directory, b"stale").unwrap();
        cleanup(&directory, now + HANDOFF_TTL).unwrap();
        assert!(!stale.path.exists());

        for index in 0..MAX_HANDOFFS + 2 {
            let name = format!("handoff-{index}.command");
            stdfs::write(directory.path.join(name), b"x").unwrap();
        }
        stdfs::write(directory.path.join("unrelated.txt"), b"keep").unwrap();
        let target = temp.0.join("target");
        stdfs::write(&target, b"keep").unwrap();
        symlink(&target, directory.path.join("handoff-link.command")).unwrap();
        let hardlink = directory.path.join("handoff-hard.command");
        stdfs::hard_link(&target, &hardlink).unwrap();

        cleanup(&directory, SystemTime::now()).unwrap();
        assert_eq!(
            eligible_handoffs(&directory).unwrap().len(),
            MAX_HANDOFFS - 1
        );
        assert!(directory.path.join("unrelated.txt").exists());
        assert!(directory.path.join("handoff-link.command").exists());
        assert!(hardlink.exists());
        assert_eq!(stdfs::read(target).unwrap(), b"keep");
    }

    #[test]
    fn stat_predicates_reject_foreign_owner_wrong_type_links_and_writable_directories() {
        let temp = TestDirectory::new("predicates");
        let directory = temp.handoffs();
        let handoff = create_handoff(&directory, b"safe").unwrap();
        let file_stat = fs::statat(
            &directory.fd,
            handoff.name.as_c_str(),
            AtFlags::SYMLINK_NOFOLLOW,
        )
        .unwrap();
        let uid = rustix::process::getuid().as_raw();
        assert!(cleanup_stat_is_eligible(&file_stat, uid));
        assert!(!cleanup_stat_is_eligible(&file_stat, uid.wrapping_add(1)));

        let directory_stat = fs::fstat(&directory.fd).unwrap();
        assert!(directory_stat_is_safe(&directory_stat, uid));
        assert!(!directory_stat_is_safe(
            &directory_stat,
            uid.wrapping_add(1)
        ));
        assert!(!directory_stat_is_safe(&file_stat, uid));
    }

    #[test]
    fn identity_fields_include_owner_type_link_count_device_and_inode() {
        let temp = TestDirectory::new("metadata");
        let directory = temp.handoffs();
        let handoff = create_handoff(&directory, b"safe").unwrap();
        let stat = fs::statat(
            &directory.fd,
            handoff.name.as_c_str(),
            AtFlags::SYMLINK_NOFOLLOW,
        )
        .unwrap();
        assert!(FileType::from_raw_mode(stat.st_mode).is_file());
        assert_eq!(handoff.identity, FileIdentity::from_stat(&stat));
    }
}
