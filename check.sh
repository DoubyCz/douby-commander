#!/bin/bash
# Everything that has to pass before a step counts as done.
# Changes nothing in the tree.

set -euo pipefail
cd "$(dirname "$0")"
export CARGO_BUILD_JOBS=${CARGO_BUILD_JOBS:-4}

step() { printf '\n== %s\n' "$1"; }

step "clippy (strict, warnings are errors)"
cargo clippy --workspace --all-targets --quiet -- -D warnings

step "clippy against the newest GTK API (deprecations)"
cargo clippy -p dc-gui --all-targets --features gtk-latest --quiet -- -D warnings

step "tests"
cargo test --workspace --quiet

step "build (so that ./target/debug/doubycmd is the checked code)"
cargo build --workspace --quiet

step "translations complete"
tmp=$(mktemp -d)
trap 'rm -rf "$tmp"' EXIT
po/extract.sh "$tmp/current.pot"
for po in po/*.po; do
    msgfmt --check --output-file=/dev/null "$po"
    # Every text of the code has a translation (msgcmp fails otherwise).
    msgcmp "$po" "$tmp/current.pot"
    echo "$po: complete"
done

step "dependencies: licenses and advisories"
if command -v cargo-deny >/dev/null; then
    cargo deny --log-level error check
else
    echo "cargo-deny is not installed (pacman -S cargo-deny), skipped"
fi

printf '\nAll checks passed.\n'
