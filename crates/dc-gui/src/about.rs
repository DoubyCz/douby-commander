//! About window (`cm_About`, `TfrmAbout` of the original), as libadwaita
//! shows it: name, version, author, the tribute to Double Commander and
//! its authors, licenses, and the system details the original lists
//! (with "Copy to clipboard"), here under Troubleshooting.

use dc_i18n::{fill, gettext};

use crate::texts;

/// Where Douby Commander is at home.
const WEBSITE: &str = "https://douby.cz";
const REPOSITORY: &str = "https://github.com/DoubyCz/douby-commander";
const ISSUES: &str = "https://github.com/DoubyCz/douby-commander/issues";
const ORIGINAL: &str = "https://github.com/doublecmd/doublecmd";

/// Developers of Double Commander, as its About window lists them.
const ORIGINAL_DEVELOPERS: [&str; 2] = ["Alexander Koblov", "Rich Chang"];
const ORIGINAL_FORMER_DEVELOPERS: [&str; 4] =
    ["Denis Bisson", "Przemysław Nagay", "Dmitry Kolomiets", "Radek Červinka (Seksi Commander)"];

pub fn show(parent: Option<&gtk::Window>) {
    let version = env!("CARGO_PKG_VERSION");
    let about = adw::AboutDialog::builder()
        .application_name(texts::APP_NAME)
        .application_icon("cz.douby.DoubyCommander")
        .version(version)
        .developer_name("Jiří Doubravský")
        .developers(["Jiří Doubravský https://douby.cz"])
        .website(WEBSITE)
        .issue_url(ISSUES)
        .copyright("© 2026 Jiří Doubravský")
        .license_type(gtk::License::Gpl20)
        .comments(gettext(
            "A two-panel file manager for Linux. A tribute to Double Commander by Alexander Koblov: \
             a fork of it, completely rewritten from Free Pascal into Rust.\n\n\
             Thank you, Alexander Koblov and all developers of Double Commander, for many years \
             of great work on a fantastic two-panel file manager.",
        ))
        .debug_info(debug_info(version))
        .build();
    about.add_link(&gettext("Source code"), REPOSITORY);
    about.add_link(&gettext("Double Commander"), ORIGINAL);
    about.add_credit_section(Some(&gettext("Double Commander, the original")), &ORIGINAL_DEVELOPERS);
    about.add_credit_section(Some(&gettext("Former developers of Double Commander")), &ORIGINAL_FORMER_DEVELOPERS);
    about.add_legal_section(
        "Double Commander",
        Some("© Alexander Koblov and the Double Commander developers"),
        gtk::License::Gpl20,
        Some(&gettext("The design, the keyboard shortcuts and the icons come from Double Commander.")),
    );
    adw::prelude::AdwDialogExt::present(&about, parent);
}

/// What the original shows next to its logo: versions and the system,
/// for bug reports.
fn debug_info(version: &str) -> String {
    let commit = option_env!("DC_COMMIT").filter(|commit| !commit.is_empty()).unwrap_or("-");
    let os = std::fs::read_to_string("/etc/os-release")
        .ok()
        .and_then(|text| {
            text.lines().find_map(|line| line.strip_prefix("PRETTY_NAME=").map(|name| name.trim_matches('"').to_owned()))
        })
        .unwrap_or_default();
    let kernel = std::fs::read_to_string("/proc/sys/kernel/osrelease").unwrap_or_default();
    let lines = [
        format!("{} {version} ({commit})", texts::APP_NAME),
        fill(&gettext("Platform: {platform}"), &[("platform", &format!("{}-linux", std::env::consts::ARCH))]),
        fill(&gettext("System: {system}"), &[("system", &format!("{os}, Linux {}", kernel.trim()))]),
        format!("GTK {}.{}.{}", gtk::major_version(), gtk::minor_version(), gtk::micro_version()),
        format!("libadwaita {}.{}.{}", adw::major_version(), adw::minor_version(), adw::micro_version()),
    ];
    lines.join("\n")
}
