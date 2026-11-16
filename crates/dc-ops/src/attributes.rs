//! Changing attributes (`cm_SetFileProperties`): permissions, owner and
//! group, times of modification and access, of the items chosen and, when
//! asked, of everything inside their directories. As `chmod`, `chown` and
//! `touch` do it; links are not followed (their own owner and times change,
//! a link has no permissions of its own).

use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

use dc_core::properties::ModeChange;
use dc_i18n::{fill, gettext};
use rustix::fs::{AtFlags, Timespec, Timestamps};

use crate::operation::{Aborted, Answer, Operation, OperationContext, Question};

/// What changes; `None` keeps what each item has.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct AttributeChanges {
    pub mode: ModeChange,
    pub owner: Option<u32>,
    pub group: Option<u32>,
    pub modified: Option<SystemTime>,
    pub accessed: Option<SystemTime>,
    /// Also everything inside the directories (links to directories are
    /// not entered).
    pub recursive: bool,
}

impl AttributeChanges {
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.mode.is_empty() && self.owner.is_none() && self.group.is_none() && self.modified.is_none() && self.accessed.is_none()
    }
}

#[derive(Debug)]
pub struct SetAttributesOperation {
    items: Vec<PathBuf>,
    changes: AttributeChanges,
    skip_errors: bool,
}

impl SetAttributesOperation {
    #[must_use]
    pub fn new(items: Vec<PathBuf>, changes: AttributeChanges) -> Self {
        Self { items, changes, skip_errors: false }
    }

    /// The items with, when recursive, everything inside them.
    fn collect(&self, ctx: &OperationContext) -> Result<Vec<PathBuf>, Aborted> {
        let mut all = Vec::new();
        let mut pending: Vec<PathBuf> = self.items.iter().rev().cloned().collect();
        while let Some(path) = pending.pop() {
            ctx.checkpoint()?;
            let is_dir = fs::symlink_metadata(&path).is_ok_and(|metadata| metadata.is_dir());
            if self.changes.recursive
                && is_dir
                && let Ok(entries) = fs::read_dir(&path)
            {
                let mut children: Vec<PathBuf> = entries.flatten().map(|entry| entry.path()).collect();
                children.sort();
                pending.extend(children.into_iter().rev());
            }
            all.push(path);
        }
        Ok(all)
    }

    fn change(&self, path: &Path) -> Result<(), (String, std::io::Error)> {
        let metadata = fs::symlink_metadata(path).map_err(|err| (gettext("Cannot read the attributes"), err))?;
        let changes = &self.changes;
        if changes.owner.is_some() || changes.group.is_some() {
            std::os::unix::fs::lchown(path, changes.owner, changes.group)
                .map_err(|err| (gettext("Cannot change the owner or the group"), err))?;
        }
        if !changes.mode.is_empty() && !metadata.is_symlink() {
            let old = metadata.permissions().mode();
            let new = changes.mode.apply(old, metadata.is_dir());
            if new != old & dc_core::properties::PERMISSIONS {
                fs::set_permissions(path, fs::Permissions::from_mode(new))
                    .map_err(|err| (gettext("Cannot change the permissions"), err))?;
            }
        }
        if changes.modified.is_some() || changes.accessed.is_some() {
            let keep = Timespec { tv_sec: 0, tv_nsec: rustix::fs::UTIME_OMIT };
            let times = Timestamps {
                last_access: changes.accessed.map_or(keep, timespec),
                last_modification: changes.modified.map_or(keep, timespec),
            };
            rustix::fs::utimensat(rustix::fs::CWD, path, &times, AtFlags::SYMLINK_NOFOLLOW)
                .map_err(|err| (gettext("Cannot change the times"), err.into()))?;
        }
        Ok(())
    }
}

fn timespec(time: SystemTime) -> Timespec {
    match time.duration_since(SystemTime::UNIX_EPOCH) {
        Ok(since) => Timespec { tv_sec: i64::try_from(since.as_secs()).unwrap_or(i64::MAX), tv_nsec: since.subsec_nanos().into() },
        Err(before) => {
            let before = before.duration();
            Timespec { tv_sec: -i64::try_from(before.as_secs()).unwrap_or(i64::MAX), tv_nsec: 0 }
        }
    }
}

impl Operation for SetAttributesOperation {
    fn description(&self) -> String {
        // rsOperSetFileProperty of the original.
        gettext("Setting property")
    }

    fn run(&mut self, ctx: &OperationContext) -> Result<(), Aborted> {
        let all = self.collect(ctx)?;
        ctx.update_progress(|p| p.total_files = Some(all.len() as u64));
        for path in &all {
            ctx.checkpoint()?;
            ctx.update_progress(|p| p.current = Some(path.clone()));
            while let Err((what, err)) = self.change(path) {
                if self.skip_errors {
                    break;
                }
                let question = Question::new(
                    fill(&gettext("{what}: {path}"), &[("what", &what), ("path", &path.display())]),
                    vec![Answer::Retry, Answer::Skip, Answer::SkipAll, Answer::Abort],
                    Answer::Retry,
                    Answer::Abort,
                )
                .with_detail(err.to_string());
                match ctx.ask(question)? {
                    Answer::Retry => {}
                    Answer::SkipAll => {
                        self.skip_errors = true;
                        break;
                    }
                    Answer::Abort => return Err(Aborted),
                    _ => break,
                }
            }
            ctx.update_progress(|p| p.done_files += 1);
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use super::*;

    fn mode(path: &Path) -> u32 {
        fs::symlink_metadata(path).unwrap().permissions().mode() & 0o7777
    }

    fn run(items: Vec<PathBuf>, changes: AttributeChanges) {
        let ctx = OperationContext::with_answerer(|_| Answer::Abort);
        SetAttributesOperation::new(items, changes).run(&ctx).unwrap();
    }

    #[test]
    fn permissions_of_a_tree_with_chmod_x() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().join("dir");
        fs::create_dir_all(root.join("sub")).unwrap();
        for (name, permissions) in [("doc.txt", 0o600), ("script.sh", 0o700), ("sub/a.txt", 0o600)] {
            fs::write(root.join(name), "").unwrap();
            fs::set_permissions(root.join(name), fs::Permissions::from_mode(permissions)).unwrap();
        }
        fs::set_permissions(root.join("sub"), fs::Permissions::from_mode(0o700)).unwrap();
        let readable = ModeChange { set: 0o055, clear: 0o002, execute_only_where_present: true };
        run(vec![root.clone()], AttributeChanges { mode: readable, recursive: true, ..AttributeChanges::default() });
        assert_eq!(mode(&root.join("doc.txt")), 0o644);
        assert_eq!(mode(&root.join("script.sh")), 0o755);
        assert_eq!(mode(&root.join("sub")), 0o755);
        assert_eq!(mode(&root.join("sub/a.txt")), 0o644);

        // Without recursion only the item itself.
        run(vec![root.clone()], AttributeChanges { mode: ModeChange { clear: 0o055, ..ModeChange::default() }, ..AttributeChanges::default() });
        assert_eq!(mode(&root), 0o700);
        assert_eq!(mode(&root.join("doc.txt")), 0o644);
    }

    #[test]
    fn times_and_own_group() {
        let tmp = tempfile::tempdir().unwrap();
        let file = tmp.path().join("a.txt");
        fs::write(&file, "").unwrap();
        let when = SystemTime::UNIX_EPOCH + Duration::from_secs(1_700_000_000);
        let gid = std::os::unix::fs::MetadataExt::gid(&fs::metadata(&file).unwrap());
        run(vec![file.clone()], AttributeChanges { modified: Some(when), group: Some(gid), ..AttributeChanges::default() });
        let metadata = fs::metadata(&file).unwrap();
        assert_eq!(metadata.modified().unwrap(), when);
        assert_ne!(metadata.accessed().unwrap(), when);
    }

    #[test]
    fn a_failure_is_a_question() {
        let tmp = tempfile::tempdir().unwrap();
        let missing = tmp.path().join("missing");
        let ctx = OperationContext::with_answerer(|_| Answer::Skip);
        let changes = AttributeChanges { mode: ModeChange { set: 0o700, ..ModeChange::default() }, ..AttributeChanges::default() };
        assert!(SetAttributesOperation::new(vec![missing], changes).run(&ctx).is_ok());
        assert!(AttributeChanges::default().is_empty());
    }
}
