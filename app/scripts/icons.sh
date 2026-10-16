#!/usr/bin/env bash
# Regenerates every app icon from app/src-tauri/icons/icon.svg.
# Needs ImageMagick 7 (with the rsvg delegate) and python3.
set -euo pipefail

dir="$(cd "$(dirname "$0")/../src-tauri/icons" && pwd)"
svg="$dir/icon.svg"

png() { magick -background none -density 384 "$svg" -resize "$1x$1" "PNG32:$2"; }

png 32 "$dir/32x32.png"
png 128 "$dir/128x128.png"
png 256 "$dir/128x128@2x.png"
png 512 "$dir/icon.png"
magick -background none -density 384 "$svg" -define icon:auto-resize=256,128,64,48,32,16 "$dir/icon.ico"

# ImageMagick reads ICNS but won't write it. The format is just a header plus
# tagged PNGs, so a dozen lines of Python do the job.
python3 - "$dir" <<'PY'
import struct, subprocess, sys, tempfile, os
d = sys.argv[1]
chunks = b""
for tag, size in ((b"ic07", 128), (b"ic08", 256), (b"ic09", 512)):
    with tempfile.TemporaryDirectory() as t:
        out = os.path.join(t, "i.png")
        subprocess.run(["magick", "-background", "none", "-density", "384",
                        os.path.join(d, "icon.svg"), "-resize", f"{size}x{size}",
                        "PNG32:" + out], check=True)
        data = open(out, "rb").read()
    chunks += tag + struct.pack(">I", len(data) + 8) + data
open(os.path.join(d, "icon.icns"), "wb").write(b"icns" + struct.pack(">I", len(chunks) + 8) + chunks)
PY
# The macOS menu bar wants a template image: black plus alpha, nothing else,
# and macOS repaints it for light and dark bars. So: drop the purple tile and
# the shading arc, paint the ring black and zoom the viewBox onto the ring,
# because an 18-point menu bar has no pixels to spare on padding.
template() {
    sed -e '/<rect /d' \
        -e '/stroke="#5F59A6"/,/\/>/d' \
        -e 's/#FFFFFF/#000000/g' \
        -e 's/viewBox="0 0 512 512" width="512" height="512"/viewBox="72 72 368 368" width="368" height="368"/' \
        "$svg" |
        magick -background none -density 384 svg:- -resize "$1x$1" -channel RGB -evaluate set 0 +channel "PNG32:$2"
}
template 18 "$dir/tray-template.png"
template 36 "$dir/tray-template@2x.png"

echo "icons written to $dir"
