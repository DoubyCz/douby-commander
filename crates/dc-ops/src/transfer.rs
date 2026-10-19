//! Copying and moving on the local file system, as
//! `TFileSystemCopyOperation`, `TFileSystemMoveOperation` and their shared
//! `TFileSystemOperationHelper` of the original.
//!
//! - Copying lists and counts the whole tree first, then goes item by item;
//!   progress is in bytes, file contents in 512 KiB blocks, so pausing and
//!   stopping work inside big files too.
//! - Moving renames whole items where it can. Only across file systems, or
//!   when names change by a mask, it goes item by item: copy, then delete.
//! - What exists at the target, errors, links and properties that cannot be
//!   set are questions with the answers of the original; "All" answers are
//!   remembered for the rest of the operation.
//!
//! Not done (yet) from the original: verifying, file templates, correcting
//! links, appending and resuming, comparing by content.

use std::ffi::{OsStr, OsString};
use std::fs::{self, File, Metadata, OpenOptions};
use std::io::{self, ErrorKind};
use std::os::unix::ffi::OsStrExt;
use std::os::unix::fs::{FileExt, MetadataExt, PermissionsExt, lchown, symlink};
use std::path::{Path, PathBuf};

use dc_core::format;
use dc_i18n::{fill, gettext};
use rustix::fs::{AtFlags, CWD, FallocateFlags, Timespec, Timestamps, XattrFlags};

use crate::operation::{Aborted, Answer, Operation, OperationContext, Question};
use crate::target::Destination;

/// Block of the copy loop (`gCopyBlockSize` of the original).
const BLOCK_SIZE: usize = 512 * 1024;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TransferMode {
    Copy,
    Move,
}

/// "When file exists" of the options.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum FileExists {
    #[default]
    Ask,
    Overwrite,
    OverwriteOlder,
    Skip,
}

/// "When directory exists" of the options.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum DirectoryExists {
    #[default]
    Ask,
    Merge,
    Skip,
}

/// "When cannot set property" of the options.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum PropertyError {
    #[default]
    Ask,
    DontSetAnymore,
    Ignore,
}

/// "Follow links" of the copy options; moving never follows links.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum FollowLinks {
    #[default]
    Ask,
    Yes,
    No,
}

/// Options of the copy and move dialog, defaults of the original.
#[derive(Debug, Clone, PartialEq, Eq)]
#[expect(clippy::struct_excessive_bools, reason = "independent check boxes of the options dialog")]
pub struct TransferOptions {
    pub file_exists: FileExists,
    pub directory_exists: DirectoryExists,
    pub property_error: PropertyError,
    pub follow_links: FollowLinks,
    pub check_free_space: bool,
    /// Allocates the whole file before writing (fallocate).
    pub reserve_space: bool,
    /// Mode bits (`chmod`).
    pub copy_attributes: bool,
    /// Copied files are writable for their owner.
    pub drop_read_only: bool,
    pub copy_time: bool,
    pub copy_ownership: bool,
    /// Reflink (FICLONE) where the file system can; moving tries it always.
    pub copy_on_write: bool,
}

impl Default for TransferOptions {
    fn default() -> Self {
        Self {
            file_exists: FileExists::Ask,
            directory_exists: DirectoryExists::Ask,
            property_error: PropertyError::Ask,
            follow_links: FollowLinks::Ask,
            check_free_space: true,
            reserve_space: true,
            copy_attributes: true,
            drop_read_only: false,
            copy_time: true,
            copy_ownership: false,
            copy_on_write: false,
        }
    }
}

/// What to do with a file whose name exists at the target, the remembered
/// rule after an "All" answer included.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ExistsRule {
    Ask,
    Overwrite,
    Skip,
    OverwriteOlder,
    OverwriteSmaller,
    OverwriteLarger,
    AutoRenameSource,
    AutoRenameTarget,
}

/// Properties that failed and are not to be set any more ("Skip All").
#[derive(Debug, Clone, Copy, Default)]
#[expect(clippy::struct_excessive_bools, reason = "one flag per kind of property")]
struct Disabled {
    ownership: bool,
    attributes: bool,
    xattrs: bool,
    times: bool,
}

#[derive(Debug)]
enum Kind {
    File,
    /// Contents, `None` while not listed (moving by rename needs no listing).
    Directory(Option<Vec<Node>>),
    /// A link copied as a link.
    Link,
    /// Device, FIFO, socket: cannot be copied.
    Special,
}

/// One item of the tree to copy. For a followed link, `metadata` and the
/// kind are of what it points to.
#[derive(Debug)]
struct Node {
    path: PathBuf,
    metadata: Metadata,
    kind: Kind,
}

impl Node {
    fn name(&self) -> &OsStr {
        self.path.file_name().unwrap_or(self.path.as_os_str())
    }

    fn size(&self) -> u64 {
        if matches!(self.kind, Kind::File) { self.metadata.len() } else { 0 }
    }

    /// Files and bytes inside, as counted for the progress: every item
    /// but directories is a file. An unlisted directory counts as one.
    fn counts(&self) -> (u64, u64) {
        match &self.kind {
            Kind::Directory(Some(children)) => {
                children.iter().map(Self::counts).fold((0, 0), |(f, b), (cf, cb)| (f + cf, b + cb))
            }
            Kind::Directory(None) => (1, 0),
            _ => (1, self.size()),
        }
    }
}

#[derive(Debug)]
#[expect(clippy::struct_excessive_bools, reason = "independent answers remembered for the rest of the operation")]
pub struct TransferOperation {
    mode: TransferMode,
    sources: Vec<PathBuf>,
    destination: Destination,
    options: TransferOptions,
    exists: ExistsRule,
    directory_exists: DirectoryExists,
    follow_links: FollowLinks,
    property_error: PropertyError,
    disabled: Disabled,
    check_free_space: bool,
    skip_errors: bool,
    skip_itself: bool,
    single_directory: bool,
    time_zone: jiff::tz::TimeZone,
}

impl TransferOperation {
    #[must_use]
    pub fn new(mode: TransferMode, sources: Vec<PathBuf>, destination: Destination, options: TransferOptions) -> Self {
        let exists = match options.file_exists {
            FileExists::Ask => ExistsRule::Ask,
            FileExists::Overwrite => ExistsRule::Overwrite,
            FileExists::OverwriteOlder => ExistsRule::OverwriteOlder,
            FileExists::Skip => ExistsRule::Skip,
        };
        let follow_links = if mode == TransferMode::Move { FollowLinks::No } else { options.follow_links };
        Self {
            mode,
            sources,
            destination,
            exists,
            directory_exists: options.directory_exists,
            follow_links,
            property_error: options.property_error,
            disabled: Disabled::default(),
            check_free_space: options.check_free_space,
            skip_errors: false,
            skip_itself: false,
            single_directory: false,
            time_zone: jiff::tz::TimeZone::system(),
            options,
        }
    }

    // ----- Listing ---------------------------------------------------------

    /// Moving inside one file system without renaming needs no listing:
    /// whole items are renamed (`TFileSystemMoveOperation.Recursive`).
    fn needs_listing(&self) -> bool {
        self.mode == TransferMode::Copy
            || self.destination.renames()
            || self.sources.iter().any(|source| !same_volume(source, &self.destination.dir))
    }

    fn node(&mut self, ctx: &OperationContext, path: PathBuf, recursive: bool) -> Result<Option<Node>, Aborted> {
        ctx.checkpoint()?;
        let metadata = match fs::symlink_metadata(&path) {
            Ok(metadata) => metadata,
            Err(err) => {
                self.read_error(ctx, &path, &err)?;
                return Ok(None);
            }
        };
        if metadata.is_symlink() {
            if self.follow(ctx, &path)?
                && let Ok(target) = fs::metadata(&path)
                && !(target.is_dir() && links_to_ancestor(&path))
            {
                let kind = if target.is_dir() {
                    Kind::Directory(if recursive { Some(self.children(ctx, &path)?) } else { None })
                } else if target.is_file() {
                    Kind::File
                } else {
                    Kind::Special
                };
                return Ok(Some(Node { path, metadata: target, kind }));
            }
            return Ok(Some(Node { path, metadata, kind: Kind::Link }));
        }
        let kind = if metadata.is_dir() {
            Kind::Directory(if recursive { Some(self.children(ctx, &path)?) } else { None })
        } else if metadata.is_file() {
            Kind::File
        } else {
            Kind::Special
        };
        Ok(Some(Node { path, metadata, kind }))
    }

    fn children(&mut self, ctx: &OperationContext, dir: &Path) -> Result<Vec<Node>, Aborted> {
        let mut paths: Vec<PathBuf> = match fs::read_dir(dir) {
            Ok(entries) => entries.filter_map(Result::ok).map(|entry| entry.path()).collect(),
            Err(err) => {
                self.read_error(ctx, dir, &err)?;
                return Ok(Vec::new());
            }
        };
        paths.sort();
        let mut nodes = Vec::with_capacity(paths.len());
        for path in paths {
            if let Some(node) = self.node(ctx, path, true)? {
                nodes.push(node);
            }
        }
        Ok(nodes)
    }

    /// `rsMsgFollowSymlink` when the option says Ask.
    fn follow(&mut self, ctx: &OperationContext, path: &Path) -> Result<bool, Aborted> {
        match self.follow_links {
            FollowLinks::Yes => return Ok(true),
            FollowLinks::No => return Ok(false),
            FollowLinks::Ask => {}
        }
        let text = fill(&gettext("Follow symlink \"{path}\"?"), &[("path", &path.display())]);
        let answer = ctx.ask(Question::new(text, vec![Answer::Yes, Answer::All, Answer::No, Answer::SkipAll], Answer::Yes, Answer::No))?;
        Ok(match answer {
            Answer::Yes => true,
            Answer::All => {
                self.follow_links = FollowLinks::Yes;
                true
            }
            Answer::SkipAll => {
                self.follow_links = FollowLinks::No;
                false
            }
            _ => false,
        })
    }

    // ----- Processing ------------------------------------------------------

    /// Copies or moves `node` to `target`. Returns whether it was done
    /// completely, which decides if a moved directory may be removed.
    fn process(&mut self, ctx: &OperationContext, node: Node, target: PathBuf) -> Result<bool, Aborted> {
        ctx.checkpoint()?;
        let size = node.size();
        ctx.update_progress(|p| {
            p.current = Some(node.path.clone());
            p.target = Some(target.clone());
            p.file_done_bytes = 0;
            p.file_total_bytes = Some(size);
        });
        match node.kind {
            Kind::Directory(_) => self.directory(ctx, node, &target),
            Kind::File | Kind::Link => {
                let done = self.item(ctx, &node, target)?;
                ctx.update_progress(|p| p.done_files += 1);
                Ok(done)
            }
            Kind::Special => {
                self.special(ctx, &node.path)?;
                ctx.update_progress(|p| p.done_files += 1);
                Ok(false)
            }
        }
    }

    fn directory(&mut self, ctx: &OperationContext, node: Node, target: &Path) -> Result<bool, Aborted> {
        let mut created = false;
        if let Ok(existing) = fs::symlink_metadata(target) {
            if is_same(&node.metadata, &existing) {
                self.itself(ctx, &node.path)?;
                skip_subtree(ctx, &node);
                return Ok(false);
            }
            if fs::metadata(target).is_ok_and(|m| m.is_dir()) && !self.merge(ctx, target)? {
                skip_subtree(ctx, &node);
                return Ok(false);
            }
        } else {
            if self.mode == TransferMode::Move && !self.destination.renames() {
                match self.rename_directory(ctx, &node, target)? {
                    Renamed::Done => return Ok(true),
                    Renamed::Skipped => return Ok(false),
                    Renamed::OtherVolume => {}
                }
            }
            if !self.create_directory(ctx, target)? {
                skip_subtree(ctx, &node);
                return Ok(false);
            }
            created = true;
        }

        let (children, placeholder) = if let Kind::Directory(Some(children)) = node.kind {
            (children, false)
        } else {
            // Listed only now (moving across file systems).
            let children = self.children(ctx, &node.path)?;
            let (files, bytes) = children.iter().map(Node::counts).fold((0, 0), |(f, b), (cf, cb)| (f + cf, b + cb));
            ctx.update_progress(|p| {
                p.total_files = p.total_files.map(|total| total + files);
                p.total_bytes = p.total_bytes.map(|total| total + bytes);
            });
            (children, true)
        };
        let mut complete = true;
        for child in children {
            let is_file = matches!(child.kind, Kind::File | Kind::Special);
            let name = self.destination.inner_name_of(child.name(), is_file, self.single_directory);
            complete &= self.process(ctx, child, target.join(name))?;
        }
        if placeholder {
            ctx.update_progress(|p| p.done_files += 1);
        }
        // After the contents, which would change the time of the directory.
        if created {
            self.copy_properties(ctx, &node.path, &node.metadata, target, false)?;
        }
        if self.mode == TransferMode::Move && complete {
            // Leftovers keep it, as in the original; a failure here is not worth a question.
            let _ = fs::remove_dir(&node.path);
        }
        Ok(complete)
    }

    /// A file or a link that is not followed.
    fn item(&mut self, ctx: &OperationContext, node: &Node, target: PathBuf) -> Result<bool, Aborted> {
        let Some(target) = self.free_target(ctx, node, target)? else {
            ctx.update_progress(|p| p.done_bytes += node.size());
            return Ok(false);
        };
        if self.mode == TransferMode::Move {
            match self.rename_item(ctx, node, &target)? {
                Renamed::Done => return Ok(true),
                Renamed::Skipped => return Ok(false),
                Renamed::OtherVolume => {}
            }
        }
        let copied = match node.kind {
            Kind::Link => self.copy_link(ctx, node, &target)?,
            _ => self.copy_file(ctx, node, &target)?,
        };
        if copied && self.mode == TransferMode::Move {
            return self.remove_source(ctx, &node.path);
        }
        Ok(copied)
    }

    // ----- Moving by rename ------------------------------------------------

    fn rename_directory(&mut self, ctx: &OperationContext, node: &Node, target: &Path) -> Result<Renamed, Aborted> {
        loop {
            match fs::rename(&node.path, target) {
                Ok(()) => {
                    let (files, bytes) = node.counts();
                    ctx.update_progress(|p| {
                        p.done_files += files;
                        p.done_bytes += bytes;
                    });
                    return Ok(Renamed::Done);
                }
                Err(err) if err.kind() == ErrorKind::CrossesDevices => return Ok(Renamed::OtherVolume),
                Err(err) => {
                    let text = fill(&gettext("Cannot move directory {path}"), &[("path", &node.path.display())]);
                    let answers = [Answer::Skip, Answer::Retry, Answer::Abort, Answer::SkipAll];
                    if self.error(ctx, text, &err, &answers, Answer::Skip)? != Answer::Retry {
                        skip_subtree(ctx, node);
                        return Ok(Renamed::Skipped);
                    }
                }
            }
        }
    }

    fn rename_item(&mut self, ctx: &OperationContext, node: &Node, target: &Path) -> Result<Renamed, Aborted> {
        loop {
            match fs::rename(&node.path, target) {
                Ok(()) => {
                    ctx.update_progress(|p| p.done_bytes += node.size());
                    return Ok(Renamed::Done);
                }
                Err(err) if err.kind() == ErrorKind::CrossesDevices => return Ok(Renamed::OtherVolume),
                Err(err) => {
                    // rsMsgErrCannotMoveFile
                    let text = fill(&gettext("Cannot move file {path}"), &[("path", &node.path.display())]);
                    let answers = [Answer::Skip, Answer::Retry, Answer::Abort, Answer::SkipAll];
                    if self.error(ctx, text, &err, &answers, Answer::Skip)? != Answer::Retry {
                        ctx.update_progress(|p| p.done_bytes += node.size());
                        return Ok(Renamed::Skipped);
                    }
                }
            }
        }
    }

    /// After moving by copying.
    fn remove_source(&mut self, ctx: &OperationContext, path: &Path) -> Result<bool, Aborted> {
        loop {
            let Err(err) = fs::remove_file(path) else { return Ok(true) };
            let text = fill(&gettext("Can not delete file {path}"), &[("path", &path.display())]);
            match self.error(ctx, text, &err, &[Answer::Skip, Answer::Retry, Answer::Abort, Answer::SkipAll], Answer::Skip)? {
                Answer::Retry => {}
                _ => return Ok(false),
            }
        }
    }

    // ----- Copying ---------------------------------------------------------

    /// Copies the contents of a regular file (`CopyFile` of the original).
    fn copy_file(&mut self, ctx: &OperationContext, node: &Node, target: &Path) -> Result<bool, Aborted> {
        let size = node.size();
        if !self.enough_space(ctx, target, size)? {
            ctx.update_progress(|p| p.done_bytes += size);
            return Ok(false);
        }
        let Some(source) = self.open(ctx, &node.path, &gettext("Cannot open existing file"), || File::open(&node.path))? else {
            ctx.update_progress(|p| p.done_bytes += size);
            return Ok(false);
        };
        let create = || OpenOptions::new().write(true).create(true).truncate(true).open(target);
        let Some(destination) = self.open(ctx, target, &gettext("Cannot create file"), create)? else {
            ctx.update_progress(|p| p.done_bytes += size);
            return Ok(false);
        };

        if self.options.copy_on_write || self.mode == TransferMode::Move {
            match rustix::fs::ioctl_ficlone(&destination, &source) {
                Ok(()) => {
                    ctx.update_progress(|p| {
                        p.file_done_bytes = size;
                        p.done_bytes += size;
                    });
                    drop(destination);
                    self.copy_properties(ctx, &node.path, &node.metadata, target, false)?;
                    return Ok(true);
                }
                Err(err) if self.options.copy_on_write => {
                    drop(destination);
                    let _ = fs::remove_file(target);
                    let text = fill(
                        &gettext("Cannot copy file {from} to {to}"),
                        &[("from", &node.path.display()), ("to", &target.display())],
                    );
                    self.error(ctx, text, &err.into(), &[Answer::Skip, Answer::SkipAll, Answer::Abort], Answer::Skip)?;
                    ctx.update_progress(|p| p.done_bytes += size);
                    return Ok(false);
                }
                Err(_) => {}
            }
        }
        if self.options.reserve_space && size > 0 {
            // Only a hint, file systems without it are fine.
            let _ = rustix::fs::fallocate(&destination, FallocateFlags::empty(), 0, size);
        }

        let mut buffer = vec![0; BLOCK_SIZE];
        let mut offset: u64 = 0;
        loop {
            let read = match self.read_block(ctx, &source, &node.path, &mut buffer, offset) {
                Ok(Some(read)) => read,
                Ok(None) => return self.incomplete(ctx, target, size.saturating_sub(offset)),
                Err(aborted) => {
                    drop(destination);
                    let _ = fs::remove_file(target);
                    return Err(aborted);
                }
            };
            if read == 0 {
                break;
            }
            let written = match self.write_block(ctx, &destination, target, &buffer[..read], offset, size) {
                Ok(written) => written,
                Err(aborted) => {
                    drop(destination);
                    let _ = fs::remove_file(target);
                    return Err(aborted);
                }
            };
            if !written {
                return self.incomplete(ctx, target, size.saturating_sub(offset));
            }
            offset += read as u64;
            ctx.update_progress(|p| {
                p.file_done_bytes = offset;
                p.done_bytes += read as u64;
            });
            if let Err(aborted) = ctx.checkpoint() {
                drop(destination);
                let _ = fs::remove_file(target);
                return Err(aborted);
            }
        }
        // A file that grew while copied, the extra bytes were counted too.
        if offset < size {
            ctx.update_progress(|p| p.done_bytes += size - offset);
        }
        // Space reserved for a file that shrank meanwhile.
        if destination.set_len(offset).is_err() {
            // Harmless, the size is right unless the file shrank.
        }
        drop(destination);
        self.copy_properties(ctx, &node.path, &node.metadata, target, false)?;
        Ok(true)
    }

    /// Opens a file, `None` when skipped. `what` is `rsMsgErrEOpen` or `rsMsgErrECreate`.
    fn open(
        &mut self,
        ctx: &OperationContext,
        path: &Path,
        what: &str,
        open: impl Fn() -> io::Result<File>,
    ) -> Result<Option<File>, Aborted> {
        loop {
            match open() {
                Ok(file) => return Ok(Some(file)),
                Err(err) => {
                    let text = format!("{what}: {}", path.display());
                    match self.error(ctx, text, &err, &[Answer::Retry, Answer::Skip, Answer::SkipAll, Answer::Abort], Answer::Retry)? {
                        Answer::Retry => {}
                        _ => return Ok(None),
                    }
                }
            }
        }
    }

    /// `None` when skipped after a read error.
    fn read_block(
        &mut self,
        ctx: &OperationContext,
        source: &File,
        path: &Path,
        buffer: &mut [u8],
        offset: u64,
    ) -> Result<Option<usize>, Aborted> {
        loop {
            match source.read_at(buffer, offset) {
                Ok(read) => return Ok(Some(read)),
                Err(err) if err.kind() == ErrorKind::Interrupted => {}
                Err(err) => {
                    // rsMsgErrERead
                    let text = format!("{}: {}", gettext("Error reading from file"), path.display());
                    match self.error(ctx, text, &err, &[Answer::Retry, Answer::Skip, Answer::SkipAll, Answer::Abort], Answer::Retry)? {
                        Answer::Retry => {}
                        _ => return Ok(None),
                    }
                }
            }
        }
    }

    /// `false` when skipped after a write error.
    fn write_block(
        &mut self,
        ctx: &OperationContext,
        destination: &File,
        path: &Path,
        block: &[u8],
        offset: u64,
        size: u64,
    ) -> Result<bool, Aborted> {
        loop {
            let Err(err) = destination.write_all_at(block, offset) else { return Ok(true) };
            let left = size.saturating_sub(offset);
            let full = free_space(path).is_some_and(|free| free < left);
            if full {
                // rsMsgNoFreeSpaceRetry: Yes retries, No aborts.
                let text = gettext("No enough free space on target drive, Retry?");
                match ctx.ask(Question::new(text, vec![Answer::Yes, Answer::No, Answer::Skip], Answer::Yes, Answer::No))? {
                    Answer::Yes => {}
                    Answer::Skip => return Ok(false),
                    _ => return Err(Aborted),
                }
            } else {
                // rsMsgErrEWrite
                let text = format!("{}: {}", gettext("Error writing to file"), path.display());
                match self.error(ctx, text, &err, &[Answer::Retry, Answer::Skip, Answer::SkipAll, Answer::Abort], Answer::Retry)? {
                    Answer::Retry => {}
                    _ => return Ok(false),
                }
            }
        }
    }

    /// A copy skipped halfway: the partial file goes away after "Skip All",
    /// otherwise the user decides (`rsMsgDeletePartiallyCopied`).
    fn incomplete(&mut self, ctx: &OperationContext, target: &Path, left: u64) -> Result<bool, Aborted> {
        ctx.update_progress(|p| p.done_bytes += left);
        let delete = self.skip_errors || {
            let text = gettext("Delete the partially copied file ?");
            ctx.ask(Question::new(text, vec![Answer::Yes, Answer::No], Answer::Yes, Answer::No))? == Answer::Yes
        };
        if delete {
            let _ = fs::remove_file(target);
        }
        Ok(false)
    }

    /// `rsMsgNoFreeSpaceCont` before a file that does not fit. `false` to skip it.
    fn enough_space(&mut self, ctx: &OperationContext, target: &Path, size: u64) -> Result<bool, Aborted> {
        if !self.check_free_space || free_space(target).is_none_or(|free| size <= free) {
            return Ok(true);
        }
        if self.skip_errors {
            return Ok(false);
        }
        let text = gettext("No enough free space on target drive, Continue?");
        let answers = vec![Answer::Yes, Answer::All, Answer::No, Answer::Skip, Answer::SkipAll];
        match ctx.ask(Question::new(text, answers, Answer::Yes, Answer::No))? {
            Answer::Yes => Ok(true),
            Answer::All => {
                self.check_free_space = false;
                Ok(true)
            }
            Answer::Skip => Ok(false),
            Answer::SkipAll => {
                self.skip_errors = true;
                Ok(false)
            }
            _ => Err(Aborted),
        }
    }

    /// A link as a link, pointing where the original points.
    fn copy_link(&mut self, ctx: &OperationContext, node: &Node, target: &Path) -> Result<bool, Aborted> {
        loop {
            let result = fs::read_link(&node.path).and_then(|points_to| symlink(points_to, target));
            match result {
                Ok(()) => {
                    self.copy_properties(ctx, &node.path, &node.metadata, target, true)?;
                    return Ok(true);
                }
                Err(err) => {
                    let text = fill(&gettext("Error creating symlink {path}"), &[("path", &target.display())]);
                    match self.error(ctx, text, &err, &[Answer::Retry, Answer::Skip, Answer::SkipAll, Answer::Abort], Answer::Retry)? {
                        Answer::Retry => {}
                        _ => return Ok(false),
                    }
                }
            }
        }
    }

    fn create_directory(&mut self, ctx: &OperationContext, target: &Path) -> Result<bool, Aborted> {
        loop {
            let Err(err) = fs::create_dir(target) else { return Ok(true) };
            // rsMsgErrForceDir
            let text = fill(&gettext("Can not create directory {path}!"), &[("path", &target.display())]);
            match self.error(ctx, text, &err, &[Answer::Retry, Answer::Skip, Answer::SkipAll, Answer::Abort], Answer::Retry)? {
                Answer::Retry => {}
                _ => return Ok(false),
            }
        }
    }

    // ----- What exists at the target ---------------------------------------

    /// The path to write a file or link to, `None` to skip it. Asks what
    /// to do with an existing item and clears the way for overwriting.
    fn free_target(&mut self, ctx: &OperationContext, node: &Node, mut target: PathBuf) -> Result<Option<PathBuf>, Aborted> {
        loop {
            let Ok(existing) = fs::symlink_metadata(&target) else { return Ok(Some(target)) };
            if is_same(&node.metadata, &existing) || fs::metadata(&target).is_ok_and(|m| is_same(&node.metadata, &m)) {
                self.itself(ctx, &node.path)?;
                return Ok(None);
            }
            if existing.is_dir() {
                let text = fill(
                    &gettext("Cannot overwrite directory \"{target}\" with non-directory \"{source}\""),
                    &[("target", &target.display()), ("source", &node.path.display())],
                );
                let answer = ctx.ask(Question::new(text, vec![Answer::Skip, Answer::Cancel], Answer::Skip, Answer::Skip))?;
                return if answer == Answer::Cancel { Err(Aborted) } else { Ok(None) };
            }
            let rule = match self.exists {
                ExistsRule::Ask => match self.ask_exists(ctx, node, &target, &existing)? {
                    Choice::Rule(rule) => rule,
                    Choice::Rename(name) => {
                        target.set_file_name(name);
                        continue;
                    }
                    Choice::AskAgain => continue,
                },
                rule => rule,
            };
            let overwrite = match rule {
                ExistsRule::Overwrite => true,
                ExistsRule::Skip | ExistsRule::Ask => false,
                ExistsRule::OverwriteOlder => node.metadata.modified().ok() > existing.modified().ok(),
                ExistsRule::OverwriteSmaller => existing.len() < node.metadata.len(),
                ExistsRule::OverwriteLarger => existing.len() > node.metadata.len(),
                ExistsRule::AutoRenameSource => return Ok(Some(next_copy_name(&target))),
                ExistsRule::AutoRenameTarget => {
                    return if self.rename_existing(ctx, &target)? { Ok(Some(target)) } else { Ok(None) };
                }
            };
            if !overwrite {
                return Ok(None);
            }
            // Copying a file writes into the existing one (its links stay);
            // moving, links and a link in the way are replaced.
            if self.mode == TransferMode::Move || matches!(node.kind, Kind::Link) || existing.is_symlink() {
                let _ = fs::remove_file(&target);
            } else if existing.permissions().readonly() {
                let mut permissions = existing.permissions();
                permissions.set_mode(permissions.mode() | 0o200);
                let _ = fs::set_permissions(&target, permissions);
            }
            return Ok(Some(target));
        }
    }

    /// The "file exists" question (`FileExists` of the original). Comparing
    /// by content, appending and resuming are not offered.
    fn ask_exists(&mut self, ctx: &OperationContext, node: &Node, target: &Path, existing: &Metadata) -> Result<Choice, Aborted> {
        let info = |path: &Path, metadata: &Metadata| {
            let size = format::count(if metadata.is_symlink() { 0 } else { metadata.len() });
            let time = format::modified(metadata.modified().ok(), &self.time_zone);
            format!("{}\n{}", path.display(), fill(&gettext("{size} bytes, {time}"), &[("size", &size), ("time", &time)]))
        };
        let text = format!(
            "{}\n{}\n\n{}\n{}",
            gettext("Overwrite:"),
            info(target, existing),
            gettext("With file:"),
            info(&node.path, &node.metadata)
        );
        let answers = vec![
            Answer::Overwrite,
            Answer::Skip,
            Answer::Rename,
            Answer::OverwriteAll,
            Answer::SkipAll,
            Answer::OverwriteOlder,
            Answer::Cancel,
            Answer::OverwriteSmaller,
            Answer::OverwriteLarger,
            Answer::AutoRenameSource,
            Answer::AutoRenameTarget,
        ];
        let remember = |this: &mut Self, rule| {
            this.exists = rule;
            Choice::Rule(rule)
        };
        Ok(match ctx.ask(Question::new(text, answers, Answer::Overwrite, Answer::Skip))? {
            Answer::Overwrite => Choice::Rule(ExistsRule::Overwrite),
            Answer::OverwriteAll => remember(self, ExistsRule::Overwrite),
            Answer::SkipAll => remember(self, ExistsRule::Skip),
            Answer::OverwriteOlder => remember(self, ExistsRule::OverwriteOlder),
            Answer::OverwriteSmaller => remember(self, ExistsRule::OverwriteSmaller),
            Answer::OverwriteLarger => remember(self, ExistsRule::OverwriteLarger),
            Answer::AutoRenameSource => remember(self, ExistsRule::AutoRenameSource),
            Answer::AutoRenameTarget => remember(self, ExistsRule::AutoRenameTarget),
            Answer::Rename => {
                let name = target.file_name().map(|name| name.to_string_lossy().into_owned()).unwrap_or_default();
                match ctx.ask_text(gettext("Filename:"), name)? {
                    Some(name) if !name.trim().is_empty() => Choice::Rename(name.into()),
                    _ => Choice::AskAgain,
                }
            }
            Answer::Cancel => return Err(Aborted),
            _ => Choice::Rule(ExistsRule::Skip),
        })
    }

    /// "Auto-rename target files": the existing one gets the next copy name.
    fn rename_existing(&mut self, ctx: &OperationContext, target: &Path) -> Result<bool, Aborted> {
        loop {
            let new_name = next_copy_name(target);
            let Err(err) = fs::rename(target, &new_name) else { return Ok(true) };
            let text = fill(
                &gettext("Cannot rename file {from} to {to}"),
                &[("from", &target.display()), ("to", &new_name.display())],
            );
            match self.error(ctx, text, &err, &[Answer::Retry, Answer::Skip, Answer::SkipAll, Answer::Abort], Answer::Retry)? {
                Answer::Retry => {}
                _ => return Ok(false),
            }
        }
    }

    /// `rsMsgFolderExistsRwrt`. Returns whether to merge.
    fn merge(&mut self, ctx: &OperationContext, target: &Path) -> Result<bool, Aborted> {
        match self.directory_exists {
            DirectoryExists::Merge => return Ok(true),
            DirectoryExists::Skip => return Ok(false),
            DirectoryExists::Ask => {}
        }
        let text = fill(&gettext("Folder {path} exists, merge?"), &[("path", &target.display())]);
        let answers = vec![Answer::Merge, Answer::MergeAll, Answer::Skip, Answer::SkipAll, Answer::Cancel];
        match ctx.ask(Question::new(text, answers, Answer::Merge, Answer::Skip))? {
            Answer::Merge => Ok(true),
            Answer::MergeAll => {
                self.directory_exists = DirectoryExists::Merge;
                Ok(true)
            }
            Answer::SkipAll => {
                self.directory_exists = DirectoryExists::Skip;
                Ok(false)
            }
            Answer::Cancel => Err(Aborted),
            _ => Ok(false),
        }
    }

    /// `rsMsgCanNotCopyMoveItSelf`: Abort, Skip, Skip All.
    fn itself(&mut self, ctx: &OperationContext, path: &Path) -> Result<(), Aborted> {
        if self.skip_itself {
            return Ok(());
        }
        let text = fill(&gettext("You can not copy/move a file \"{path}\" to itself!"), &[("path", &path.display())]);
        match ctx.ask(Question::new(text, vec![Answer::Abort, Answer::Skip, Answer::SkipAll], Answer::Abort, Answer::Skip))? {
            Answer::Abort => Err(Aborted),
            Answer::SkipAll => {
                self.skip_itself = true;
                Ok(())
            }
            _ => Ok(()),
        }
    }

    /// `rsMsgCannotCopySpecialFile`: devices, FIFOs and sockets.
    fn special(&mut self, ctx: &OperationContext, path: &Path) -> Result<(), Aborted> {
        if self.skip_errors {
            return Ok(());
        }
        let text = fill(&gettext("Cannot copy special file {path}"), &[("path", &path.display())]);
        match ctx.ask(Question::new(text, vec![Answer::Skip, Answer::SkipAll, Answer::Abort], Answer::Skip, Answer::Abort))? {
            Answer::Abort => Err(Aborted),
            Answer::SkipAll => {
                self.skip_errors = true;
                Ok(())
            }
            _ => Ok(()),
        }
    }

    fn read_error(&mut self, ctx: &OperationContext, path: &Path, err: &io::Error) -> Result<(), Aborted> {
        let text = fill(&gettext("Cannot read {path}"), &[("path", &path.display())]);
        self.error(ctx, text, err, &[Answer::Skip, Answer::SkipAll, Answer::Abort], Answer::Skip).map(|_| ())
    }

    /// An error question. "Skip All" answers later ones with Skip, Abort and
    /// Cancel abort; other answers come back.
    fn error(
        &mut self,
        ctx: &OperationContext,
        text: String,
        err: &io::Error,
        answers: &[Answer],
        default: Answer,
    ) -> Result<Answer, Aborted> {
        if self.skip_errors && answers.contains(&Answer::SkipAll) {
            return Ok(Answer::Skip);
        }
        let question = Question::new(text, answers.to_vec(), default, Answer::Abort).with_detail(err.to_string());
        match ctx.ask(question)? {
            Answer::SkipAll => {
                self.skip_errors = true;
                Ok(Answer::Skip)
            }
            Answer::Abort | Answer::Cancel => Err(Aborted),
            answer => Ok(answer),
        }
    }

    // ----- Properties ------------------------------------------------------

    /// Owner, mode bits, extended attributes and times of `source` on
    /// `target` (`CopyProperties` of the original), times last as the
    /// others could change them. Failures are one question.
    fn copy_properties(
        &mut self,
        ctx: &OperationContext,
        source: &Path,
        metadata: &Metadata,
        target: &Path,
        is_link: bool,
    ) -> Result<(), Aborted> {
        let options = &self.options;
        let shown = target.display();
        let mut failed: Vec<(String, io::Error)> = Vec::new();
        let mut failed_kinds = Disabled::default();

        if options.copy_ownership
            && !self.disabled.ownership
            && let Err(err) = lchown(target, Some(metadata.uid()), Some(metadata.gid()))
        {
            failed.push((fill(&gettext("Can not set owner/group for \"{path}\""), &[("path", &shown)]), err));
            failed_kinds.ownership = true;
        }
        if options.copy_attributes && !is_link && !self.disabled.attributes {
            let mut mode = metadata.mode() & 0o7777;
            if options.drop_read_only {
                mode |= 0o200;
            }
            if let Err(err) = fs::set_permissions(target, fs::Permissions::from_mode(mode)) {
                failed.push((fill(&gettext("Can not set attributes for \"{path}\""), &[("path", &shown)]), err));
                failed_kinds.attributes = true;
            }
        }
        if !self.disabled.xattrs
            && let Err(err) = copy_xattrs(source, target)
        {
            failed.push((fill(&gettext("Can not set extended attributes for \"{path}\""), &[("path", &shown)]), err));
            failed_kinds.xattrs = true;
        }
        if options.copy_time && !self.disabled.times {
            let times = Timestamps {
                last_access: Timespec { tv_sec: metadata.atime(), tv_nsec: metadata.atime_nsec() },
                last_modification: Timespec { tv_sec: metadata.mtime(), tv_nsec: metadata.mtime_nsec() },
            };
            if let Err(err) = rustix::fs::utimensat(CWD, target, &times, AtFlags::SYMLINK_NOFOLLOW) {
                failed.push((fill(&gettext("Can not set date/time for \"{path}\""), &[("path", &shown)]), err.into()));
                failed_kinds.times = true;
            }
        }
        if failed.is_empty() {
            return Ok(());
        }

        let disable = |this: &mut Self| {
            this.disabled.ownership |= failed_kinds.ownership;
            this.disabled.attributes |= failed_kinds.attributes;
            this.disabled.xattrs |= failed_kinds.xattrs;
            this.disabled.times |= failed_kinds.times;
        };
        match self.property_error {
            PropertyError::Ignore => Ok(()),
            PropertyError::DontSetAnymore => {
                disable(self);
                Ok(())
            }
            PropertyError::Ask => {
                let text = failed.iter().map(|(text, _)| text.as_str()).collect::<Vec<_>>().join("\n");
                let detail = failed.iter().map(|(_, err)| err.to_string()).collect::<Vec<_>>().join("\n");
                let answers = vec![Answer::Skip, Answer::SkipAll, Answer::IgnoreAll, Answer::Abort];
                match ctx.ask(Question::new(text, answers, Answer::Skip, Answer::Abort).with_detail(detail))? {
                    Answer::SkipAll => disable(self),
                    Answer::IgnoreAll => self.property_error = PropertyError::Ignore,
                    Answer::Abort => return Err(Aborted),
                    _ => {}
                }
                Ok(())
            }
        }
    }
}

enum Renamed {
    Done,
    Skipped,
    /// Across file systems: has to be copied.
    OtherVolume,
}

enum Choice {
    Rule(ExistsRule),
    Rename(OsString),
    AskAgain,
}

impl Operation for TransferOperation {
    fn description(&self) -> String {
        match self.mode {
            // rsOperCopying, rsOperMoving
            TransferMode::Copy => gettext("Copying"),
            TransferMode::Move => gettext("Moving"),
        }
    }

    fn run(&mut self, ctx: &OperationContext) -> Result<(), Aborted> {
        ctx.update_progress(|p| p.by_bytes = true);
        // Several items to a new name: that directory is created first
        // (`ForceDirectories`); failing here shows up at the first item.
        let _ = fs::create_dir_all(&self.destination.dir);

        let recursive = self.needs_listing();
        let mut roots = Vec::new();
        for source in self.sources.clone() {
            if let Some(node) = self.node(ctx, source, recursive)? {
                roots.push(node);
            }
        }
        let (files, bytes) = roots.iter().map(Node::counts).fold((0, 0), |(f, b), (cf, cb)| (f + cf, b + cb));
        ctx.update_progress(|p| {
            p.total_files = Some(files);
            p.total_bytes = Some(bytes);
        });

        self.single_directory = matches!(roots.as_slice(), [Node { kind: Kind::Directory(_), .. }]);
        for root in roots {
            let is_file = matches!(root.kind, Kind::File | Kind::Special);
            let name = self.destination.name_of(root.name(), is_file, self.single_directory);
            let target = self.destination.dir.join(name);
            self.process(ctx, root, target)?;
        }
        Ok(())
    }
}

/// A skipped item with everything inside counts as done.
fn skip_subtree(ctx: &OperationContext, node: &Node) {
    let (files, bytes) = node.counts();
    ctx.update_progress(|p| {
        p.done_files += files;
        p.done_bytes += bytes;
    });
}

/// Same file system object: copying onto itself.
fn is_same(a: &Metadata, b: &Metadata) -> bool {
    a.dev() == b.dev() && a.ino() == b.ino()
}

/// A link to a directory that contains the link: following it never ends.
fn links_to_ancestor(link: &Path) -> bool {
    let (Ok(target), Some(Ok(parent))) = (fs::canonicalize(link), link.parent().map(fs::canonicalize)) else {
        return true;
    };
    parent.starts_with(target)
}

/// Whether `source` and the directory `dir` (or its nearest existing
/// ancestor) are on one file system, where a rename moves anything.
fn same_volume(source: &Path, dir: &Path) -> bool {
    let Ok(source) = fs::symlink_metadata(source) else { return false };
    dir.ancestors().find_map(|ancestor| fs::metadata(ancestor).ok()).is_some_and(|target| target.dev() == source.dev())
}

fn free_space(path: &Path) -> Option<u64> {
    let dir = path.parent().unwrap_or(path);
    dc_vfs::mounts::free_space(dir).map(|space| space.free)
}

/// `Copy (1) name.ext`, the first that does not exist (`GetNextCopyName`
/// of the original); a hidden `.name` becomes `.Copy (1) name`.
fn next_copy_name(path: &Path) -> PathBuf {
    let name = path.file_name().map(|name| name.to_string_lossy().into_owned()).unwrap_or_default();
    let (prefix, rest) = name.strip_prefix('.').map_or(("", name.as_str()), |rest| (".", rest));
    let template = gettext("Copy ({number}) {name}");
    let mut number: u64 = 1;
    loop {
        let candidate = path.with_file_name(format!("{prefix}{}", fill(&template, &[("number", &number), ("name", &rest)])));
        if fs::symlink_metadata(&candidate).is_err() {
            return candidate;
        }
        number += 1;
    }
}

/// Extended attributes of `source` set on `target`, links not followed.
/// A target file system without them is not an error.
fn copy_xattrs(source: &Path, target: &Path) -> io::Result<()> {
    let unsupported = |err: &rustix::io::Errno| *err == rustix::io::Errno::NOTSUP;
    // An empty buffer asks for the size.
    let size = match rustix::fs::llistxattr(source, &mut [0_u8; 0][..]) {
        Ok(0) => return Ok(()),
        Ok(size) => size,
        Err(err) if unsupported(&err) => return Ok(()),
        Err(err) => return Err(err.into()),
    };
    let mut names = vec![0_u8; size];
    let length = rustix::fs::llistxattr(source, &mut names[..])?;
    for name in names[..length.min(names.len())].split(|&byte| byte == 0).filter(|name| !name.is_empty()) {
        let name = OsStr::from_bytes(name);
        let size = rustix::fs::lgetxattr(source, name, &mut [0_u8; 0][..])?;
        let mut value = vec![0_u8; size];
        let length = rustix::fs::lgetxattr(source, name, &mut value[..])?;
        match rustix::fs::lsetxattr(target, name, &value[..length.min(value.len())], XattrFlags::empty()) {
            Err(err) if unsupported(&err) => return Ok(()),
            result => result?,
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::sync::{Arc, Mutex};

    use super::*;

    /// Answers questions from a script and records their texts.
    fn scripted(answers: &[(Answer, Option<&str>)]) -> (OperationContext, Arc<Mutex<Vec<String>>>) {
        let asked = Arc::new(Mutex::new(Vec::new()));
        let script: Vec<(Answer, Option<String>)> = answers.iter().map(|(a, t)| (*a, t.map(str::to_owned))).collect();
        let script = Mutex::new(script.into_iter());
        let record = Arc::clone(&asked);
        let ctx = OperationContext::with_replies(move |question| {
            record.lock().unwrap().push(question.text.clone());
            script.lock().unwrap().next().unwrap_or_else(|| panic!("unexpected question: {}", question.text))
        });
        (ctx, asked)
    }

    fn run(mode: TransferMode, sources: &[&Path], dir: &Path, mask: Option<&str>, ctx: &OperationContext) -> Result<(), Aborted> {
        let destination = Destination { dir: dir.to_owned(), mask: mask.map(str::to_owned) };
        let sources = sources.iter().map(|path| path.to_path_buf()).collect();
        let options = TransferOptions { follow_links: FollowLinks::No, ..TransferOptions::default() };
        TransferOperation::new(mode, sources, destination, options).run(ctx)
    }

    fn tree(root: &Path) {
        fs::create_dir_all(root.join("sub/deep")).unwrap();
        fs::write(root.join("a.txt"), b"hello").unwrap();
        fs::write(root.join("sub/b.txt"), vec![7_u8; BLOCK_SIZE * 2 + 3]).unwrap();
        fs::write(root.join("sub/deep/c"), b"").unwrap();
        symlink("a.txt", root.join("link")).unwrap();
    }

    #[test]
    fn copies_a_tree_with_contents_links_and_times() {
        let tmp = tempfile::tempdir().unwrap();
        let src = tmp.path().join("src");
        tree(&src);
        let old = Timestamps {
            last_access: Timespec { tv_sec: 1_000_000_000, tv_nsec: 0 },
            last_modification: Timespec { tv_sec: 1_000_000_000, tv_nsec: 0 },
        };
        rustix::fs::utimensat(CWD, src.join("a.txt"), &old, AtFlags::empty()).unwrap();
        let out = tmp.path().join("out");
        fs::create_dir(&out).unwrap();

        let (ctx, asked) = scripted(&[]);
        run(TransferMode::Copy, &[&src], &out, None, &ctx).unwrap();

        assert!(asked.lock().unwrap().is_empty());
        assert_eq!(fs::read(out.join("src/a.txt")).unwrap(), b"hello");
        assert_eq!(fs::read(out.join("src/sub/b.txt")).unwrap(), fs::read(src.join("sub/b.txt")).unwrap());
        assert!(out.join("src/sub/deep/c").is_file());
        assert_eq!(fs::read_link(out.join("src/link")).unwrap(), Path::new("a.txt"));
        assert_eq!(fs::metadata(out.join("src/a.txt")).unwrap().mtime(), 1_000_000_000);
        assert!(src.join("a.txt").exists(), "copying keeps the source");

        let progress = ctx.progress();
        assert_eq!((progress.done_files, progress.total_files), (4, Some(4)));
        assert_eq!(progress.done_bytes, progress.total_bytes.unwrap());
        assert_eq!(progress.total_bytes, Some(5 + (BLOCK_SIZE as u64) * 2 + 3));
    }

    #[test]
    fn moves_by_rename_and_with_a_mask() {
        let tmp = tempfile::tempdir().unwrap();
        let src = tmp.path().join("src");
        tree(&src);
        let out = tmp.path().join("out");
        fs::create_dir(&out).unwrap();

        let (ctx, _) = scripted(&[]);
        run(TransferMode::Move, &[&src], &out, None, &ctx).unwrap();
        assert!(!src.exists() && out.join("src/sub/b.txt").is_file());

        let (ctx, _) = scripted(&[]);
        let a = out.join("src/a.txt");
        run(TransferMode::Move, &[&a], &out, Some("*.bak"), &ctx).unwrap();
        assert!(!a.exists() && out.join("a.bak").is_file());
    }

    /// `/tmp` and the build directory are often different file systems
    /// (tmpfs, disk); then moving copies and deletes. Passes either way.
    #[test]
    fn moves_across_file_systems() {
        let tmp = tempfile::tempdir().unwrap();
        let src = tmp.path().join("src");
        tree(&src);
        let other = tempfile::tempdir_in(concat!(env!("CARGO_MANIFEST_DIR"), "/../../target")).unwrap();

        let (ctx, asked) = scripted(&[]);
        run(TransferMode::Move, &[&src], other.path(), None, &ctx).unwrap();
        assert!(asked.lock().unwrap().is_empty());
        assert!(!src.exists(), "moved away completely");
        assert_eq!(fs::read(other.path().join("src/a.txt")).unwrap(), b"hello");
        assert_eq!(fs::read_link(other.path().join("src/link")).unwrap(), Path::new("a.txt"));
        let progress = ctx.progress();
        assert_eq!(progress.done_files, progress.total_files.unwrap());
    }

    #[test]
    fn one_directory_copied_under_a_new_name() {
        let tmp = tempfile::tempdir().unwrap();
        let src = tmp.path().join("src");
        tree(&src);
        let (ctx, _) = scripted(&[]);
        run(TransferMode::Copy, &[&src], tmp.path(), Some("copy"), &ctx).unwrap();
        assert!(tmp.path().join("copy/sub/b.txt").is_file());
        assert!(tmp.path().join("copy/a.txt").is_file(), "names inside keep");
    }

    #[test]
    fn existing_files_ask_and_answers_are_remembered() {
        let tmp = tempfile::tempdir().unwrap();
        let (src, out) = (tmp.path().join("src"), tmp.path().join("out"));
        fs::create_dir_all(&src).unwrap();
        fs::create_dir_all(&out).unwrap();
        for name in ["1", "2", "3", "4", "5"] {
            fs::write(src.join(name), format!("new {name}")).unwrap();
            fs::write(out.join(name), "old").unwrap();
        }
        let sources: Vec<PathBuf> = ["1", "2", "3", "4", "5"].iter().map(|n| src.join(n)).collect();
        let sources: Vec<&Path> = sources.iter().map(PathBuf::as_path).collect();

        // 1 overwrite, 2 skip, 3 renamed by hand, 4 auto-rename for the rest.
        let (ctx, asked) = scripted(&[
            (Answer::Overwrite, None),
            (Answer::Skip, None),
            (Answer::Rename, None),
            (Answer::Yes, Some("three")),
            (Answer::AutoRenameSource, None),
        ]);
        run(TransferMode::Copy, &sources, &out, None, &ctx).unwrap();

        assert_eq!(fs::read_to_string(out.join("1")).unwrap(), "new 1");
        assert_eq!(fs::read_to_string(out.join("2")).unwrap(), "old");
        assert_eq!(fs::read_to_string(out.join("three")).unwrap(), "new 3");
        assert_eq!(fs::read_to_string(out.join("Copy (1) 4")).unwrap(), "new 4");
        assert_eq!(fs::read_to_string(out.join("Copy (1) 5")).unwrap(), "new 5");
        let asked = asked.lock().unwrap();
        assert_eq!(asked.len(), 5);
        assert!(asked[0].starts_with("Overwrite:\n") && asked[0].contains("With file:") && asked[0].contains("3 bytes"));
    }

    #[test]
    fn existing_directories_are_merged_on_request() {
        let tmp = tempfile::tempdir().unwrap();
        let (src, out) = (tmp.path().join("src"), tmp.path().join("out"));
        fs::create_dir_all(src.join("d")).unwrap();
        fs::write(src.join("d/new"), b"n").unwrap();
        fs::create_dir_all(out.join("d")).unwrap();
        fs::write(out.join("d/kept"), b"k").unwrap();

        let (ctx, asked) = scripted(&[(Answer::Merge, None)]);
        run(TransferMode::Move, &[&src.join("d")], &out, None, &ctx).unwrap();
        assert!(asked.lock().unwrap()[0].contains("exists, merge?"));
        assert!(out.join("d/new").is_file() && out.join("d/kept").is_file());
        assert!(!src.join("d").exists(), "a fully moved directory goes away");
    }

    #[test]
    fn copying_onto_itself_is_refused() {
        let tmp = tempfile::tempdir().unwrap();
        let file = tmp.path().join("f");
        fs::write(&file, b"x").unwrap();
        let (ctx, asked) = scripted(&[(Answer::Skip, None)]);
        run(TransferMode::Copy, &[&file], tmp.path(), None, &ctx).unwrap();
        assert!(asked.lock().unwrap()[0].contains("to itself"));
        assert_eq!(fs::read(&file).unwrap(), b"x");
    }

    #[test]
    fn followed_links_copy_what_they_point_to() {
        let tmp = tempfile::tempdir().unwrap();
        let src = tmp.path().join("src");
        tree(&src);
        let out = tmp.path().join("out");
        fs::create_dir(&out).unwrap();
        let (ctx, asked) = scripted(&[(Answer::Yes, None)]);
        let destination = Destination { dir: out.clone(), mask: None };
        TransferOperation::new(TransferMode::Copy, vec![src.join("link")], destination, TransferOptions::default())
            .run(&ctx)
            .unwrap();
        assert!(asked.lock().unwrap()[0].starts_with("Follow symlink"));
        let copied = out.join("link");
        assert!(!fs::symlink_metadata(&copied).unwrap().is_symlink());
        assert_eq!(fs::read(&copied).unwrap(), b"hello");
    }

    #[test]
    fn copy_names_like_the_original() {
        let tmp = tempfile::tempdir().unwrap();
        fs::write(tmp.path().join("Copy (1) a.txt"), b"").unwrap();
        assert_eq!(next_copy_name(&tmp.path().join("a.txt")), tmp.path().join("Copy (2) a.txt"));
        assert_eq!(next_copy_name(&tmp.path().join(".bashrc")), tmp.path().join(".Copy (1) bashrc"));
    }
}
