# Douby Commander

![Douby Commander in six Omarchy themes](docs/poster.webp)

A two-panel file manager for Linux, written in Rust with GTK 4.

Douby Commander is a tribute to [Double Commander](https://github.com/doublecmd/doublecmd),
the great open-source file manager inspired by Total Commander. It rewrites Double Commander
from Free Pascal into fast, idiomatic Rust, step by step: not a line-by-line port, but true
to its look, its keyboard shortcuts and how it feels to use. The "D" of Double stays in
the name and the abbreviation DC.

It follows the theme of [Omarchy](https://omarchy.org/) live: switch the theme and the
colors change at once, with no setup.

> **Alpha (0.0.1).** Many features are not done yet. Most menu items and toolbar buttons
> are not connected, and there is no settings dialog. Expect rough edges.

## What works

- Two panels, keyboard driven: Tab, quick search by typing, marking with Insert, Space
  and Shift
- F3 opens a file in your terminal editor (`$VISUAL` or `$EDITOR`), F4 in your desktop
  text editor, Enter in the default application
- F5 copy and F6 move, with the questions and options of the original, a progress
  window, pause, and queues
- F7 new directory, F8 delete (to the trash, or permanently with Shift)
- Adjustable panel divider, drive list with free space
- English and Czech

## Installing

There are no packages yet, so build from source. You need Rust 1.92 or newer, GTK 4.14
or newer with development files, and gettext. First install them for your distribution.

Arch Linux (Omarchy):

```sh
sudo pacman -S --needed base-devel rust gtk4 gettext
```

Debian 13, Ubuntu 24.04 and newer:

```sh
sudo apt install build-essential pkg-config libgtk-4-dev gettext
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh
```

Fedora, RHEL 10 and derivatives:

```sh
sudo dnf install gcc pkgconf-pkg-config gtk4-devel gettext
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh
```

Then, the same everywhere:

```sh
git clone https://github.com/DoubyCz/douby-commander.git
cd douby-commander
./install.sh
```

This builds it and installs it for you into `~/.local`, with a launcher entry and an icon.
For all users: `cargo build --release && sudo PREFIX=/usr/local ./install.sh --no-build`.
`./install.sh --uninstall` removes it (with the same `PREFIX`).

## License

GPL-2.0-or-later, as Double Commander. Icons come from Double Commander.
