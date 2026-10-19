#!/bin/bash
# Extracts translatable strings into po/doubycmd.pot and merges them
# into every po/<lang>.po. Run after changing texts, then translate.

set -euo pipefail
cd "$(dirname "$0")/.."

po/extract.sh po/doubycmd.pot

for po in po/*.po; do
    [ -e "$po" ] || continue
    msgmerge --quiet --update --backup=none --no-fuzzy-matching "$po" po/doubycmd.pot
    # Texts no longer in the code.
    msgattrib --no-obsolete --output-file="$po" "$po"
done

echo "po/doubycmd.pot: $(grep -c '^msgid ' po/doubycmd.pot) strings"
