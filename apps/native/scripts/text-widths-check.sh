#!/bin/sh
# Compares the core's typesetter (core/src/text/typeface.rs, typeset.rs) with Core Text. Measures
# every word and sentence from the scratch drive and the Latin-script UI languages in every face
# the app uses, plus wrapped and unwrapped line counts. Widths must agree within 0.001 pt and
# line counts exactly.
#
# Core Text's measurements come from text-widths (gpui/mac-tools), built under the build lock
# and copied to the scratch folder.
#
# Usage: scripts/text-widths-check.sh [<scratch folder>]   (from apps/native; a Mac with
# python3)
set -eu
here=$(cd "$(dirname "$0")/.." && pwd)
scratch=${1:-$(mktemp -d)}
mkdir -p "$scratch"
scratch=$(cd "$scratch" && pwd -P)
"$here/scripts/with-build-lock.sh" sh -c '
  set -eu
  cd "$1/gpui"
  cargo build --release -j 4 -p mac-tools --bin text-widths
  cp target/release/text-widths "$2/text-widths"
' text-widths "$here" "$scratch"
drive="$scratch/drive"
rm -rf "$drive"
mkdir -p "$drive"
sh "$here/scripts/scratch-drive.sh" "$drive" >/dev/null
python3 - "$here" "$drive" > "$scratch/cases.txt" <<'PY'
import os, re, sys
here, drive = sys.argv[1], sys.argv[2]
texts = set()
# Text a page or card displays.
shown = set()
def quoted_in(path):
    source = open(path, encoding="utf-8").read()
    for quoted in re.findall(r'"((?:[^"\\\n]|\\.){1,200})"|\'((?:[^\'\\\n]|\\.){1,200})\'', source):
        yield (quoted[0] or quoted[1]).replace("\\n", "\n").replace("\\t", "\t")
# Names and file contents from the drive, plus the strings in the script that makes it.
texts.update(quoted_in(os.path.join(here, "scripts/scratch-drive.sh")))
for folder, folders, files in os.walk(drive):
    for name in folders + files:
        texts.add(name)
        shown.add(name)
    for name in files:
        path = os.path.join(folder, name)
        if os.path.getsize(path) <= 65536:
            try:
                words = open(path, encoding="utf-8").read()
            except UnicodeDecodeError:
                continue
            texts.add(words)
            shown.add(words)
# UI strings in the Latin-script languages (core/strings).
import json
for language in ("en", "es", "pt", "fr", "de", "it", "nl", "pl", "sv", "cs", "tr", "id", "vi"):
    catalog = json.load(open(os.path.join(here, "core/strings", f"{language}.json"), encoding="utf-8"))
    texts.update(catalog.values())
    shown.update(catalog.values())
words = set()
sentences = set()
for text in texts:
    for line in text.split("\n"):
        if line.strip():
            sentences.add(line.strip())
        words.update(word for word in re.split(r"\s+", line) if word)
words = sorted(words)
sentences = sorted(sentence for sentence in sentences if len(sentence) > 12)
# Pages and cards wrap text, so test their lines wrapped.
wrapped = sorted({line.strip() for text in shown for line in text.split("\n") if len(line.strip()) > 12})
def escape(text):
    return text.replace("\n", "\\n").replace("\t", "\\t")
sans_sizes = [9, 10, 11, 12, 13, 14, 15, 16, 17, 18, 20, 24, 28, 30, 32]
weights = [400, 500, 600, 650, 700]
fonts = []
for size in sans_sizes:
    for weight in weights:
        for tabular in (0, 1):
            for slanted in (0, 1):
                fonts.append(f"{size}\t{weight}\t0\t{tabular}\t{slanted}")
for size in [8.5, 10, 11, 12, 12.75, 13, 14.45, 17, 20.4, 25.5]:
    for slanted in (0, 1):
        fonts.append(f"{size}\t400\t1\t0\t{slanted}")
for font in fonts:
    for word in words:
        print(f"W\t{font}\t0\t{escape(word)}")
# Whole sentences on one line, and with the page title's tracking.
for font in ("12\t400\t0\t0\t0", "14\t500\t0\t0\t0", "15\t400\t0\t0\t1", "32\t700\t0\t0\t0", "13\t400\t1\t0\t0"):
    for sentence in sentences:
        print(f"W\t{font}\t0\t{escape(sentence)}")
for sentence in sentences:
    print(f"W\t32\t700\t0\t0\t0\t-0.32\t{escape(sentence)}")
# Sentences wrapped at card, dialog and page widths.
for font in ("12\t400\t0\t0\t0", "14\t400\t0\t0\t0", "15\t400\t0\t0\t0", "15\t600\t0\t0\t1", "12.75\t400\t1\t0\t0"):
    for width in (60, 96, 150, 220, 320, 500):
        for sentence in wrapped:
            print(f"L\t{font}\t{width}\t1\t0\t{escape(sentence)}")
# Code and plain text files whole, with line ends and tabs kept.
for text in sorted(shown):
    if "\n" in text or "\t" in text:
        for font in ("12.75\t400\t1\t0\t0", "13\t400\t0\t0\t0"):
            for tabs in (0, 4):
                print(f"L\t{font}\t400\t0\t{tabs}\t{escape(text)}")
                print(f"L\t{font}\t180\t1\t{tabs}\t{escape(text)}")
PY
echo "cases: $(wc -l < "$scratch/cases.txt")"
"$scratch/text-widths" "$here/core/assets/fonts" < "$scratch/cases.txt" > "$scratch/answers.txt"
cd "$here/core"
NR_TEXT_WIDTHS="$scratch/answers.txt" ../scripts/with-build-lock.sh cargo test --release -j 4 --lib \
  typeset::tests::every_case_sets_as_core_text_sets_it -- --ignored --nocapture
