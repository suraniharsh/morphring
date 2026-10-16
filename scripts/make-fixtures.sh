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
# The PDF, EPS, PS, SVG, Markdown and archive fixtures further down are
# byte-stable: their sources are heredocs right here and the archives get a
# fixed mtime and owner. sample.pdf is the exception, LibreOffice stamps a
# creation date into it, so it is generated once and committed like the rest.
#
# Needs: magick (ImageMagick 7), ffmpeg, soffice (LibreOffice), bsdtar, gzip.
set -euo pipefail

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
out="$repo_root/crates/engines/tests/fixtures"

for tool in magick ffmpeg soffice bsdtar gzip; do
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
    # soffice stamps the current time into docx/odt, so regenerating them churns
    # git for nothing. They are generate-once: delete one, or set
    # MORPHRING_FIXTURES_FORCE=1, to make it again.
    if [[ -s "$out/sample.$fmt" && "${MORPHRING_FIXTURES_FORCE:-0}" != 1 ]]; then
        echo "  sample.$fmt exists, keeping it"
        continue
    fi
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

echo "sample.pdf"
# A flat ODT with one paragraph per page. LibreOffice embeds a subset of
# Liberation Sans with a ToUnicode map, so pdftotext gets the accents back,
# which a hand-written PostScript file with base-14 fonts never manages.
# No Hangul here: CI runners are not guaranteed to have a CJK font.
cat > "$work/sample-pdf.fodt" <<'FODT'
<?xml version="1.0" encoding="UTF-8"?>
<office:document xmlns:office="urn:oasis:names:tc:opendocument:xmlns:office:1.0"
    xmlns:style="urn:oasis:names:tc:opendocument:xmlns:style:1.0"
    xmlns:text="urn:oasis:names:tc:opendocument:xmlns:text:1.0"
    xmlns:fo="urn:oasis:names:tc:opendocument:xmlns:xsl-fo-compatible:1.0"
    xmlns:svg="urn:oasis:names:tc:opendocument:xmlns:svg-compatible:1.0"
    office:version="1.3" office:mimetype="application/vnd.oasis.opendocument.text">
  <office:font-face-decls>
    <style:font-face style:name="Liberation Sans" svg:font-family="'Liberation Sans'"
        style:font-family-generic="swiss" style:font-pitch="variable"/>
  </office:font-face-decls>
  <office:styles>
    <style:style style:name="Standard" style:family="paragraph">
      <style:text-properties style:font-name="Liberation Sans" fo:font-size="14pt"/>
    </style:style>
  </office:styles>
  <office:automatic-styles>
    <style:style style:name="PageBreak" style:family="paragraph" style:parent-style-name="Standard">
      <style:paragraph-properties fo:break-before="page"/>
    </style:style>
  </office:automatic-styles>
  <office:body>
    <office:text>
      <text:p text:style-name="Standard">Page 1</text:p>
      <text:p text:style-name="Standard">Café déjà vu</text:p>
      <text:p text:style-name="PageBreak">Page 2</text:p>
      <text:p text:style-name="PageBreak">Page 3</text:p>
    </office:text>
  </office:body>
</office:document>
FODT
soffice -env:UserInstallation="$profile" --headless --norestore \
    --convert-to pdf --outdir "$work" "$work/sample-pdf.fodt" >/dev/null 2>&1
if [[ ! -s "$work/sample-pdf.pdf" ]]; then
    echo "error: soffice did not produce sample.pdf" >&2
    exit 1
fi
mv -f "$work/sample-pdf.pdf" "$out/sample.pdf"

echo "sample.eps"
# An orange disc on nothing at all. EPS has no background unless you paint
# one, so "transparent" is free; gs renders it with -dEPSCrop and an alpha device.
cat > "$out/sample.eps" <<'EPS'
%!PS-Adobe-3.0 EPSF-3.0
%%Creator: MorphRing make-fixtures.sh
%%Title: sample.eps
%%BoundingBox: 0 0 100 80
%%HiResBoundingBox: 0 0 100 80
%%Pages: 1
%%EndComments
%%Page: 1 1
gsave
1 0.5 0 setrgbcolor
50 40 30 0 360 arc closepath fill
0 0.25 1 setrgbcolor
10 10 20 20 rectfill
grestore
showpage
%%EOF
EPS

echo "sample.ps"
cat > "$out/sample.ps" <<'PS'
%!PS-Adobe-3.0
%%Creator: MorphRing make-fixtures.sh
%%Title: sample.ps
%%BoundingBox: 0 0 612 792
%%Pages: 1
%%EndComments
%%Page: 1 1
0.2 0.6 0.3 setrgbcolor
72 600 200 120 rectfill
0 setgray
/Helvetica findfont 24 scalefont setfont
72 560 moveto (Hello from MorphRing) show
showpage
%%EOF
PS

echo "sample.svg, sample.svgz"
cat > "$out/sample.svg" <<'SVG'
<?xml version="1.0" encoding="UTF-8"?>
<svg xmlns="http://www.w3.org/2000/svg" width="120" height="80" viewBox="0 0 120 80">
  <rect x="4" y="4" width="112" height="72" rx="8" fill="#2b6cb0"/>
  <circle cx="34" cy="40" r="20" fill="#f6ad55"/>
  <text x="62" y="46" font-family="sans-serif" font-size="16" fill="#ffffff">Ring</text>
</svg>
SVG
# -n drops the name and mtime from the gzip header, so the bytes stay put.
gzip -9n -c "$out/sample.svg" > "$out/sample.svgz"

echo "sample.md"
cat > "$out/sample.md" <<'MD'
# MorphRing sample

A *tiny* Markdown file for the pandoc engine.

- drag a file
- pick a format
- done

See [MorphRing](https://github.com/suraniharsh/morphring) for more.
MD

echo "sample.zip, sample.tar.gz"
# Fixed mtime and anonymous owner make both archives byte-stable, so a re-run
# leaves git status clean instead of churning two binaries every time.
tree="$work/archive"
mkdir -p "$tree/dir"
printf '%s\n' 'Hello from the archive.' > "$tree/hello.txt"
printf '%s\n' 'Nested one level down.' > "$tree/dir/inner.txt"
touch -d @1700000000 "$tree/hello.txt" "$tree/dir/inner.txt" "$tree/dir"
rm -f "$out/sample.zip" "$out/sample.tar.gz"
owner=(--uid 0 --gid 0 --uname '' --gname '')
# Zip straight from disk would also record atime and ctime, and ctime is the one
# timestamp touch cannot fake. So we tar first (ustar keeps only mtime) and
# repack that stream as zip. TZ=UTC because zip's DOS timestamps are local
# time, and a fixture should not depend on where its author lives.
(cd "$tree" && bsdtar --format ustar "${owner[@]}" -cf "$work/sample.tar" hello.txt dir/inner.txt)
TZ=UTC bsdtar --format zip -cf "$out/sample.zip" @"$work/sample.tar"
gzip -9n -c "$work/sample.tar" > "$out/sample.tar.gz"

echo "done: fixtures written to $out"
