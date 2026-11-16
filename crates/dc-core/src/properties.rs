//! What Linux keeps about a file, and what of it can be changed: the
//! permissions (`chmod`), the owner and group (`chown`), the times
//! (`touch`). Read through `statx` (also the time of birth), the flags of
//! the file system as `lsattr` shows them, the extended attributes.
//!
//! Windows' "read-only", "hidden", "archive" do not exist here: hidden is
//! a name starting with a dot, read-only is a missing `w`.

use std::ffi::OsString;
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicBool;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use dc_i18n::gettext;
use rustix::fs::{AtFlags, IFlags, Mode, OFlags, StatxFlags, StatxTimestamp};

use crate::program;

/// The bits `chmod` changes: permissions, setuid, setgid, sticky.
pub const PERMISSIONS: u32 = 0o7777;
/// Execute (search, for directories) for owner, group and others.
pub const EXECUTE: u32 = 0o111;

/// A change of permissions for many items at once: bits to set, bits to
/// clear, the rest stays as each item has it.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct ModeChange {
    pub set: u32,
    pub clear: u32,
    /// Execute bits go only to directories and to files that are
    /// executable already (`chmod X`): a whole tree made `+x` does not
    /// turn every document into a program.
    pub execute_only_where_present: bool,
}

impl ModeChange {
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.set == 0 && self.clear == 0
    }

    /// The new permissions of an item with `mode`.
    #[must_use]
    pub fn apply(&self, mode: u32, is_dir: bool) -> u32 {
        let old = mode & PERMISSIONS;
        let mut set = self.set;
        if self.execute_only_where_present && !is_dir && old & EXECUTE == 0 {
            set &= !EXECUTE;
        }
        (old & !self.clear) | set
    }
}

/// Whether `bit` is set in all `modes` (`Some(true)`), in none
/// (`Some(false)`) or in some (`None`).
#[must_use]
pub fn bit_state(modes: &[u32], bit: u32) -> Option<bool> {
    let set = modes.iter().filter(|mode| *mode & bit != 0).count();
    match set {
        0 => Some(false),
        all if all == modes.len() => Some(true),
        _ => None,
    }
}

/// `0755`, as `chmod` takes it.
#[must_use]
pub fn octal(mode: u32) -> String {
    format!("{:04o}", mode & PERMISSIONS)
}

/// Permissions written in octal (`755`, `0644`, `4755`).
#[must_use]
pub fn parse_octal(text: &str) -> Option<u32> {
    let text = text.trim();
    if text.is_empty() || text.len() > 4 {
        return None;
    }
    u32::from_str_radix(text, 8).ok().filter(|mode| *mode <= PERMISSIONS)
}

/// A user or a group.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Account {
    pub id: u32,
    pub name: String,
}

/// Lines of `/etc/passwd` or `/etc/group` (`name:x:id:...`), sorted by name.
#[must_use]
pub fn parse_accounts(text: &str) -> Vec<Account> {
    let mut accounts: Vec<Account> = text
        .lines()
        .filter_map(|line| {
            let mut fields = line.split(':');
            let name = fields.next()?.to_owned();
            let id = fields.nth(1)?.parse().ok()?;
            (!name.is_empty()).then_some(Account { id, name })
        })
        .collect();
    accounts.sort_by(|a, b| a.name.cmp(&b.name));
    accounts.dedup_by_key(|account| account.id);
    accounts
}

/// The users (`passwd`) or groups (`group`) of the system as `getent`
/// lists them: the local files and whatever else NSS knows (LDAP, ...).
#[must_use]
pub fn accounts(database: &str) -> Vec<Account> {
    let output = program::find("getent").and_then(|getent| {
        program::run(&getent, &[database.into()], None, Duration::from_secs(5), 8 << 20, &AtomicBool::new(false))
    });
    match output {
        Some(output) => parse_accounts(&String::from_utf8_lossy(&output)),
        None => parse_accounts(&fs::read_to_string(format!("/etc/{database}")).unwrap_or_default()),
    }
}

/// What `statx` tells about one item, a link not followed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Details {
    /// Type and permissions (`st_mode`).
    pub mode: u32,
    pub uid: u32,
    pub gid: u32,
    pub size: u64,
    /// Bytes the item takes on the disk.
    pub on_disk: u64,
    pub links: u32,
    pub inode: u64,
    /// `major:minor` of the device the item is on.
    pub device: (u32, u32),
    pub modified: SystemTime,
    pub accessed: SystemTime,
    /// The last change of the metadata (`ctime`).
    pub changed: SystemTime,
    /// The birth, where the file system keeps it.
    pub born: Option<SystemTime>,
    /// Where a symbolic link points, and whether it leads anywhere.
    pub link: Option<(PathBuf, bool)>,
}

fn time(stamp: StatxTimestamp) -> SystemTime {
    let nanos = Duration::new(stamp.tv_sec.unsigned_abs(), stamp.tv_nsec);
    if stamp.tv_sec >= 0 { UNIX_EPOCH + nanos } else { UNIX_EPOCH - nanos }
}

impl Details {
    /// # Errors
    /// When the item cannot be inspected.
    pub fn read(path: &Path) -> std::io::Result<Self> {
        let stat = rustix::fs::statx(rustix::fs::CWD, path, AtFlags::SYMLINK_NOFOLLOW, StatxFlags::BASIC_STATS | StatxFlags::BTIME)?;
        let mode = u32::from(stat.stx_mode);
        let link = (mode & 0o170_000 == 0o120_000)
            .then(|| fs::read_link(path).ok())
            .flatten()
            .map(|target| (target, fs::metadata(path).is_ok()));
        Ok(Self {
            mode,
            uid: stat.stx_uid,
            gid: stat.stx_gid,
            size: stat.stx_size,
            on_disk: stat.stx_blocks * 512,
            links: stat.stx_nlink,
            inode: stat.stx_ino,
            device: (stat.stx_dev_major, stat.stx_dev_minor),
            modified: time(stat.stx_mtime),
            accessed: time(stat.stx_atime),
            changed: time(stat.stx_ctime),
            born: (stat.stx_mask & StatxFlags::BTIME.bits() != 0).then(|| time(stat.stx_btime)),
            link,
        })
    }

    #[must_use]
    pub fn is_dir(&self) -> bool {
        self.mode & 0o170_000 == 0o040_000
    }

    #[must_use]
    pub fn is_link(&self) -> bool {
        self.mode & 0o170_000 == 0o120_000
    }
}

/// The flags of the file system an item has (`lsattr`), each with its
/// letter and what it means; `None` where the file system has none (or
/// for links and special files, which cannot be opened for it).
#[must_use]
pub fn fs_flags(path: &Path) -> Option<Vec<(char, String)>> {
    let file = rustix::fs::open(path, OFlags::RDONLY | OFlags::NONBLOCK | OFlags::NOFOLLOW | OFlags::CLOEXEC, Mode::empty()).ok()?;
    let flags = rustix::fs::ioctl_getflags(&file).ok()?;
    let known = [
        (IFlags::IMMUTABLE, 'i', gettext("immutable: cannot be changed, deleted or renamed, not even by root")),
        (IFlags::APPEND, 'a', gettext("append only")),
        (IFlags::NOCOW, 'C', gettext("no copy on write")),
        (IFlags::COMPRESSED, 'c', gettext("compressed by the file system")),
        (IFlags::NODUMP, 'd', gettext("left out of backups by dump")),
        (IFlags::NOATIME, 'A', gettext("time of access not updated")),
        (IFlags::SYNC, 'S', gettext("written synchronously")),
        (IFlags::DIRSYNC, 'D', gettext("directory changes written synchronously")),
        (IFlags::JOURNALING, 'j', gettext("data journaled")),
        (IFlags::PROJECT_INHERIT, 'P', gettext("project inherited")),
        (IFlags::TOPDIR, 'T', gettext("top of a directory hierarchy")),
    ];
    Some(known.into_iter().filter(|(flag, ..)| flags.contains(*flag)).map(|(_, letter, meaning)| (letter, meaning)).collect())
}

/// The extended attributes of an item (a link itself, not followed),
/// with their values.
#[must_use]
pub fn xattrs(path: &Path) -> Vec<(String, Vec<u8>)> {
    let mut names = vec![0_u8; 64 * 1024];
    let Ok(length) = rustix::fs::llistxattr(path, &mut names[..]) else { return Vec::new() };
    names[..length]
        .split(|byte| *byte == 0)
        .filter(|name| !name.is_empty())
        .map(|name| {
            let name = String::from_utf8_lossy(name).into_owned();
            let mut value = vec![0_u8; 64 * 1024];
            let value = match rustix::fs::lgetxattr(path, name.as_str(), &mut value[..]) {
                Ok(length) => value[..length].to_vec(),
                Err(_) => Vec::new(),
            };
            (name, value)
        })
        .collect()
}

/// A value of an extended attribute for the eye: text when it is text,
/// else its size.
#[must_use]
pub fn xattr_text(value: &[u8]) -> String {
    let text = value.strip_suffix(&[0]).unwrap_or(value);
    match std::str::from_utf8(text) {
        Ok(text) if text.chars().all(|ch| !ch.is_control() || ch == '\n' || ch == '\t') => text.to_owned(),
        _ => dc_i18n::fill(&gettext("{count} bytes of binary data"), &[("count", &value.len())]),
    }
}

/// What a set of items holds, directories counted through.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Content {
    pub files: u64,
    pub dirs: u64,
    pub links: u64,
    pub bytes: u64,
    pub on_disk: u64,
}

/// Counts `paths` with everything inside their directories, links not
/// followed (as `du`); stops early when `cancel` is raised.
#[must_use]
pub fn content(paths: &[PathBuf], cancel: &AtomicBool) -> Content {
    use std::os::unix::fs::MetadataExt;
    let mut content = Content::default();
    let mut pending: Vec<PathBuf> = paths.to_vec();
    while let Some(path) = pending.pop() {
        if cancel.load(std::sync::atomic::Ordering::Relaxed) {
            break;
        }
        let Ok(metadata) = fs::symlink_metadata(&path) else { continue };
        content.on_disk += metadata.blocks() * 512;
        if metadata.is_symlink() {
            content.links += 1;
        } else if metadata.is_dir() {
            content.dirs += 1;
            if let Ok(entries) = fs::read_dir(&path) {
                pending.extend(entries.flatten().map(|entry| entry.path()));
            }
        } else {
            content.files += 1;
            content.bytes += metadata.len();
        }
    }
    content
}

/// The mount the item at `path` is on: where, the device and the type of
/// its file system (the longest mount point leading to it).
#[must_use]
pub fn mount_of(path: &Path) -> Option<(PathBuf, String, String)> {
    let path = fs::canonicalize(path).unwrap_or_else(|_| path.to_owned());
    let mounts = fs::read_to_string("/proc/self/mounts").ok()?;
    mounts
        .lines()
        .filter_map(|line| {
            let mut fields = line.split_whitespace();
            let device = fields.next()?;
            let point = PathBuf::from(fields.next()?.replace("\\040", " "));
            let fs_type = fields.next()?;
            path.starts_with(&point).then(|| (point, device.to_owned(), fs_type.to_owned()))
        })
        .max_by_key(|(point, ..)| point.as_os_str().len())
}

/// Whether the permissions of `path` (not followed) are those of `mode`.
#[must_use]
pub fn has_mode(path: &Path, mode: u32) -> bool {
    fs::symlink_metadata(path).is_ok_and(|metadata| metadata.permissions().mode() & PERMISSIONS == mode & PERMISSIONS)
}

/// The name of `path` for a title (`/` for the root).
#[must_use]
pub fn display_name(path: &Path) -> OsString {
    path.file_name().map_or_else(|| path.as_os_str().to_owned(), ToOwned::to_owned)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mode_changes_keep_what_they_do_not_name() {
        let change = ModeChange { set: 0o020, clear: 0o002, execute_only_where_present: false };
        assert_eq!(change.apply(0o100_646, false), 0o664);
        assert!(!change.is_empty());
        let executable = ModeChange { set: 0o111, clear: 0, execute_only_where_present: true };
        // chmod X: a document stays a document, a script and a directory get it.
        assert_eq!(executable.apply(0o100_644, false), 0o644);
        assert_eq!(executable.apply(0o100_744, false), 0o755);
        assert_eq!(executable.apply(0o040_700, true), 0o711);
        assert_eq!(ModeChange { set: 0o4000, ..ModeChange::default() }.apply(0o755, false), 0o4755);
    }

    #[test]
    fn bits_of_many_items() {
        assert_eq!(bit_state(&[0o644, 0o640], 0o400), Some(true));
        assert_eq!(bit_state(&[0o644, 0o640], 0o004), None);
        assert_eq!(bit_state(&[0o644, 0o640], 0o100), Some(false));
    }

    #[test]
    fn octal_both_ways() {
        assert_eq!(octal(0o100_755), "0755");
        assert_eq!(octal(0o4755), "4755");
        assert_eq!(parse_octal(" 644 "), Some(0o644));
        assert_eq!(parse_octal("0750"), Some(0o750));
        assert_eq!(parse_octal("7777"), Some(0o7777));
        assert_eq!(parse_octal("8"), None);
        assert_eq!(parse_octal("17777"), None);
        assert_eq!(parse_octal(""), None);
    }

    #[test]
    fn accounts_of_getent() {
        let text = "root:x:0:0:root:/root:/bin/bash\njirka:x:1000:1000::/home/jirka:/bin/bash\nbroken\nnobody:x:65534:65534:::\n";
        let parsed = parse_accounts(text);
        let names: Vec<&str> = parsed.iter().map(|account| account.name.as_str()).collect();
        assert_eq!(names, ["jirka", "nobody", "root"]);
        assert_eq!(parsed[0].id, 1000);
        assert!(accounts("passwd").iter().any(|account| account.id == 0));
    }

    #[test]
    fn details_of_files_and_links() {
        let tmp = tempfile::tempdir().unwrap();
        let file = tmp.path().join("a.txt");
        fs::write(&file, "hello").unwrap();
        fs::set_permissions(&file, fs::Permissions::from_mode(0o640)).unwrap();
        let details = Details::read(&file).unwrap();
        assert_eq!(details.size, 5);
        assert_eq!(details.mode & PERMISSIONS, 0o640);
        assert_eq!(details.links, 1);
        assert!(!details.is_dir() && !details.is_link());
        assert!(has_mode(&file, 0o640));
        std::os::unix::fs::symlink("nowhere", tmp.path().join("broken")).unwrap();
        let link = Details::read(&tmp.path().join("broken")).unwrap();
        assert!(link.is_link());
        assert_eq!(link.link, Some((PathBuf::from("nowhere"), false)));
        assert!(Details::read(tmp.path()).unwrap().is_dir());
    }

    #[test]
    fn content_of_a_tree() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().join("d");
        fs::create_dir_all(root.join("sub")).unwrap();
        fs::write(root.join("a"), "12345").unwrap();
        fs::write(root.join("sub/b"), "123").unwrap();
        std::os::unix::fs::symlink("a", root.join("link")).unwrap();
        let counted = content(std::slice::from_ref(&root), &AtomicBool::new(false));
        assert_eq!((counted.files, counted.dirs, counted.links, counted.bytes), (2, 2, 1, 8));
        assert_eq!(mount_of(&root).map(|(point, ..)| point.is_absolute()), Some(true));
    }

    #[test]
    fn values_of_extended_attributes() {
        assert_eq!(xattr_text(b"text/plain\0"), "text/plain");
        assert!(xattr_text(&[0x01, 0x02, 0xFF]).contains('3'));
    }
}
