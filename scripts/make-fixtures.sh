#!/usr/bin/env bash
# Generates the tiny test fixtures in crates/engines/tests/fixtures.
#
# We make every fixture ourselves with the installed tools, so we own all of
# them and nobody has to wonder where a sample.jpg came from. Safe to re-run:
# every file is overwritten, never appended to. Not byte-for-byte
# reproducible though, because magick and soffice love stamping the current
# time into their output. Run it once on the dev machine and commit the result;
# CI never regenerates fixtures.
#
# Needs: magick (ImageMagick 7), ffmpeg, soffice (LibreOffice).
set -euo pipefail

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
out="$repo_root/crates/engines/tests/fixtures"

for tool in magick ffmpeg soffice; do
    if ! command -v "$tool" >/dev/null 2>&1; then
        echo "error: $tool not found; install it and run this script again" >&2
        exit 1
    fi
done

mkdir -p "$out"

# Scratch space for soffice output and its throwaway profile, so we never touch
# the user's real LibreOffice settings (or fight a running instance for them).
work="$(mktemp -d)"
trap 'rm -rf "$work"' EXIT

echo "sample.png"
# 64x48 RGBA gradient; the alpha ramp makes sure "A" in RGBA actually means something.
magick -size 64x48 gradient:'#ff4000-#0040ff' \
    \( -size 64x48 gradient:white-gray25 \) \
    -alpha off -compose CopyOpacity -composite \
    -depth 8 -define png:color-type=6 -strip "$out/sample.png"

echo "sample.jpg"
magick -seed 42 -size 64x48 plasma:fractal -quality 85 -strip "$out/sample.jpg"

echo "sample.gif"
# Three 32x32 frames: red, green, blue. A traffic light that took a wrong turn.
magick -delay 20 -size 32x32 xc:red xc:green xc:blue -loop 0 "$out/sample.gif"

echo "sample.wav"
# 1 s of 440 Hz at 8 kHz mono, about 16 KB. Concert pitch on a budget.
ffmpeg -hide_banner -loglevel error -y \
    -f lavfi -i "sine=frequency=440:sample_rate=8000:duration=1" \
    -ac 1 -c:a pcm_s16le -map_metadata -1 -bitexact "$out/sample.wav"

echo "sample.mp4"
ffmpeg -hide_banner -loglevel error -y \
    -f lavfi -i "testsrc=size=64x64:rate=25:duration=1" \
    -f lavfi -i "sine=frequency=440:sample_rate=44100:duration=1" \
    -c:v libx264 -pix_fmt yuv420p -preset veryfast -crf 30 \
    -c:a aac -b:a 32k -shortest -map_metadata -1 -movflags +faststart \
    "$out/sample.mp4"

echo "sample.txt"
# Hangul and an emoji keep every engine honest about UTF-8.
printf '%s\n' 'Hello from MorphRing.' '안녕하세요 세계 🌍' > "$out/sample.txt"

echo "sample.docx, sample.odt"
profile="file://$work/lo-profile"
for fmt in docx odt; do
    soffice -env:UserInstallation="$profile" --headless --norestore \
        --convert-to "$fmt" --outdir "$work" "$out/sample.txt" >/dev/null 2>&1
    # soffice exit codes are fiction; the output file is the only truth.
    if [[ ! -s "$work/sample.$fmt" ]]; then
        echo "error: soffice did not produce sample.$fmt" >&2
        exit 1
    fi
    mv -f "$work/sample.$fmt" "$out/sample.$fmt"
done

echo "notes.unknownext"
printf '%s\n' 'No engine knows what this is, and that is the point.' > "$out/notes.unknownext"

echo "done: fixtures written to $out"
