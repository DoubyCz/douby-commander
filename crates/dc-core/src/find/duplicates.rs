//! Duplicate files: the files a search finds, grouped by the same name,
//! the same size, and then by the same hash (BLAKE3) or the same content
//! compared byte by byte. Only groups of two and more are found.

use std::collections::HashMap;
use std::ffi::OsString;
use std::fs::File;
use std::io::{self, Read};
use std::path::Path;

use serde::{Deserialize, Serialize};

use super::{Found, Progress};

/// What makes files duplicates; name or size at least.
#[expect(clippy::struct_excessive_bools, reason = "the check boxes of the search window")]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct Duplicates {
    pub name: bool,
    pub size: bool,
    pub hash: bool,
    pub content: bool,
}

/// Groups `candidates` and tells `progress` about the members of each
/// group, groups numbered from 1 in the order of their first paths.
pub(super) fn group(candidates: Vec<Found>, by: Duplicates, progress: &Progress) {
    let mut first: HashMap<(Option<OsString>, Option<u64>), Vec<Found>> = HashMap::new();
    for found in candidates {
        let name = by.name.then(|| found.path.file_name().unwrap_or_default().to_owned());
        let size = by.size.then_some(found.size);
        first.entry((name, size)).or_default().push(found);
    }
    let mut groups: Vec<Vec<Found>> = first.into_values().filter(|group| group.len() > 1).collect();
    if by.hash {
        groups = split(groups, progress, |files, progress| {
            let mut by_hash: HashMap<blake3::Hash, Vec<Found>> = HashMap::new();
            for found in files {
                progress.enter(&found.path);
                if let Ok(hash) = hash(&found.path) {
                    by_hash.entry(hash).or_default().push(found);
                }
            }
            by_hash.into_values().collect()
        });
    }
    if by.content {
        groups = split(groups, progress, |files, progress| {
            let mut same: Vec<Vec<Found>> = Vec::new();
            for found in files {
                progress.enter(&found.path);
                match same.iter_mut().find(|group| same_content(&group[0].path, &found.path).unwrap_or(false)) {
                    Some(group) => group.push(found),
                    None => same.push(vec![found]),
                }
            }
            same
        });
    }
    for group in &mut groups {
        group.sort_by(|a, b| a.path.cmp(&b.path));
    }
    groups.sort_by(|a, b| a[0].path.cmp(&b[0].path));
    for (index, group) in groups.into_iter().enumerate() {
        for found in group {
            progress.add(Found { group: Some(index + 1), ..found });
        }
    }
}

/// Every group split by `how`, groups of one left out; stops when cancelled.
fn split(
    groups: Vec<Vec<Found>>,
    progress: &Progress,
    how: impl Fn(Vec<Found>, &Progress) -> Vec<Vec<Found>>,
) -> Vec<Vec<Found>> {
    let mut result = Vec::new();
    for group in groups {
        if progress.is_cancelled() {
            return Vec::new();
        }
        result.extend(how(group, progress).into_iter().filter(|group| group.len() > 1));
    }
    result
}

fn hash(path: &Path) -> io::Result<blake3::Hash> {
    let mut hasher = blake3::Hasher::new();
    hasher.update_reader(File::open(path)?)?;
    Ok(hasher.finalize())
}

fn same_content(a: &Path, b: &Path) -> io::Result<bool> {
    let (mut a, mut b) = (File::open(a)?, File::open(b)?);
    let (mut block_a, mut block_b) = (vec![0; 1 << 16], vec![0; 1 << 16]);
    loop {
        let read = read_full(&mut a, &mut block_a)?;
        if read != read_full(&mut b, &mut block_b)? || block_a[..read] != block_b[..read] {
            return Ok(false);
        }
        if read == 0 {
            return Ok(true);
        }
    }
}

/// Fills `block` unless the file ends first; how much was read.
fn read_full(file: &mut File, block: &mut [u8]) -> io::Result<usize> {
    let mut filled = 0;
    while filled < block.len() {
        match file.read(&mut block[filled..])? {
            0 => break,
            read => filled += read,
        }
    }
    Ok(filled)
}
