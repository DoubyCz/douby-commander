#!/bin/bash
# Extracts translatable strings of all crates into the given .pot file.
# Keywords: see crates/dc-i18n/src/lib.rs.

set -euo pipefail
cd "$(dirname "$0")/.."
output=$(realpath -m "$1")

mapfile -t sources < <(find crates -path '*/src/*.rs' | sort)

xgettext \
    --language=Rust \
    --from-code=UTF-8 \
    --add-comments=TRANSLATORS: \
    --keyword=gettext \
    --keyword=ngettext:1,2 \
    --keyword=pgettext:1c,2 \
    --keyword=npgettext:1c,2,3 \
    --keyword=N_ \
    --keyword=NC_:1c,2 \
    --package-name=doubycmd \
    --msgid-bugs-address=douby@douby.cz \
    --output="$output" \
    "${sources[@]}"
