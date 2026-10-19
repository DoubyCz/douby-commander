//! Mounted file systems for the drive bar, and free space.
//!
//! Follows `CheckMountEntry` in `platform/udrivewatcher.pas` of the
//! original: system and virtual mounts are left out, the root too (it has
//! its own button). The original also asks `UDisks` (hides e.g. the EFI
//! partition) and lists fstab entries that are not mounted; neither is
//! done here yet.

use std::fs;
use std::path::{Path, PathBuf};

/// Icon and behaviour class of a drive.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DriveKind {
    HardDisk,
    Removable,
    Optical,
    Network,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MountPoint {
    pub path: PathBuf,
    pub device: String,
    pub fs_type: String,
    pub kind: DriveKind,
}

impl MountPoint {
    /// Button caption: the last part of the path (`/var/log` is `log`).
    #[must_use]
    pub fn display_name(&self) -> String {
        self.path.file_name().map_or_else(|| self.path.display().to_string(), |name| name.to_string_lossy().into_owned())
    }
}

/// Free and total bytes of the file system holding a path.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FreeSpace {
    pub free: u64,
    pub total: u64,
}

/// Mount points worth showing, sorted by their name as the drive bar of the
/// original shows them.
#[must_use]
pub fn mount_points() -> Vec<MountPoint> {
    let read = |file| fs::read_to_string(file).unwrap_or_default();
    let mut mounts = parse_mounts(&read("/proc/self/mounts"));
    resolve_automounts(&mut mounts, &parse_mounts(&read("/etc/fstab")));
    mounts.sort_by_cached_key(|mount| mount.display_name().to_lowercase());
    mounts
}

/// A systemd automount shows as `autofs` until first used; its real device
/// and type are in fstab (the original reads fstab as well).
fn resolve_automounts(mounts: &mut [MountPoint], fstab: &[MountPoint]) {
    for mount in mounts.iter_mut().filter(|mount| mount.fs_type == "autofs") {
        if let Some(real) = fstab.iter().find(|entry| entry.path == mount.path) {
            mount.clone_from(real);
        }
    }
}

/// Space available to the user (not to root) on the file system of `path`.
#[must_use]
pub fn free_space(path: &Path) -> Option<FreeSpace> {
    let stat = rustix::fs::statvfs(path).ok()?;
    Some(FreeSpace { free: stat.f_bavail * stat.f_frsize, total: stat.f_blocks * stat.f_frsize })
}

/// Parses the `/proc/self/mounts` (fstab) format.
#[must_use]
pub fn parse_mounts(text: &str) -> Vec<MountPoint> {
    let mut found: Vec<MountPoint> = Vec::new();
    for line in text.lines() {
        let mut fields = line.split_whitespace();
        let (Some(device), Some(dir), Some(fs_type), Some(options)) = (fields.next(), fields.next(), fields.next(), fields.next())
        else {
            continue;
        };
        let (device, dir) = (unescape(device), unescape(dir));
        if !is_shown(&device, &dir, fs_type, options) {
            continue;
        }
        let mount =
            MountPoint { kind: kind(&device, &dir, fs_type), path: PathBuf::from(&dir), device, fs_type: fs_type.to_owned() };
        match found.iter_mut().find(|known| known.path == mount.path) {
            // The real file system mounted over its automount placeholder.
            Some(known) if known.fs_type == "autofs" => *known = mount,
            // Mounted over again: keep the first one.
            Some(_) => {}
            None => found.push(mount),
        }
    }
    found
}

const HIDDEN_TYPES: [&str; 26] = [
    "ignore",
    "none",
    "cgroup",
    "cgroup2",
    "cpuset",
    "ramfs",
    "tmpfs",
    "proc",
    "swap",
    "sysfs",
    "debugfs",
    "devtmpfs",
    "devpts",
    "fusectl",
    "securityfs",
    "binfmt_misc",
    "fuse.portal",
    "fuse.gvfsd-fuse",
    "fuse.gvfs-fuse-daemon",
    "fuse.truecrypt",
    "nfsd",
    "usbfs",
    "mqueue",
    "configfs",
    "hugetlbfs",
    "rpc_pipefs",
];

/// `CheckMountEntry` of the original, plus `cgroup2`, which is newer than
/// its list.
fn is_shown(device: &str, dir: &str, fs_type: &str, options: &str) -> bool {
    let hidden_dir = dir.is_empty()
        || dir == "/"
        || dir == "none"
        || dir == "/proc"
        || ["/dev/", "/sys/", "/proc/", "/snap/", "/var/tmp/"].iter().any(|prefix| dir.starts_with(prefix))
        || dir.contains("/snapd/")
        || Path::new(dir).file_name().is_some_and(|name| name.to_string_lossy().starts_with('.'));
    let hidden_type = device == "proc" || HIDDEN_TYPES.contains(&fs_type) || fs_type == "selinuxfs";
    let hidden_options = options.split(',').any(|option| option == "bind" || option == "x-gvfs-hide");
    !(hidden_dir || hidden_type || hidden_options)
}

fn kind(device: &str, dir: &str, fs_type: &str) -> DriveKind {
    let (fs_upper, device_upper, dir_upper) = (fs_type.to_uppercase(), device.to_uppercase(), dir.to_uppercase());
    let any = |text: &str, words: &[&str]| words.iter().any(|word| text.contains(word));
    if any(&fs_upper, &["ISO9660", "CDROM", "CDRW", "DVD", "UDF"]) || any(&device_upper, &["ISO9660", "CDROM", "CDRW", "DVD"]) {
        DriveKind::Optical
    } else if any(&fs_upper, &["ZIP", "USB", "CAMERA"]) || any(&dir_upper, &["/MEDIA/", "/RUN/MEDIA/"]) {
        DriveKind::Removable
    } else if any(&fs_upper, &["NFS", "SMB", "NETW", "CIFS"]) {
        DriveKind::Network
    } else {
        DriveKind::HardDisk
    }
}

/// Mount files escape space, tab, newline and backslash as `\040` etc.
fn unescape(field: &str) -> String {
    let bytes = field.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'\\'
            && let Some(octal) = bytes.get(i + 1..i + 4)
            && let Ok(value) = u8::from_str_radix(&String::from_utf8_lossy(octal), 8)
        {
            out.push(value);
            i += 4;
        } else {
            out.push(bytes[i]);
            i += 1;
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Shortened `/proc/self/mounts` of the developer's machine.
    const SAMPLE: &str = "\
proc /proc proc rw,nosuid,nodev,noexec,relatime 0 0
sys /sys sysfs rw,nosuid,nodev,noexec,relatime 0 0
dev /dev devtmpfs rw,nosuid,relatime,size=7G 0 0
/dev/mapper/root / btrfs rw,relatime,subvol=/@ 0 0
tmpfs /run tmpfs rw,nosuid,nodev,size=3G,mode=755 0 0
cgroup2 /sys/fs/cgroup cgroup2 rw,nosuid,nodev,noexec 0 0
/dev/mapper/root /home btrfs rw,relatime,subvol=/@home 0 0
/dev/mapper/root /var/log btrfs rw,relatime,subvol=/@log 0 0
/dev/mapper/root /var/cache/pacman/pkg btrfs rw,relatime,subvol=/@pkg 0 0
/dev/nvme0n1p1 /boot vfat rw,relatime 0 0
tmpfs /tmp tmpfs rw,nosuid,nodev 0 0
//192.168.88.228/Hry /mnt/nintendo cifs rw,relatime,vers=3.1.1 0 0
/dev/sdb1 /run/media/jirka/USB\\040DISK vfat rw,nosuid,nodev 0 0
/dev/sr0 /run/media/jirka/CD iso9660 ro 0 0
/home/jirka/x /home/jirka/y none rw,bind 0 0
portal /run/user/1000/doc fuse.portal rw 0 0
";

    #[test]
    fn keeps_real_mounts_like_the_original() {
        let mounts = parse_mounts(SAMPLE);
        let shown: Vec<_> = mounts.iter().map(|m| (m.display_name(), m.kind)).collect();
        assert_eq!(
            shown,
            [
                ("home".to_owned(), DriveKind::HardDisk),
                ("log".to_owned(), DriveKind::HardDisk),
                ("pkg".to_owned(), DriveKind::HardDisk),
                ("boot".to_owned(), DriveKind::HardDisk),
                ("nintendo".to_owned(), DriveKind::Network),
                ("USB DISK".to_owned(), DriveKind::Removable),
                ("CD".to_owned(), DriveKind::Optical),
            ]
        );
        assert_eq!(mounts[5].path, Path::new("/run/media/jirka/USB DISK"));
    }

    #[test]
    fn automount_takes_the_real_type_from_a_later_mount_or_fstab() {
        let mounted = parse_mounts(
            "systemd-1 /mnt/nas autofs rw,fd=97 0 0\n//server/share /mnt/nas cifs rw 0 0\nsystemd-1 /mnt/other autofs rw 0 0\n",
        );
        assert_eq!(mounted[0].fs_type, "cifs");
        assert_eq!(mounted[0].kind, DriveKind::Network);

        let mut not_yet_used = mounted;
        let fstab = parse_mounts("//server/other /mnt/other cifs credentials=/x,x-systemd.automount 0 0\n");
        resolve_automounts(&mut not_yet_used, &fstab);
        assert_eq!((not_yet_used[1].fs_type.as_str(), not_yet_used[1].kind), ("cifs", DriveKind::Network));
    }

    #[test]
    fn free_space_of_existing_path() {
        let space = free_space(Path::new("/")).unwrap();
        assert!(space.total > 0 && space.free <= space.total);
        assert!(free_space(Path::new("/does/not/exist")).is_none());
    }
}
