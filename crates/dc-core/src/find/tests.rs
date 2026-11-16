use std::os::unix::fs::{PermissionsExt, symlink};

use jiff::civil::{date, time};

use super::*;

/// A tree to search:
///
/// ```text
/// docs/zprava.doc  docs/Zprava.txt  docs/old/plan.doc
/// src/main.rs  src/.hidden.rs  run.sh (755)  empty/
/// ```
fn tree() -> tempfile::TempDir {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    for dir in ["docs/old", "src", "empty"] {
        fs::create_dir_all(root.join(dir)).unwrap();
    }
    fs::write(root.join("docs/zprava.doc"), "výroční zpráva").unwrap();
    fs::write(root.join("docs/Zprava.txt"), "zpráva o stavu").unwrap();
    fs::write(root.join("docs/old/plan.doc"), "plán na rok").unwrap();
    fs::write(root.join("src/main.rs"), "fn main() {}\n").unwrap();
    fs::write(root.join("src/.hidden.rs"), "// skrytý\n").unwrap();
    fs::write(root.join("run.sh"), "#!/bin/sh\n").unwrap();
    fs::set_permissions(root.join("run.sh"), fs::Permissions::from_mode(0o755)).unwrap();
    tmp
}

/// Paths found by `query` under `root`, relative to it, in the order found.
fn find(root: &Path, query: &FindQuery) -> Vec<String> {
    find_in(root, query, Roots::Directories(vec![root.to_owned()]))
}

fn find_in(root: &Path, query: &FindQuery, roots: Roots) -> Vec<String> {
    let finder = Finder::new(query, roots, Timestamp::now(), TimeZone::UTC).unwrap();
    let progress = Progress::default();
    finder.run(&progress);
    progress
        .take_found()
        .into_iter()
        .map(|found| {
            let path = found.path.strip_prefix(root).unwrap().to_string_lossy().into_owned();
            match found.group {
                Some(group) => format!("{group}:{path}"),
                None => path,
            }
        })
        .collect()
}

fn masks(masks: &str) -> FindQuery {
    FindQuery { masks: masks.to_owned(), ..FindQuery::default() }
}

#[test]
fn names_by_masks_parts_and_expressions() {
    let tmp = tree();
    let root = tmp.path();
    assert_eq!(find(root, &masks("*.doc")), ["docs/old/plan.doc", "docs/zprava.doc"]);
    // A part of the name, in any case; hidden files too, as the original.
    assert_eq!(find(root, &masks("zprava")), ["docs/Zprava.txt", "docs/zprava.doc"]);
    assert_eq!(find(root, &masks("hidden")), ["src/.hidden.rs"]);
    fs::write(root.join("docs/Výroční zpráva.odt"), "").unwrap();
    assert_eq!(find(root, &masks("vyrocni")), ["docs/Výroční zpráva.odt"]);
    fs::remove_file(root.join("docs/Výroční zpráva.odt")).unwrap();
    assert_eq!(find(root, &FindQuery { partial_names: false, ..masks("zprava") }), Vec::<String>::new());
    // Nothing given: everything, directories too.
    assert_eq!(find(root, &FindQuery::default()).len(), 10);
    assert_eq!(find(root, &FindQuery { exclude_files: "*.doc;*.rs".to_owned(), ..masks("*") }).len(), 6);
    let regex = FindQuery { name_regex: true, ..masks(r"^[a-z]+\.(rs|sh)$") };
    assert_eq!(find(root, &regex), ["run.sh", "src/main.rs"]);
    let bad = FindQuery { name_regex: true, ..masks("(") };
    assert!(matches!(
        Finder::new(&bad, Roots::Directories(Vec::new()), Timestamp::now(), TimeZone::UTC),
        Err(FindError::BadExpression { field: Field::Masks, .. })
    ));
}

#[test]
fn directories_depth_and_exclusions() {
    let tmp = tree();
    let root = tmp.path();
    assert_eq!(find(root, &FindQuery { depth: Some(0), ..masks("*.doc;*.sh") }), ["run.sh"]);
    assert_eq!(find(root, &FindQuery { depth: Some(1), ..masks("*.doc") }), ["docs/zprava.doc"]);
    assert_eq!(find(root, &FindQuery { exclude_dirs: "old".to_owned(), ..masks("*.doc") }), ["docs/zprava.doc"]);
    assert_eq!(find(root, &FindQuery { exclude_dirs: "docs/o*".to_owned(), ..masks("*.doc") }), ["docs/zprava.doc"]);
    let absolute = format!("{}/docs", root.display());
    assert_eq!(find(root, &FindQuery { exclude_dirs: absolute, ..masks("*.doc") }), Vec::<String>::new());
    assert!(matches!(
        Finder::new(&masks(""), Roots::Directories(vec![root.join("nowhere")]), Timestamp::now(), TimeZone::UTC),
        Err(FindError::NoDirectory(_))
    ));
}

#[test]
fn only_the_selected_items() {
    let tmp = tree();
    let root = tmp.path();
    let roots = Roots::Selected(vec![root.join("docs/old"), root.join("run.sh"), root.join("src/main.rs")]);
    assert_eq!(find_in(root, &masks("*.doc;*.sh"), roots), ["docs/old/plan.doc", "run.sh"]);
}

#[test]
fn links_followed_once() {
    let tmp = tree();
    let root = tmp.path();
    symlink(root.join("docs"), root.join("src/to-docs")).unwrap();
    symlink(root, root.join("docs/old/loop")).unwrap();
    // Not followed: the link itself is found by its name, nothing behind it.
    assert_eq!(find(root, &masks("*.doc")), ["docs/old/plan.doc", "docs/zprava.doc"]);
    assert_eq!(find(root, &masks("to-docs")), ["src/to-docs"]);
    // Followed: what is behind it once, a cycle not at all.
    let followed = find(root, &FindQuery { follow_links: true, ..masks("*.doc") });
    assert_eq!(followed, ["docs/old/plan.doc", "docs/zprava.doc"]);
}

#[test]
fn time_size_and_attributes() {
    let tmp = tree();
    let root = tmp.path();
    let set_time = |path: &str, when: Timestamp| {
        let file = fs::File::options().write(true).open(root.join(path)).unwrap();
        file.set_modified(SystemTime::from(when)).unwrap();
    };
    set_time("docs/zprava.doc", "2025-03-10T08:00:00Z".parse().unwrap());
    set_time("docs/old/plan.doc", "2024-12-31T23:00:00Z".parse().unwrap());
    let doc = masks("*.doc");
    assert_eq!(find(root, &FindQuery { date_from: Some(date(2025, 1, 1)), ..doc.clone() }), ["docs/zprava.doc"]);
    assert_eq!(find(root, &FindQuery { date_to: Some(date(2024, 12, 31)), ..doc.clone() }), ["docs/old/plan.doc"]);
    let morning = FindQuery { time_to: Some(time(12, 0, 0, 0)), ..doc.clone() };
    assert_eq!(find(root, &morning), ["docs/zprava.doc"]);
    let moment = FindQuery { date_to: Some(date(2025, 3, 10)), time_to: Some(time(7, 0, 0, 0)), ..doc.clone() };
    assert_eq!(find(root, &moment), ["docs/old/plan.doc"]);
    let recent = FindQuery { age: Some(Age { amount: 1, unit: TimeUnit::Hour }), ..masks("*") };
    assert!(!find(root, &recent).contains(&"docs/zprava.doc".to_owned()));
    assert!(find(root, &recent).contains(&"src/main.rs".to_owned()));

    // "výroční zpráva" has 18 bytes, "zpráva o stavu" 15; directories have no size.
    let sized = FindQuery { size_from: Some(15), size_unit: SizeUnit::Bytes, ..masks("*") };
    assert_eq!(find(root, &sized), ["docs/Zprava.txt", "docs/zprava.doc"]);
    assert_eq!(find(root, &FindQuery { size_to: Some(0), size_unit: SizeUnit::Kilo, ..masks("*") }), Vec::<String>::new());

    assert_eq!(find(root, &FindQuery { attributes: "r+ux+".to_owned(), ..masks("*") }), ["run.sh"]);
    assert_eq!(find(root, &FindQuery { attributes: "d+".to_owned(), ..masks("*") }), ["docs", "docs/old", "empty", "src"]);
}

#[test]
fn text_inside_files() {
    let tmp = tree();
    let root = tmp.path();
    let mut query = masks("*");
    query.text = TextQuery { enabled: true, text: "ZPRÁVA".to_owned(), ..TextQuery::default() };
    assert_eq!(find(root, &query), ["docs/Zprava.txt", "docs/zprava.doc"]);
    query.text.not_containing = true;
    assert_eq!(find(root, &query), ["docs/old/plan.doc", "run.sh", "src/.hidden.rs", "src/main.rs"]);
}

#[test]
fn names_inside_archives() {
    let tmp = tree();
    let root = tmp.path();
    let status = std::process::Command::new("tar")
        .args(["-cf", "docs.tar", "docs"])
        .current_dir(root)
        .status()
        .unwrap();
    assert!(status.success());
    assert_eq!(find(root, &masks("plan")), ["docs/old/plan.doc"]);
    let archives = FindQuery { in_archives: true, ..masks("plan") };
    assert_eq!(find(root, &archives), ["docs/old/plan.doc", "docs.tar/docs/old/plan.doc"]);
}

#[test]
fn duplicates_in_groups() {
    let tmp = tree();
    let root = tmp.path();
    fs::write(root.join("docs/old/zprava.doc"), "jiná, starší zpráva").unwrap();
    fs::write(root.join("src/copy.txt"), "výroční zpráva").unwrap();
    fs::write(root.join("src/same-size.txt"), "výroční zprávy").unwrap();
    let by = |duplicates: Duplicates| FindQuery { duplicates: Some(duplicates), ..masks("*") };
    let name = find(root, &by(Duplicates { name: true, ..Duplicates::default() }));
    assert_eq!(name, ["1:docs/old/zprava.doc", "1:docs/zprava.doc"]);
    let size = find(root, &by(Duplicates { size: true, ..Duplicates::default() }));
    assert_eq!(size, ["1:docs/zprava.doc", "1:src/copy.txt", "1:src/same-size.txt"]);
    for check in [Duplicates { size: true, hash: true, ..Duplicates::default() }, Duplicates {
        size: true,
        content: true,
        ..Duplicates::default()
    }] {
        assert_eq!(find(root, &by(check)), ["1:docs/zprava.doc", "1:src/copy.txt"]);
    }
    assert!(matches!(
        Finder::new(&by(Duplicates::default()), Roots::Directories(Vec::new()), Timestamp::now(), TimeZone::UTC),
        Err(FindError::DuplicatesHowUnknown)
    ));
}

#[test]
fn cancelled_search_stops() {
    let tmp = tree();
    let finder = Finder::new(&masks("*"), Roots::Directories(vec![tmp.path().to_owned()]), Timestamp::now(), TimeZone::UTC)
        .unwrap();
    let progress = Progress::default();
    progress.cancel();
    finder.run(&progress);
    assert_eq!(progress.found_count(), 0);
}
