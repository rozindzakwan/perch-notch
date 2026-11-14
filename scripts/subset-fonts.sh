#!/usr/bin/env bash
# Subset the bundled fonts to the characters Perch draws, so fontdue (which
# builds every glyph's outline at load time) does not hold thousands of
# OpenType alternates it can never reach without shaping.
#
# Inputs are the original files (Inter 4.1 release TTFs, DejaVu Sans 2.37),
# outputs replace crates/ui/assets/. Needs fontTools (`pyftsubset`):
#   apt-get install python3-fonttools   or   pip install fonttools
# Usage: scripts/subset-fonts.sh <dir with Inter-*.ttf and DejaVuSans.ttf>
set -euo pipefail
SRC=${1:?directory with the original Inter-*.ttf and DejaVuSans.ttf}
OUT=$(dirname "$0")/../crates/ui/assets
command -v pyftsubset >/dev/null || pyftsubset() { python3 -m fontTools.subset "$@"; }

# Inter: Latin (incl. Vietnamese), Greek, Cyrillic, combining marks,
# punctuation, currency, letterlike, arrows, math, geometric shapes.
INTER=U+0000-024F,U+0250-02FF,U+0300-036F,U+0370-03FF,U+0400-052F,U+1E00-1EFF,U+2000-206F,U+2070-209F,U+20A0-20CF,U+2100-214F,U+2150-218F,U+2190-21FF,U+2200-22FF,U+2300-23FF,U+2460-24FF,U+25A0-25FF,U+2600-26FF,U+2700-27BF,U+FB00-FB06,U+FEFF,U+FFFD
for w in Regular Medium Bold; do
    pyftsubset "$SRC/Inter-$w.ttf" --unicodes="$INTER" \
        --layout-features='' --no-hinting --desubroutinize \
        --name-IDs='*' --name-languages='*' --notdef-outline \
        --output-file="$OUT/Inter-$w.ttf"
done

# DejaVu is only the fallback for what Inter lacks, in two files so that an
# emoji does not pull in every other script: common symbols and emoji, and
# other scripts plus rare symbols (Armenian, Hebrew, Arabic, Georgian, box
# drawing, braille, math alphanumerics; Arabic presentation forms
# are left out, they are only reachable through shaping). The script ranges
# must match `TextRenderer::fallback_for`.
SYMBOLS=U+0000-001F,U+2000-24FF,U+25A0-27BF,U+2900-2BFF,U+3000-303F,U+FFF0-FFFF,U+1F000-1F0FF,U+1F300-1F6FF,U+1F900-1F9FF
SCRIPTS=U+0000-001F,U+0530-1DFF,U+1F00-1FFF,U+2500-259F,U+27C0-28FF,U+2C00-2E7F,U+A640-A6FF,U+FB13-FB4F,U+FE20-FE2F,U+1D400-1D7FF
for part in Symbols:$SYMBOLS Scripts:$SCRIPTS; do
    pyftsubset "$SRC/DejaVuSans.ttf" --unicodes="${part#*:}" \
        --layout-features='' --no-hinting \
        --name-IDs='*' --name-languages='*' --notdef-outline \
        --output-file="$OUT/Perch${part%%:*}.ttf"
done

ls -l "$OUT"/*.ttf
sha256sum "$OUT"/*.ttf
