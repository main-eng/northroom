/**
 * The Mac's own tools the sample files are made with, called the same way every time: headless
 * Chrome for designed pictures and the PDF, FFmpeg for media, and small fixes that pin the dates
 * some tools write into their files so every run makes the same bytes.
 */
import { mkdirSync, readFileSync, writeFileSync } from "node:fs";
import path from "node:path";

export const CHROME = "/Applications/Google Chrome.app/Contents/MacOS/Google Chrome";

/** Runs `command` and returns what it printed; a failure stops the run with what it said. */
export function run(
  command: string,
  args: readonly string[],
  options: { cwd?: string } = {},
): Buffer {
  const result = Bun.spawnSync({
    cmd: [command, ...args],
    cwd: options.cwd,
    stdout: "pipe",
    stderr: "pipe",
  });
  if (result.exitCode !== 0) {
    const said = result.stderr.toString().trim().split("\n").slice(-6).join("\n");
    throw new Error(`${path.basename(command)} failed (${result.exitCode}): ${said}`);
  }
  return result.stdout;
}

/** FFmpeg with its banner off, overwriting, and with no dates or encoder names in what it writes. */
export function ffmpeg(args: readonly string[]): void {
  run("ffmpeg", ["-hide_banner", "-loglevel", "error", "-y", ...args]);
}

/** Output options that keep FFmpeg's files the same from run to run. */
export const BITEXACT = [
  "-map_metadata",
  "-1",
  "-fflags",
  "+bitexact",
  "-flags:v",
  "+bitexact",
  "-flags:a",
  "+bitexact",
] as const;

/** A size in CSS pixels and the scale Chrome draws it at, so a picture is `width × scale` wide. */
export interface Viewport {
  widthPx: number;
  heightPx: number;
  scale: number;
}

/** Headless Chrome, run with its own throwaway profile in `work`. */
export class Chrome {
  private readonly pages: string;

  constructor(private readonly work: string) {
    this.pages = path.join(work, "pages");
    mkdirSync(this.pages, { recursive: true });
  }

  private launch(args: readonly string[]): Buffer {
    return run(CHROME, [
      "--headless=new",
      "--no-first-run",
      "--no-default-browser-check",
      "--disable-extensions",
      "--disable-background-networking",
      "--disable-component-update",
      "--hide-scrollbars",
      // Without it a busy machine can capture a heavy page before every tile is drawn.
      "--run-all-compositor-stages-before-draw",
      "--force-color-profile=srgb",
      "--font-render-hinting=none",
      `--user-data-dir=${path.join(this.work, "chrome-profile")}`,
      ...args,
    ]);
  }

  private page(name: string, html: string): string {
    const file = path.join(this.pages, `${name}.html`);
    writeFileSync(file, html);
    return `file://${file}`;
  }

  /** A PNG of `html` drawn at `viewport`. */
  screenshot(name: string, html: string, viewport: Viewport, out: string): void {
    const url = this.page(name, html);
    this.launch([
      `--window-size=${viewport.widthPx},${viewport.heightPx}`,
      `--force-device-scale-factor=${viewport.scale}`,
      "--virtual-time-budget=2000",
      `--screenshot=${out}`,
      url,
    ]);
  }

  /** A PDF of `html`, its pages as its `@page` rule sizes them, with no header or footer. */
  printPdf(name: string, html: string, out: string): void {
    const url = this.page(name, html);
    this.launch([
      "--no-pdf-header-footer",
      "--virtual-time-budget=2000",
      `--print-to-pdf=${out}`,
      url,
    ]);
  }

  /** The PNG at `png` encoded as WebP at `quality` (0 to 1) by Chrome's canvas, which has the encoder. */
  webp(png: string, quality: number): Buffer {
    const source = `data:image/png;base64,${readFileSync(png).toString("base64")}`;
    const html = `<!doctype html><body><script>
const picture = new Image();
picture.onload = () => {
  const canvas = document.createElement("canvas");
  canvas.width = picture.naturalWidth;
  canvas.height = picture.naturalHeight;
  canvas.getContext("2d").drawImage(picture, 0, 0);
  document.body.textContent = canvas.toDataURL("image/webp", ${quality});
};
picture.src = "${source}";
</script></body>`;
    const dom = this.launch(["--virtual-time-budget=5000", "--dump-dom", this.page("webp", html)]);
    const encoded = /data:image\/webp;base64,([A-Za-z0-9+/=]+)/.exec(dom.toString())?.[1];
    if (encoded === undefined) throw new Error("Chrome did not encode the WebP");
    return Buffer.from(encoded, "base64");
  }
}

/** DOS time and date fields for `date`, as a zip's headers store them, in UTC. */
function dosDateTime(date: Date): { time: number; day: number } {
  const time =
    (date.getUTCHours() << 11) | (date.getUTCMinutes() << 5) | (date.getUTCSeconds() >> 1);
  const day =
    ((date.getUTCFullYear() - 1980) << 9) | ((date.getUTCMonth() + 1) << 5) | date.getUTCDate();
  return { time, day };
}

const ZIP_END_SIGNATURE = 0x06054b50;
const ZIP_CENTRAL_SIGNATURE = 0x02014b50;
const ZIP_LOCAL_SIGNATURE = 0x04034b50;
/** The extra field holding a Unix timestamp, which some zip writers add beside the DOS date. */
const EXTENDED_TIMESTAMP_FIELD = 0x5455;

/** Sets each extra field's Unix timestamps in `header` to `seconds`. */
function pinExtraTimestamps(zip: Buffer, start: number, length: number, seconds: number): void {
  for (let at = start; at + 4 <= start + length; ) {
    const id = zip.readUInt16LE(at);
    const size = zip.readUInt16LE(at + 2);
    if (id === EXTENDED_TIMESTAMP_FIELD) {
      for (let stamp = at + 5; stamp + 4 <= at + 4 + size; stamp += 4) {
        zip.writeUInt32LE(seconds, stamp);
      }
    }
    at += 4 + size;
  }
}

/** The zip in `zip` with every entry dated `date`, in place: its contents and layout are unchanged. */
export function pinZipDates(zip: Buffer, date: Date): Buffer {
  const { time, day } = dosDateTime(date);
  const seconds = Math.floor(date.getTime() / 1000);
  let end = zip.length - 22;
  while (end >= 0 && zip.readUInt32LE(end) !== ZIP_END_SIGNATURE) end -= 1;
  if (end < 0) throw new Error("not a zip: no end of central directory");
  const entries = zip.readUInt16LE(end + 10);
  let at = zip.readUInt32LE(end + 16);
  for (let entry = 0; entry < entries; entry += 1) {
    if (zip.readUInt32LE(at) !== ZIP_CENTRAL_SIGNATURE) throw new Error("bad zip directory");
    zip.writeUInt16LE(time, at + 12);
    zip.writeUInt16LE(day, at + 14);
    const nameLength = zip.readUInt16LE(at + 28);
    const extraLength = zip.readUInt16LE(at + 30);
    const commentLength = zip.readUInt16LE(at + 32);
    pinExtraTimestamps(zip, at + 46 + nameLength, extraLength, seconds);
    const local = zip.readUInt32LE(at + 42);
    if (zip.readUInt32LE(local) !== ZIP_LOCAL_SIGNATURE) throw new Error("bad zip entry");
    zip.writeUInt16LE(time, local + 10);
    zip.writeUInt16LE(day, local + 12);
    const localName = zip.readUInt16LE(local + 26);
    const localExtra = zip.readUInt16LE(local + 28);
    pinExtraTimestamps(zip, local + 30 + localName, localExtra, seconds);
    at += 46 + nameLength + extraLength + commentLength;
  }
  return zip;
}

/** The PDF in `pdf` with its creation and change dates set to `date`, each the same length as before. */
export function pinPdfDates(pdf: Buffer, date: Date): Buffer {
  const stamp = date.toISOString().replace(/[-:T]/g, "").slice(0, 14);
  const text = pdf.toString("latin1");
  const pinned = text.replace(
    /\/(CreationDate|ModDate) \(D:(\d{14})([^)]*)\)/g,
    (_, key, _d, zone) => `/${key} (D:${stamp}${zone})`,
  );
  if (pinned.length !== text.length) throw new Error("pinning the PDF's dates changed its length");
  return Buffer.from(pinned, "latin1");
}

/** The boxes inside which an MP4's dated headers sit. */
const MP4_CONTAINERS = new Set(["moov", "trak", "mdia"]);
/** The headers that carry a creation and a change time. */
const MP4_DATED = new Set(["mvhd", "tkhd", "mdhd"]);
/** Seconds from 1904, where MP4 counts time from, to 1970. */
const MP4_EPOCH_OFFSET_S = 2_082_844_800;

/** The MP4 or M4A in `file` with its creation and change times set to `date`, in place. */
export function pinMp4Dates(file: Buffer, date: Date): Buffer {
  const seconds = Math.floor(date.getTime() / 1000) + MP4_EPOCH_OFFSET_S;
  const walk = (start: number, end: number) => {
    for (let at = start; at + 8 <= end; ) {
      const size = file.readUInt32BE(at);
      const type = file.toString("latin1", at + 4, at + 8);
      if (size < 8) return;
      if (MP4_CONTAINERS.has(type)) walk(at + 8, at + size);
      if (MP4_DATED.has(type)) {
        const version = file.readUInt8(at + 8);
        if (version === 1) {
          file.writeBigUInt64BE(BigInt(seconds), at + 12);
          file.writeBigUInt64BE(BigInt(seconds), at + 20);
        } else {
          file.writeUInt32BE(seconds, at + 12);
          file.writeUInt32BE(seconds, at + 16);
        }
      }
      at += size;
    }
  };
  walk(0, file.length);
  return file;
}
