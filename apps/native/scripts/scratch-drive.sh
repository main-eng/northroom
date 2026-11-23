#!/bin/sh
# Makes a small drive for checks that run the app: nested and empty folders, media, Markdown
# pages, a tagged picture, a long file name, code, plain text and a shell terminal. Every byte
# and date is identical on every run and platform.
#
# Media files are checked in under drive-media because each ffmpeg version writes different
# bytes, and the list view shows file sizes. Made with ffmpeg 8.0.1 from lavfi sources:
#   gradient.png  gradients=s=1280x720:c0=0x1e88e5:c1=0xe53935:x0=0:y0=0:x1=1280:y1=720:seed=7
#   portrait.png  gradients=s=800x1000:c0=0x43a047:c1=0xfdd835:x0=0:y0=0:x1=800:y1=1000:seed=7
#   chart.jpg     testsrc2=s=1024x576
#   square.png    gradients=s=600x600:c0=0x8e24aa:c1=0x00acc1:x0=0:y0=0:x1=600:y1=600:seed=7
#   deepest.png   color=c=0xff7043:s=640x360            (each a single frame: -frames:v 1)
#   clip.mp4      testsrc2=s=1280x720:r=30:d=4 and sine=f=440:d=4, -c:v libx264 -pix_fmt yuv420p
#                 -c:a aac -shortest
#   tone.wav      sine=f=330:d=3
#
# Usage: scripts/scratch-drive.sh <folder>      (needs python3 on a Mac, for the tags)
set -eu
drive=${1:?usage: scratch-drive.sh <folder>}
media=$(cd "$(dirname "$0")/drive-media" && pwd)
mkdir -p "$drive/Assets/Pictures/Deep" "$drive/Notes" "$drive/Empty folder"
cd "$drive"
cp "$media/gradient.png" "$media/clip.mp4" "$media/tone.wav" .
cp "$media/portrait.png" "$media/chart.jpg" Assets/
cp "$media/square.png" Assets/Pictures/
cp "$media/deepest.png" Assets/Pictures/Deep/
echo "plain text" > "A very long file name that keeps going and going so every view has to truncate it somewhere.txt"
cp gradient.png tagged.png
# Finder tags "Red" (colour 6) and "Hero". Only a Mac keeps them.
if [ "$(uname)" = Darwin ]; then
python3 - <<'PY'
import plistlib, subprocess
tags = plistlib.dumps(["Red\n6", "Hero"], fmt=plistlib.FMT_BINARY)
subprocess.run(["xattr", "-wx", "com.apple.metadata:_kMDItemUserTags", tags.hex(), "tagged.png"], check=True)
PY
fi
printf '# Plan\n\nA short page inside a folder.\n\n- first\n- second\n' > Notes/Plan.md
# The other card kinds: source code, a text file with no extension, and a terminal that runs
# the plain shell, never an agent.
mkdir -p Extras
printf 'def greet(name):\n\treturn f"hello {name}"\n\n\nif __name__ == "__main__":\n\tprint(greet("world"))\n' > Extras/script.py
printf 'First paragraph of a file\nwith no extension.\n\nSecond paragraph.\n' > Extras/README
printf '{\n  "terminal": 1\n}\n' > Extras/Shell.nterm
# A Rust card shows the top of the code, and all of it when selected or opened.
printf 'use std::fmt;\n\n/// Greets by name.\npub fn greet(name: &str) -> String {\n    format!("hello {name}")\n}\n\nfn main() {\n    let count: u32 = 3;\n    for _ in 0..count {\n        println!("{}", greet("world"));\n    }\n}\n' > Extras/main.rs
cat > "All blocks.md" <<'PAGE'
# Heading one

A paragraph with **bold**, *italic*, `code`, ~~strike~~ and a [link](https://example.com).

## Heading two

### Heading three

- bullet one
- bullet two

1. numbered one
2. numbered two

- [ ] to do open
- [x] to do done

> A quote that someone said.

```
let code = "block";
```

---

| Name | Value |
| --- | --- |
| alpha | 1 |
| beta | 2 |

![gradient](gradient.png)

Last paragraph.
PAGE
# One fixed past date for everything, so the app shows the same text on any day
# ("Mar 14, 2024 at 9:26 AM", never "Today" or "Yesterday").
find . -exec touch -t 202403140926.00 {} +
echo "$drive"
