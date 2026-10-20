//! Copying out of an archive (F5 in a panel inside an archive).
//!
//! Two steps, so that everything copying does applies as it is: the
//! selected items are extracted into a hidden temporary directory inside
//! the target directory (the same file system), then moved from there by
//! [`TransferOperation`], a rename each, with its questions (file exists,
//! merge), rename masks and progress. The temporary directory goes away in
//! any case, also when stopped.

use std::fs;
use std::path::PathBuf;

use dc_i18n::{fill, gettext};
use dc_vfs::archive::{ExtractEvent, extract};

use crate::operation::{Aborted, Answer, Operation, OperationContext, Question};
use crate::target::Destination;
use crate::transfer::{TransferMode, TransferOperation, TransferOptions};

#[derive(Debug)]
pub struct ExtractOperation {
    /// The archive file on disk.
    archive: PathBuf,
    /// Directory within the archive the items are listed in (virtual path).
    base: PathBuf,
    /// The items to extract (virtual paths).
    items: Vec<PathBuf>,
    destination: Destination,
    options: TransferOptions,
    /// Bytes of all files to extract, for the progress.
    total_bytes: u64,
}

impl ExtractOperation {
    #[must_use]
    pub fn new(
        archive: PathBuf,
        base: PathBuf,
        items: Vec<PathBuf>,
        destination: Destination,
        options: TransferOptions,
        total_bytes: u64,
    ) -> Self {
        Self { archive, base, items, destination, options, total_bytes }
    }

    /// An error that ends the operation, told to the user first.
    fn fail(ctx: &OperationContext, text: String, detail: String) -> Aborted {
        let question = Question::new(text, vec![Answer::Abort], Answer::Abort, Answer::Abort).with_detail(detail);
        // Stopped while asking ends it the same way.
        let _ = ctx.ask(question);
        Aborted
    }
}

impl Operation for ExtractOperation {
    fn description(&self) -> String {
        gettext("Extracting")
    }

    fn run(&mut self, ctx: &OperationContext) -> Result<(), Aborted> {
        let target_dir = self.destination.dir.clone();
        // A new target directory is created first, as when copying.
        let _ = fs::create_dir_all(&target_dir);
        let temporary = tempfile::Builder::new().prefix(".douby-extract-").tempdir_in(&target_dir).map_err(|err| {
            let text = fill(&gettext("Can not create directory {path}!"), &[("path", &target_dir.display())]);
            Self::fail(ctx, text, err.to_string())
        })?;

        let (total_files, total_bytes) = (self.items.len() as u64, self.total_bytes);
        ctx.update_progress(|p| {
            p.by_bytes = true;
            p.total_files = Some(total_files);
            p.total_bytes = Some(total_bytes);
            p.target = Some(target_dir.clone());
        });
        let (temp, base) = (temporary.path().to_owned(), self.base.clone());
        let extracted = extract(&self.archive, &self.base, &self.items, &temp, |event| {
            match event {
                ExtractEvent::File(path) => {
                    // Shown as where it is in the archive, not the hidden directory.
                    let shown = path.strip_prefix(&temp).map(|relative| base.join(relative)).ok();
                    ctx.update_progress(|p| {
                        p.current = shown;
                        p.file_done_bytes = 0;
                        p.file_total_bytes = None;
                    });
                }
                ExtractEvent::Bytes(count) => ctx.update_progress(|p| {
                    p.done_bytes += count;
                    p.file_done_bytes += count;
                }),
            }
            ctx.checkpoint().is_ok()
        });
        match extracted {
            Ok(true) => {}
            Ok(false) => return Err(Aborted),
            Err(err) => return Err(Self::fail(ctx, err.to_string(), err.reason())),
        }

        // Into place, with everything copying does.
        let sources: Vec<PathBuf> = self
            .items
            .iter()
            .filter_map(|item| item.file_name())
            .map(|name| temporary.path().join(name))
            .filter(|path| fs::symlink_metadata(path).is_ok())
            .collect();
        let mut moving = TransferOperation::new(TransferMode::Move, sources, self.destination.clone(), self.options.clone());
        let result = moving.run(ctx);
        drop(temporary);
        result
    }
}

#[cfg(test)]
mod tests {
    use std::process::Command;

    use super::*;

    #[test]
    fn extracts_into_the_target_and_cleans_up() {
        let tmp = tempfile::tempdir().unwrap();
        let tree = tmp.path().join("tree");
        fs::create_dir_all(tree.join("dir")).unwrap();
        fs::write(tree.join("dir/a.txt"), b"aaa").unwrap();
        fs::write(tree.join("b.txt"), b"bb").unwrap();
        let zip = tmp.path().join("t.zip");
        let made = Command::new("bsdtar").args(["--format", "zip", "-cf"]).arg(&zip).arg("-C").arg(&tree).args(["dir", "b.txt"]).status();
        if !made.is_ok_and(|status| status.success()) {
            return; // No bsdtar here.
        }
        let out = tmp.path().join("out");
        fs::create_dir(&out).unwrap();
        fs::write(out.join("b.txt"), b"old").unwrap();

        let ctx = OperationContext::with_answerer(|question| {
            assert!(question.text.starts_with("Overwrite:"), "only the existing b.txt is asked about");
            Answer::Overwrite
        });
        let destination = Destination { dir: out.clone(), mask: None };
        let mut operation = ExtractOperation::new(
            zip.clone(),
            zip.clone(),
            vec![zip.join("dir"), zip.join("b.txt")],
            destination,
            TransferOptions::default(),
            5,
        );
        operation.run(&ctx).unwrap();

        assert_eq!(fs::read(out.join("dir/a.txt")).unwrap(), b"aaa");
        assert_eq!(fs::read(out.join("b.txt")).unwrap(), b"bb");
        let left: Vec<_> = fs::read_dir(&out).unwrap().filter_map(Result::ok).map(|entry| entry.file_name()).collect();
        assert_eq!(left.len(), 2, "no temporary directory left: {left:?}");
    }
}
