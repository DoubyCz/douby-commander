//! - Bundles `resources/` (icons) into a `GResource`, see `resources.gresource.xml`.
//! - Compiles the translations `po/<lang>.po` of the workspace into
//!   `$OUT_DIR/locale/<lang>/LC_MESSAGES/doubycmd.mo` and tells the
//!   program where they are (`DC_LOCALE_DIR`).

use std::path::{Path, PathBuf};
use std::process::Command;
use std::{env, fs};

const DOMAIN: &str = "doubycmd";

fn main() {
    glib_build_tools::compile_resources(&["resources"], "resources/resources.gresource.xml", "dc-gui.gresource");
    compile_translations();
    commit();
}

/// The commit built, for the About window (`DC_COMMIT`); empty outside git.
fn commit() {
    let git_dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../.git");
    println!("cargo::rerun-if-changed={}", git_dir.join("HEAD").display());
    println!("cargo::rerun-if-changed={}", git_dir.join("refs/heads").display());
    let commit = Command::new("git")
        .args(["rev-parse", "--short", "HEAD"])
        .current_dir(env!("CARGO_MANIFEST_DIR"))
        .output()
        .ok()
        .filter(|output| output.status.success())
        .map(|output| String::from_utf8_lossy(&output.stdout).trim().to_owned())
        .unwrap_or_default();
    println!("cargo::rustc-env=DC_COMMIT={commit}");
}

fn compile_translations() {
    let po_dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../po");
    let locale_dir = PathBuf::from(env::var("OUT_DIR").expect("cargo sets OUT_DIR")).join("locale");
    println!("cargo::rerun-if-changed={}", po_dir.display());
    println!("cargo::rustc-env=DC_LOCALE_DIR={}", locale_dir.display());

    let Ok(entries) = fs::read_dir(&po_dir) else { return };
    for po in entries.filter_map(Result::ok).map(|entry| entry.path()) {
        if po.extension().is_none_or(|ext| ext != "po") {
            continue;
        }
        let language = po.file_stem().expect("a .po file has a name").to_string_lossy().into_owned();
        let target_dir = locale_dir.join(&language).join("LC_MESSAGES");
        fs::create_dir_all(&target_dir).expect("cannot create the locale directory in OUT_DIR");

        let mo = target_dir.join(format!("{DOMAIN}.mo"));
        let status = Command::new("msgfmt").arg("--check").arg("-o").arg(&mo).arg(&po).status();
        match status {
            Ok(status) if status.success() => {}
            Ok(status) => panic!("msgfmt failed on {} ({status})", po.display()),
            Err(err) => println!("cargo::warning=msgfmt not available ({err}), translations are not built"),
        }
    }
}
