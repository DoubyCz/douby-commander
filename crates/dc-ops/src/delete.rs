//! Deleting, as `TFileSystemDeleteOperation` of the original.
//!
//! - Permanently: the whole tree is listed and counted first (progress then
//!   shows done/total), then removed bottom-up, item by item.
//! - Into the trash: item by item as selected. When the trash fails, the user
//!   may delete the item directly instead.
//!
//! Read-only items, failures and trash failures are questions with the
//! answers of the original; "All" and "Skip All" are remembered for the rest
//! of the operation.

use std::path::{Path, PathBuf};

use dc_i18n::{fill, gettext};
use dc_vfs::{Entry, EntryKind, FileSource};

use crate::operation::{Aborted, Answer, Operation, OperationContext, Question};

/// How [`DeleteOperation`] gets rid of files.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DeleteMode {
    /// Move to the desktop trash, files can be restored from there.
    Trash,
    /// Remove for good.
    Permanently,
}

/// A remembered "All" or "Skip All".
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Remembered {
    Ask,
    Always,
    Never,
}

/// One item of a counted tree.
#[derive(Debug)]
struct Node {
    path: PathBuf,
    entry: Entry,
}

#[derive(Debug)]
pub struct DeleteOperation<S> {
    source: S,
    items: Vec<PathBuf>,
    mode: DeleteMode,
    read_only: Remembered,
    delete_directly: Remembered,
    skip_errors: bool,
}

impl<S: FileSource> DeleteOperation<S> {
    #[must_use]
    pub fn new(source: S, items: Vec<PathBuf>, mode: DeleteMode) -> Self {
        Self {
            source,
            items,
            mode,
            read_only: Remembered::Ask,
            delete_directly: Remembered::Ask,
            skip_errors: false,
        }
    }

    fn delete_permanently(&mut self, ctx: &OperationContext, roots: &[PathBuf]) -> Result<(), Aborted> {
        let nodes = self.collect(ctx, roots)?;
        let (files, bytes) = (nodes.len() as u64, nodes.iter().map(|node| node.entry.size).sum::<u64>());
        ctx.update_progress(|p| {
            p.total_files = Some(p.total_files.unwrap_or(0) + files);
            p.total_bytes = Some(p.total_bytes.unwrap_or(0) + bytes);
        });

        // Listed parents first, so the reverse removes contents before their directory.
        for node in nodes.iter().rev() {
            ctx.checkpoint()?;
            ctx.update_progress(|p| p.current = Some(node.path.clone()));
            self.remove_one(ctx, node)?;
            ctx.update_progress(|p| {
                p.done_files += 1;
                p.done_bytes += node.entry.size;
            });
        }
        Ok(())
    }

    fn delete_to_trash(&mut self, ctx: &OperationContext) -> Result<(), Aborted> {
        let items = self.items.clone();
        ctx.update_progress(|p| p.total_files = Some(items.len() as u64));

        for path in &items {
            ctx.checkpoint()?;
            ctx.update_progress(|p| p.current = Some(path.clone()));
            // An item that is gone already needs no trash.
            let gone = self.source.entry(path).is_err();
            if !gone && self.source.move_to_trash(path).is_err() && self.ask_delete_directly(ctx, path)? {
                self.delete_permanently(ctx, std::slice::from_ref(path))?;
            }
            ctx.update_progress(|p| p.done_files += 1);
        }
        Ok(())
    }

    /// Lists `roots` with everything inside, parents before their contents.
    /// Links are not followed. Unreadable items are a question.
    fn collect(&mut self, ctx: &OperationContext, roots: &[PathBuf]) -> Result<Vec<Node>, Aborted> {
        let mut nodes = Vec::new();
        let mut pending: Vec<PathBuf> = roots.iter().rev().cloned().collect();
        while let Some(path) = pending.pop() {
            ctx.checkpoint()?;
            let entry = match self.source.entry(&path) {
                Ok(entry) => entry,
                Err(err) => {
                    self.report_and_skip(ctx, &err)?;
                    continue;
                }
            };
            if entry.kind == EntryKind::Directory {
                match self.source.list(&path) {
                    Ok(children) => {
                        let mut children: Vec<PathBuf> = children.iter().map(|child| path.join(&child.name)).collect();
                        children.sort();
                        pending.extend(children.into_iter().rev());
                    }
                    // The directory is still listed, removing it then fails with its own question.
                    Err(err) => self.report_and_skip(ctx, &err)?,
                }
            }
            nodes.push(Node { path, entry });
        }
        Ok(nodes)
    }

    fn remove_one(&mut self, ctx: &OperationContext, node: &Node) -> Result<(), Aborted> {
        if is_read_only(&node.entry) && !self.confirm_read_only(ctx, &node.path)? {
            return Ok(());
        }
        loop {
            let Err(err) = self.source.remove(&node.path) else { return Ok(()) };
            if self.skip_errors {
                return Ok(());
            }
            // rsMsgCannotDeleteDirectory / rsMsgNotDelete of the original.
            let template = if node.entry.kind == EntryKind::Directory {
                gettext("Cannot delete directory {path}")
            } else {
                gettext("Can not delete file {path}")
            };
            let answer = ctx.ask(
                Question::new(
                    fill(&template, &[("path", &node.path.display())]),
                    vec![Answer::Retry, Answer::Skip, Answer::SkipAll, Answer::Abort],
                    Answer::Retry,
                    Answer::Abort,
                )
                .with_detail(err.reason()),
            )?;
            match answer {
                Answer::Retry => {}
                Answer::SkipAll => {
                    self.skip_errors = true;
                    return Ok(());
                }
                Answer::Abort => return Err(Aborted),
                _ => return Ok(()),
            }
        }
    }

    /// `rsMsgFileReadOnly` of the original. Returns whether to delete.
    fn confirm_read_only(&mut self, ctx: &OperationContext, path: &Path) -> Result<bool, Aborted> {
        match self.read_only {
            Remembered::Always => return Ok(true),
            Remembered::Never => return Ok(false),
            Remembered::Ask => {}
        }
        let answer = ctx.ask(Question::new(
            fill(&gettext("File {path} is marked as read-only. Delete it?"), &[("path", &path.display())]),
            vec![Answer::Yes, Answer::Skip, Answer::Abort, Answer::All, Answer::SkipAll],
            Answer::Yes,
            Answer::Abort,
        ))?;
        match answer {
            Answer::Yes => Ok(true),
            Answer::All => {
                self.read_only = Remembered::Always;
                Ok(true)
            }
            Answer::SkipAll => {
                self.read_only = Remembered::Never;
                Ok(false)
            }
            Answer::Abort => Err(Aborted),
            _ => Ok(false),
        }
    }

    /// `rsMsgDelToTrashForce` of the original. Returns whether to delete directly.
    fn ask_delete_directly(&mut self, ctx: &OperationContext, path: &Path) -> Result<bool, Aborted> {
        match self.delete_directly {
            Remembered::Always => return Ok(true),
            Remembered::Never => return Ok(false),
            Remembered::Ask => {}
        }
        let answer = ctx.ask(Question::new(
            fill(&gettext("Can not delete \"{path}\" to trash! Delete directly?"), &[("path", &path.display())]),
            vec![Answer::Yes, Answer::All, Answer::Skip, Answer::SkipAll, Answer::Abort],
            Answer::Yes,
            Answer::Abort,
        ))?;
        match answer {
            Answer::Yes => Ok(true),
            Answer::All => {
                self.delete_directly = Remembered::Always;
                Ok(true)
            }
            Answer::SkipAll => {
                self.delete_directly = Remembered::Never;
                Ok(false)
            }
            Answer::Abort => Err(Aborted),
            _ => Ok(false),
        }
    }

    /// An item that cannot even be read: Skip or Abort (`ShowError` of the original).
    fn report_and_skip(&mut self, ctx: &OperationContext, err: &dc_vfs::Error) -> Result<(), Aborted> {
        if self.skip_errors {
            return Ok(());
        }
        let question = Question::new(err.to_string(), vec![Answer::Skip, Answer::Abort], Answer::Skip, Answer::Abort)
            .with_detail(err.reason());
        match ctx.ask(question)? {
            Answer::Abort => Err(Aborted),
            _ => Ok(()),
        }
    }
}

impl<S: FileSource> Operation for DeleteOperation<S> {
    fn description(&self) -> String {
        // rsOperDeleting of the original.
        gettext("Deleting")
    }

    fn run(&mut self, ctx: &OperationContext) -> Result<(), Aborted> {
        match self.mode {
            DeleteMode::Permanently => {
                let items = self.items.clone();
                self.delete_permanently(ctx, &items)
            }
            DeleteMode::Trash => self.delete_to_trash(ctx),
        }
    }
}

/// `FileIsReadOnly` of the original on Unix: the owner may read but not write.
/// Links always have all permissions and are never read-only.
fn is_read_only(entry: &Entry) -> bool {
    !matches!(entry.kind, EntryKind::Symlink { .. }) && entry.mode & 0o400 != 0 && entry.mode & 0o200 == 0
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::os::unix::fs::{PermissionsExt, symlink};
    use std::sync::{Arc, Mutex};

    use dc_vfs::{Error, LocalFs};

    use super::*;

    /// Answers questions from a script and records them.
    fn scripted(answers: &[Answer]) -> (OperationContext, Arc<Mutex<Vec<String>>>) {
        let asked = Arc::new(Mutex::new(Vec::new()));
        let script: Vec<Answer> = answers.into();
        let script = Mutex::new(script.into_iter());
        let record = Arc::clone(&asked);
        let ctx = OperationContext::with_answerer(move |question| {
            record.lock().unwrap().push(format!("{}\n{}", question.text, question.detail));
            script.lock().unwrap().next().expect("unexpected question")
        });
        (ctx, asked)
    }

    fn set_mode(path: &Path, mode: u32) {
        fs::set_permissions(path, fs::Permissions::from_mode(mode)).unwrap();
    }

    #[test]
    fn deletes_trees_permanently_counting_everything_first() {
        let tmp = tempfile::tempdir().unwrap();
        let tree = tmp.path().join("tree");
        fs::create_dir_all(tree.join("a/b")).unwrap();
        fs::write(tree.join("a/b/file"), b"12345").unwrap();
        fs::write(tree.join("top.txt"), b"xx").unwrap();
        let single = tmp.path().join("single");
        fs::write(&single, b"1").unwrap();

        let (ctx, asked) = scripted(&[]);
        let mut op = DeleteOperation::new(LocalFs, vec![tree.clone(), single.clone()], DeleteMode::Permanently);
        op.run(&ctx).unwrap();

        assert!(!tree.exists() && !single.exists());
        assert!(asked.lock().unwrap().is_empty());
        let progress = ctx.progress();
        assert_eq!((progress.done_files, progress.total_files), (6, Some(6)));
        assert_eq!((progress.done_bytes, progress.total_bytes), (8, Some(8)));
    }

    #[test]
    fn link_to_directory_is_removed_not_followed() {
        let tmp = tempfile::tempdir().unwrap();
        let target = tmp.path().join("target");
        fs::create_dir(&target).unwrap();
        fs::write(target.join("keep"), b"").unwrap();
        let link = tmp.path().join("link");
        symlink(&target, &link).unwrap();

        let (ctx, _) = scripted(&[]);
        DeleteOperation::new(LocalFs, vec![link.clone()], DeleteMode::Permanently).run(&ctx).unwrap();
        assert!(fs::symlink_metadata(&link).is_err());
        assert!(target.join("keep").exists());
    }

    #[test]
    fn read_only_files_ask_and_all_is_remembered() {
        let tmp = tempfile::tempdir().unwrap();
        for name in ["r1", "r2", "r3"] {
            let path = tmp.path().join(name);
            fs::write(&path, b"").unwrap();
            set_mode(&path, 0o444);
        }
        let paths: Vec<_> = ["r1", "r2", "r3"].iter().map(|n| tmp.path().join(n)).collect();

        // Processed bottom-up: r3 first. Skip it, then All for the rest.
        let (ctx, asked) = scripted(&[Answer::Skip, Answer::All]);
        DeleteOperation::new(LocalFs, paths, DeleteMode::Permanently).run(&ctx).unwrap();

        assert_eq!(asked.lock().unwrap().len(), 2);
        assert!(asked.lock().unwrap()[0].contains("r3") && asked.lock().unwrap()[0].contains("read-only"));
        assert!(tmp.path().join("r3").exists());
        assert!(!tmp.path().join("r2").exists() && !tmp.path().join("r1").exists());
    }

    #[test]
    fn failures_ask_retry_skip_skip_all_abort() {
        let tmp = tempfile::tempdir().unwrap();
        let locked = tmp.path().join("locked");
        fs::create_dir(&locked).unwrap();
        for name in ["a", "b", "c"] {
            fs::write(locked.join(name), b"").unwrap();
        }
        // Files in a directory without write permission cannot be removed.
        set_mode(&locked, 0o555);

        // c fails: Retry (fails again), then Skip; b fails: Skip All; a then fails silently.
        // The directory itself (r-x for the owner) is read-only as in the original: Skip.
        let (ctx, asked) = scripted(&[Answer::Retry, Answer::Skip, Answer::SkipAll, Answer::Skip]);
        let result = DeleteOperation::new(LocalFs, vec![locked.clone()], DeleteMode::Permanently).run(&ctx);
        set_mode(&locked, 0o755);

        assert_eq!(result, Ok(()));
        let asked = asked.lock().unwrap();
        assert_eq!(asked.len(), 4);
        assert!(asked[0].starts_with("Can not delete file") && asked[0].contains("Permission denied"));
        assert!(asked[3].contains("read-only"));
        assert!(locked.join("a").exists() && locked.join("c").exists());
    }

    #[test]
    fn abort_stops_and_keeps_the_rest() {
        let tmp = tempfile::tempdir().unwrap();
        let locked = tmp.path().join("locked");
        fs::create_dir(&locked).unwrap();
        fs::write(locked.join("a"), b"").unwrap();
        set_mode(&locked, 0o555);
        let other = tmp.path().join("other");
        fs::write(&other, b"").unwrap();

        // "other" is processed first (bottom-up), then locked/a fails.
        let (ctx, _) = scripted(&[Answer::Abort]);
        let result = DeleteOperation::new(LocalFs, vec![locked.clone(), other.clone()], DeleteMode::Permanently).run(&ctx);
        set_mode(&locked, 0o755);

        assert_eq!(result, Err(Aborted));
        assert!(!other.exists());
        assert!(locked.join("a").exists());
    }

    /// File source whose trash never works, to test the fallback without
    /// touching the real trash of the user running the tests.
    struct NoTrash;

    impl FileSource for NoTrash {
        fn list(&self, dir: &Path) -> dc_vfs::Result<Vec<Entry>> {
            LocalFs.list(dir)
        }
        fn entry(&self, path: &Path) -> dc_vfs::Result<Entry> {
            LocalFs.entry(path)
        }
        fn create_directory(&self, path: &Path) -> dc_vfs::Result<()> {
            LocalFs.create_directory(path)
        }
        fn remove(&self, path: &Path) -> dc_vfs::Result<()> {
            LocalFs.remove(path)
        }
        fn move_to_trash(&self, path: &Path) -> dc_vfs::Result<()> {
            Err(Error::Trash { path: path.to_owned(), message: "no trash here".into() })
        }
    }

    #[test]
    fn failed_trash_offers_deleting_directly() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().join("dir");
        fs::create_dir(&dir).unwrap();
        fs::write(dir.join("inside"), b"").unwrap();
        let (one, two) = (tmp.path().join("one"), tmp.path().join("two"));
        fs::write(&one, b"").unwrap();
        fs::write(&two, b"").unwrap();

        // dir: Yes (deleted directly with its content), one: Skip, two: Yes.
        let (ctx, asked) = scripted(&[Answer::Yes, Answer::Skip, Answer::Yes]);
        DeleteOperation::new(NoTrash, vec![dir.clone(), one.clone(), two.clone()], DeleteMode::Trash).run(&ctx).unwrap();

        assert!(asked.lock().unwrap()[0].contains("to trash! Delete directly?"));
        assert!(!dir.exists() && one.exists() && !two.exists());
    }
}
