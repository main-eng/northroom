/**
 * Writes a folder with one well-made sample file of every type the app shows, all telling one
 * story (Harbor Coffee launching a subscription), to prove each type opens and to show it off:
 * the types Northroom draws itself, the ones Quick Look draws inside it, and one nothing previews.
 * It uses only what a Mac has: Bun, FFmpeg, headless Chrome, sips, textutil, afconvert, say,
 * sqlite3 and usdzip. Every file is dated the same day, and a second run makes the same bytes
 * wherever the tools allow.
 *
 * Usage: bun apps/native/scripts/sample-files.ts <folder>
 *        (the folder must be new or empty)
 */
import {
  chmodSync,
  copyFileSync,
  existsSync,
  mkdirSync,
  mkdtempSync,
  readFileSync,
  readdirSync,
  rmSync,
  statSync,
  utimesSync,
  writeFileSync,
} from "node:fs";
import { tmpdir } from "node:os";
import path from "node:path";
import { deckFile, workbookFile, zipFile } from "../../web/scripts/office-files";
import { logoSvg } from "./sample-files/art";
import { DECK_PICTURE, pitchDeck } from "./sample-files/deck";
import {
  BUSINESS,
  COLOUR,
  COST,
  FORECAST,
  PLANS,
  STORY_DATE,
  planCostDollars,
  planMarginPercent,
} from "./sample-files/story";
import {
  LABEL,
  SCENE,
  STEAM,
  announcementScene,
  badgeScene,
  bagsScene,
  beansScene,
  boxScene,
  brandSheetScene,
  cupScene,
  labelScene,
  steamSheet,
} from "./sample-files/scenes";
import { doorBell, firstCrack, theme, wavFile } from "./sample-files/sound";
import {
  bagParts,
  beanMesh,
  glbFile,
  mugParts,
  objFile,
  plyFile,
  stlFile,
  usdaFile,
} from "./sample-files/models";
import {
  BITEXACT,
  Chrome,
  ffmpeg,
  pinMp4Dates,
  pinPdfDates,
  pinZipDates,
  run,
} from "./sample-files/tools";
import {
  codeFiles,
  contactCard,
  customersTsv,
  launchCalendar,
  letterHtml,
  notesText,
  ordersSql,
  planPage,
  priceListCsv,
  proposalHtml,
  subscribePage,
  SCHEMA_SQL,
} from "./sample-files/writing";

const FOLDER = {
  documents: "Documents",
  spreadsheets: "Spreadsheets",
  pictures: "Pictures",
  video: "Video",
  sound: "Sound",
  models: "3D",
  code: "Code",
  fonts: "Fonts",
  other: "Other",
} as const;

/** What each folder holds, for the README. */
const FOLDER_ABOUT: Record<string, string> = {
  [FOLDER.documents]: "The launch plan, a letter and the pitch, in every document format.",
  [FOLDER.spreadsheets]: "Prices, customers, the money model and the orders database.",
  [FOLDER.pictures]: "Product photographs and brand artwork, one per picture format.",
  [FOLDER.video]: "The 8-second promo film, one per video format.",
  [FOLDER.sound]: "Music, a door bell, the roaster and a voice memo, one per sound format.",
  [FOLDER.models]: "A mug, a bag and a bean, one per 3D format.",
  [FOLDER.code]: "The small programs and settings the roastery runs on, and a terminal.",
  [FOLDER.fonts]: "Typefaces copied from this Mac, one per font format.",
  [FOLDER.other]: "A contact card, a calendar and a zip that nothing previews.",
};

/** The terminal file the app makes for a new terminal (ADR-0005). */
const TERMINAL_FILE_TEXT = `${JSON.stringify({ terminal: 1 }, null, 2)}\n`;

/** System fonts copied into the folder at run time; font files are never kept in the repo. */
const FONTS = [
  {
    from: "/System/Library/Fonts/Optima.ttc",
    about: "Optima, the brand's display face (TrueType collection).",
  },
  {
    from: "/System/Library/Fonts/Supplemental/Georgia.ttf",
    about: "Georgia, the letter's serif (TrueType).",
  },
  {
    from: "/System/Library/Fonts/Supplemental/STIXGeneral.otf",
    about: "STIX General, a text serif (OpenType, CFF outlines).",
  },
] as const;

const VOICE = "Samantha";
const VOICE_MEMO =
  "Note for Saturday. The new burrs are in, so dial Morning Tide a little finer: eighteen grams in, thirty-six out, about twenty-eight seconds. " +
  "And remind Sam the subscribe page goes live on the twenty-first, so we need photos of all three bags by Wednesday.";

const FILM = { secondsPerShot: 2, fadeSeconds: 0.6, framesPerSecond: 30, scale: 1.2 } as const;

/** One file in the folder and what it shows, for the README. */
interface Written {
  path: string;
  about: string;
}

class SampleFolder {
  readonly written: Written[] = [];

  constructor(readonly root: string) {}

  /** The absolute path of `relative`, its folder made. */
  place(relative: string): string {
    const target = path.join(this.root, relative);
    mkdirSync(path.dirname(target), { recursive: true });
    return target;
  }

  write(relative: string, data: string | Uint8Array, about: string): void {
    writeFileSync(this.place(relative), data);
    this.note(relative, about);
  }

  /** Records a file another tool wrote at `relative`. */
  note(relative: string, about: string): void {
    if (!existsSync(path.join(this.root, relative))) throw new Error(`${relative} was not written`);
    this.written.push({ path: relative, about });
  }
}

const join = (...parts: string[]) => path.join(...parts);

function readTarget(): string {
  const folder = process.argv[2];
  if (folder === undefined || process.argv.length > 3) {
    console.error("usage: bun apps/native/scripts/sample-files.ts <folder>");
    process.exit(2);
  }
  const resolved = path.resolve(folder);
  if (existsSync(resolved) && readdirSync(resolved).length > 0) {
    console.error(`${resolved} is not empty; nothing was written`);
    process.exit(2);
  }
  mkdirSync(resolved, { recursive: true });
  return resolved;
}

/** Re-encodes `from` with FFmpeg into `to`, with `options` between them. */
const convert = (from: string, to: string, options: readonly string[]) =>
  ffmpeg(["-i", from, ...options, ...BITEXACT, to]);

/** The photographs, drawn once at film size; each still is the frame scaled to the picture size. */
function drawShots(chrome: Chrome, work: string) {
  const shots = {
    bags: bagsScene(),
    cup: cupScene(),
    box: boxScene(),
    beans: beansScene(),
    announcement: announcementScene(),
  };
  const frames = {} as Record<keyof typeof shots, { frame: string; still: string }>;
  for (const [name, html] of Object.entries(shots) as [keyof typeof shots, string][]) {
    const frame = join(work, `frame-${name}.png`);
    const still = join(work, `still-${name}.png`);
    chrome.screenshot(name, html, { ...SCENE, scale: FILM.scale }, frame);
    convert(frame, still, [
      "-vf",
      `scale=${SCENE.widthPx}:${SCENE.heightPx}:flags=lanczos`,
      "-pred",
      "mixed",
    ]);
    frames[name] = { frame, still };
  }
  return frames;
}

type Shots = ReturnType<typeof drawShots>;

function writePictures(folder: SampleFolder, chrome: Chrome, work: string, shots: Shots): void {
  const at = (name: string) => folder.place(join(FOLDER.pictures, name));
  convert(shots.bags.still, at("Bags on paper.jpg"), ["-q:v", "2", "-huffman", "optimal"]);
  folder.note(
    join(FOLDER.pictures, "Bags on paper.jpg"),
    "Product photograph: two bags and a mug on a paper sweep (JPEG).",
  );

  folder.write(
    join(FOLDER.pictures, "Cup from above.webp"),
    chrome.webp(shots.cup.still, 0.9),
    "A cup of Morning Tide from above, with a tasting card (WebP, encoded by Chrome).",
  );

  run("sips", [
    "-s",
    "format",
    "heic",
    "-s",
    "formatOptions",
    "85",
    shots.box.still,
    "--out",
    at("Subscription box.heic"),
  ]);
  folder.note(
    join(FOLDER.pictures, "Subscription box.heic"),
    "The subscription box, opened, three bags inside (HEIC, as an iPhone saves).",
  );

  run("sips", [
    "-s",
    "format",
    "avif",
    "-s",
    "formatOptions",
    "75",
    shots.beans.still,
    "--out",
    at("Beans up close.avif"),
  ]);
  folder.note(
    join(FOLDER.pictures, "Beans up close.avif"),
    "A bed of roasted beans with a shallow focus (AVIF).",
  );

  copyFileSync(shots.announcement.still, at("Launch announcement.png"));
  folder.note(
    join(FOLDER.pictures, "Launch announcement.png"),
    "The launch announcement for social posts: flat colour (PNG).",
  );

  const brand = join(work, "brand.png");
  chrome.screenshot("brand", brandSheetScene(), { ...SCENE, scale: 1 }, brand);
  convert(brand, at("Brand sheet.tiff"), ["-compression_algo", "deflate", "-pix_fmt", "rgb24"]);
  folder.note(
    join(FOLDER.pictures, "Brand sheet.tiff"),
    "Logo, colours and typefaces on one sheet (TIFF, deflate).",
  );

  const label = join(work, "label.png");
  chrome.screenshot("label", labelScene(), { ...LABEL, scale: 1 }, label);
  run("sips", ["-s", "format", "psd", label, "--out", at("Label artwork.psd")]);
  folder.note(
    join(FOLDER.pictures, "Label artwork.psd"),
    "The bag's front label as print artwork with crop marks (Photoshop).",
  );

  const badge = join(work, "badge.png");
  chrome.screenshot(
    "badge",
    badgeScene(COLOUR.paper),
    { widthPx: 512, heightPx: 512, scale: 1 },
    badge,
  );
  convert(badge, at("Badge.bmp"), [
    "-vf",
    "split[a][b];[a]palettegen=max_colors=48:reserve_transparent=0[p];[b][p]paletteuse=dither=none",
    "-pix_fmt",
    "pal8",
  ]);
  folder.note(
    join(FOLDER.pictures, "Badge.bmp"),
    "The round badge for stickers (BMP, 256-colour).",
  );

  const sheet = join(work, "steam.png");
  const rows = STEAM.frames / STEAM.columns;
  chrome.screenshot(
    "steam",
    steamSheet(),
    { widthPx: STEAM.widthPx * STEAM.columns, heightPx: STEAM.heightPx * rows, scale: 1 },
    sheet,
  );
  ffmpeg([
    "-loop",
    "1",
    "-framerate",
    String(STEAM.framesPerSecond),
    // The palette is made at the input's end, so the input stops after the last frame.
    "-t",
    String(STEAM.frames / STEAM.framesPerSecond),
    "-i",
    sheet,
    "-filter_complex",
    `crop=${STEAM.widthPx}:${STEAM.heightPx}:'mod(n,${STEAM.columns})*${STEAM.widthPx}':'floor(n/${STEAM.columns})*${STEAM.heightPx}',split[a][b];[a]palettegen=max_colors=96:stats_mode=full[p];[b][p]paletteuse=dither=bayer:bayer_scale=4`,
    "-loop",
    "0",
    ...BITEXACT,
    at("Steaming mug.gif"),
  ]);
  folder.note(
    join(FOLDER.pictures, "Steaming mug.gif"),
    "A mug with rising steam that loops (animated GIF, 24 frames).",
  );

  folder.write(join(FOLDER.pictures, "Logo.svg"), logoSvg(), "The logo: mark and wordmark (SVG).");
}

/** The promo: the five shots cross-fading over the music bed, kept losslessly to encode from. */
function makeFilm(work: string, shots: Shots): { film: string; bed: string } {
  const order = [shots.bags, shots.cup, shots.box, shots.beans, shots.announcement];
  const seconds = order.length * FILM.secondsPerShot - (order.length - 1) * FILM.fadeSeconds;
  const bed = join(work, "bed.wav");
  writeFileSync(bed, wavFile(theme(seconds)));
  const inputs = order.flatMap((shot) => [
    "-loop",
    "1",
    "-framerate",
    String(FILM.framesPerSecond),
    "-t",
    String(FILM.secondsPerShot),
    "-i",
    shot.frame,
  ]);
  const prepared = order.map((_, index) => `[${index}:v]format=yuv420p,setsar=1[s${index}]`);
  const fades = order.slice(1).map((_, index) => {
    const from = index === 0 ? "[s0]" : `[x${index}]`;
    const offset = (index + 1) * (FILM.secondsPerShot - FILM.fadeSeconds);
    return `${from}[s${index + 1}]xfade=transition=fade:duration=${FILM.fadeSeconds}:offset=${offset.toFixed(2)}[x${index + 1}]`;
  });
  // It opens on the first shot, not black, so its first frame works as a thumbnail.
  const last = `[x${order.length - 1}]fade=t=out:st=${(seconds - 0.5).toFixed(2)}:d=0.5[film]`;
  const film = join(work, "film.mkv");
  ffmpeg([
    ...inputs,
    "-i",
    bed,
    "-filter_complex",
    [...prepared, ...fades, last].join(";"),
    "-map",
    "[film]",
    "-map",
    `${order.length}:a`,
    "-c:v",
    "ffv1",
    "-c:a",
    "pcm_s16le",
    "-t",
    seconds.toFixed(2),
    ...BITEXACT,
    film,
  ]);
  return { film, bed };
}

function writeVideo(folder: SampleFolder, film: string): void {
  const encode = (name: string, options: readonly string[], about: string) => {
    convert(film, folder.place(join(FOLDER.video, name)), options);
    folder.note(join(FOLDER.video, name), about);
  };
  const h264 = [
    "-c:v",
    "libx264",
    "-preset",
    "slow",
    "-crf",
    "24",
    "-tune",
    "stillimage",
    "-pix_fmt",
    "yuv420p",
  ];
  const aac = ["-c:a", "aac", "-b:a", "160k"];
  encode(
    "Promo.mp4",
    [...h264, ...aac, "-movflags", "+faststart"],
    "The 7.6-second promo, 1080p (H.264 and AAC in MP4).",
  );
  encode(
    "Promo.mov",
    [
      "-c:v",
      "libx265",
      "-crf",
      "24",
      "-preset",
      "medium",
      "-tag:v",
      "hvc1",
      "-x265-params",
      "log-level=error:pools=1:frame-threads=1",
      "-pix_fmt",
      "yuv420p",
      ...aac,
      "-movflags",
      "+faststart",
    ],
    "The promo as Apple cameras save it (HEVC and AAC in QuickTime).",
  );
  encode(
    "Promo 720p.m4v",
    ["-vf", "scale=1280:720:flags=lanczos", ...h264, ...aac, "-movflags", "+faststart"],
    "The promo at 720p for phones (H.264 and AAC in M4V).",
  );
  encode(
    "Promo.webm",
    [
      "-c:v",
      "libvpx-vp9",
      "-crf",
      "34",
      "-b:v",
      "0",
      "-deadline",
      "good",
      "-cpu-used",
      "2",
      "-threads",
      "1",
      "-pix_fmt",
      "yuv420p",
      "-c:a",
      "libopus",
      "-b:a",
      "128k",
    ],
    "The promo for the web (VP9 and Opus in WebM).",
  );
  encode(
    "Promo.mkv",
    [
      "-c:v",
      "libsvtav1",
      "-crf",
      "36",
      "-preset",
      "6",
      "-svtav1-params",
      "lp=1",
      "-pix_fmt",
      "yuv420p",
      "-c:a",
      "libopus",
      "-b:a",
      "128k",
    ],
    "The promo in AV1 and Opus in Matroska; needs a Mac that decodes AV1 (M3 or later).",
  );
}

function writeSound(folder: SampleFolder, work: string, bed: string): void {
  const at = (name: string) => folder.place(join(FOLDER.sound, name));
  const themeWav = join(work, "theme.wav");
  writeFileSync(themeWav, wavFile(theme(16)));
  convert(themeWav, at("Harbor theme.mp3"), ["-c:a", "libmp3lame", "-b:a", "192k"]);
  folder.note(
    join(FOLDER.sound, "Harbor theme.mp3"),
    "The shop's 16-second theme: pads, a plucked arpeggio and bass (MP3).",
  );
  convert(themeWav, at("Harbor theme master.flac"), ["-c:a", "flac", "-compression_level", "8"]);
  folder.note(join(FOLDER.sound, "Harbor theme master.flac"), "The same theme, lossless (FLAC).");
  convert(bed, at("Promo music bed.aac"), ["-c:a", "aac", "-b:a", "160k"]);
  folder.note(
    join(FOLDER.sound, "Promo music bed.aac"),
    "The music under the promo film (raw AAC, ADTS).",
  );
  convert(themeWav, at("Phone hold music.opus"), ["-ac", "1", "-c:a", "libopus", "-b:a", "40k"]);
  folder.note(
    join(FOLDER.sound, "Phone hold music.opus"),
    "The theme in mono for the phone line (Opus).",
  );
  folder.write(
    join(FOLDER.sound, "Shop door bell.wav"),
    wavFile(doorBell()),
    "The shop door's two-note bell (WAV, 16-bit).",
  );

  const crack = join(work, "crack.wav");
  writeFileSync(crack, wavFile(firstCrack(10)));
  convert(crack, at("First crack.ogg"), ["-c:a", "vorbis", "-strict", "experimental", "-q:a", "5"]);
  folder.note(
    join(FOLDER.sound, "First crack.ogg"),
    "Beans popping at first crack in the roaster's drum (Ogg Vorbis).",
  );

  const spoken = join(work, "memo.aiff");
  const voices = run("say", ["-v", "?"]).toString();
  const voice = voices.split("\n").some((line) => line.startsWith(`${VOICE} `))
    ? ["-v", VOICE]
    : [];
  run("say", [...voice, "-r", "178", "-o", spoken, VOICE_MEMO]);
  const room = join(work, "memo.wav");
  ffmpeg([
    "-i",
    spoken,
    "-f",
    "lavfi",
    "-i",
    "anoisesrc=color=pink:amplitude=0.006:seed=7:sample_rate=48000",
    "-filter_complex",
    "[0:a]aresample=48000,adelay=500,apad=pad_dur=0.8,highpass=f=90,lowpass=f=8000,aecho=0.8:0.5:40:0.12[voice];[voice][1:a]amix=inputs=2:duration=first:normalize=0,afade=t=in:d=0.2[out]",
    "-map",
    "[out]",
    "-ac",
    "1",
    "-c:a",
    "pcm_s16le",
    ...BITEXACT,
    room,
  ]);
  run("afconvert", ["-f", "m4af", "-d", "aac", "-b", "64000", room, at("Voice memo.m4a")]);
  writeFileSync(at("Voice memo.m4a"), pinMp4Dates(readFileSync(at("Voice memo.m4a")), STORY_DATE));
  folder.note(
    join(FOLDER.sound, "Voice memo.m4a"),
    "A voice memo about grind settings and the photo shoot (AAC in M4A).",
  );
}

function writeModels(folder: SampleFolder, work: string): void {
  const mug = mugParts();
  const bag = bagParts();
  const generator = "Northroom sample files";
  folder.write(
    join(FOLDER.models, "Mug and saucer.glb"),
    glbFile(mug, generator),
    "A green glazed mug of coffee on its saucer (binary glTF 2.0).",
  );
  folder.write(
    join(FOLDER.models, "Mug and saucer.obj"),
    objFile(mug, "Mug and saucer"),
    "The same mug and saucer as plain geometry (Wavefront OBJ).",
  );
  const usda = usdaFile(bag, "CoffeeBag");
  folder.write(
    join(FOLDER.models, "Coffee bag.usda"),
    usda,
    "A stand-up coffee bag with its label and seal (USD, text).",
  );
  const packed = join(work, "CoffeeBag.usda");
  writeFileSync(packed, usda);
  const usdz = folder.place(join(FOLDER.models, "Coffee bag.usdz"));
  run("/usr/bin/usdzip", [usdz, "CoffeeBag.usda"], { cwd: work });
  writeFileSync(usdz, pinZipDates(readFileSync(usdz), STORY_DATE));
  folder.note(
    join(FOLDER.models, "Coffee bag.usdz"),
    "The coffee bag packed for AR Quick Look (USDZ).",
  );
  folder.write(
    join(FOLDER.models, "Coffee bag.ply"),
    plyFile(bag, "Harbor Coffee bag, centimetres"),
    "The coffee bag with a colour on every vertex (binary PLY).",
  );
  folder.write(
    join(FOLDER.models, "Coffee bean.stl"),
    stlFile(beanMesh(), "Harbor Coffee roasted bean, millimetres"),
    "One roasted bean with its crease, for 3D printing (binary STL).",
  );
}

/** The orders database, built by sqlite3 from the schema in the code folder and its rows. */
function writeDatabase(folder: SampleFolder, work: string): void {
  const sql = join(work, "orders.sql");
  writeFileSync(sql, `${SCHEMA_SQL}\n${ordersSql()}`);
  const database = join(work, "orders.sqlite");
  run("sqlite3", [database, `.read ${sql}`]);
  copyFileSync(database, folder.place(join(FOLDER.spreadsheets, "Orders.sqlite")));
  folder.note(
    join(FOLDER.spreadsheets, "Orders.sqlite"),
    "Customers, coffees and 48 orders in three tables (SQLite).",
  );
}

function writeSpreadsheets(folder: SampleFolder, work: string): void {
  folder.write(
    join(FOLDER.spreadsheets, "Price list.csv"),
    priceListCsv(),
    "Six coffees with origin, roast and price (CSV).",
  );
  folder.write(
    join(FOLDER.spreadsheets, "Customers.tsv"),
    customersTsv(),
    "The first 20 subscribers, their plan and grind (tab-separated).",
  );
  const workbook = workbookFile([
    {
      name: "Plans",
      columnWidths: [14, 14, 16, 16, 12],
      header: ["Plan", "Bags a month", "Price a month", "Cost a month", "Margin %"],
      rows: PLANS.map((plan) => [
        plan.name,
        plan.bags,
        { dollars: plan.dollars },
        { dollars: planCostDollars(plan) },
        planMarginPercent(plan),
      ]),
    },
    {
      name: "Forecast",
      columnWidths: [12, 14, 14, 14, 14],
      header: ["Month", "Subscribers", "Revenue", "Costs", "Profit"],
      rows: FORECAST.map((month) => [
        month.month,
        month.subscribers,
        { dollars: month.revenueDollars },
        { dollars: month.costDollars },
        { dollars: month.profitDollars },
      ]),
    },
  ]);
  folder.write(
    join(FOLDER.spreadsheets, "Subscription model.xlsx"),
    workbook,
    `The plans and the first year's forecast, at $${COST.perBagDollars} a bag and $${COST.shippingDollars} shipping (Excel, two sheets).`,
  );
  writeDatabase(folder, work);
}

const jpegDataUrl = (file: string) =>
  `data:image/jpeg;base64,${readFileSync(file).toString("base64")}`;

function writeDocuments(folder: SampleFolder, chrome: Chrome, work: string, shots: Shots): void {
  const docs = FOLDER.documents;
  folder.write(
    join(docs, "Launch plan.md"),
    planPage(),
    "The launch plan page: front matter, headings, lists, to-dos, a quote, code, a table and reference links (Markdown).",
  );
  folder.write(
    join(docs, "Cupping notes.txt"),
    notesText(),
    "Tasting notes from Saturday's cupping (plain text).",
  );
  folder.write(
    join(docs, "Subscribe.html"),
    subscribePage(),
    "The subscribe page, live: pick a plan and grind and see the price (HTML, no network).",
  );

  const pictures = { bags: join(work, "pdf-bags.jpg"), box: join(work, "pdf-box.jpg") };
  convert(shots.bags.still, pictures.bags, ["-q:v", "4"]);
  convert(shots.box.still, pictures.box, ["-q:v", "4"]);
  const pdf = folder.place(join(docs, "Launch plan.pdf"));
  chrome.printPdf(
    "proposal",
    proposalHtml({ bags: jpegDataUrl(pictures.bags), box: jpegDataUrl(pictures.box) }),
    pdf,
  );
  writeFileSync(pdf, pinPdfDates(readFileSync(pdf), STORY_DATE));
  folder.note(
    join(docs, "Launch plan.pdf"),
    "The launch plan as three designed pages: cover, plans, first year (PDF).",
  );

  const square = join(work, "deck-bags.jpg");
  convert(shots.bags.still, square, ["-vf", "crop=900:900:350:0", "-q:v", "4"]);
  const darkBadge = join(work, "badge-dark.png");
  chrome.screenshot(
    "badge-dark",
    badgeScene(COLOUR.ink),
    { widthPx: 512, heightPx: 512, scale: 1 },
    darkBadge,
  );
  const deck = pitchDeck([
    { format: "jpeg", bytes: readFileSync(square) },
    { format: "jpeg", bytes: readFileSync(pictures.box) },
    { format: "png", bytes: readFileSync(darkBadge) },
  ]);
  if (deck.pictures.length !== Object.keys(DECK_PICTURE).length)
    throw new Error("the deck is missing a picture");
  folder.write(
    join(docs, "Pitch deck.pptx"),
    deckFile(deck),
    "The six-slide pitch for the bank and partners (PowerPoint).",
  );

  // textutil reads a designed HTML letter and writes each word-processor format from it.
  const letterDir = join(work, "letter");
  mkdirSync(letterDir, { recursive: true });
  convert(join(work, "badge.png"), join(letterDir, "logo.png"), [
    "-vf",
    "scale=128:128:flags=lanczos",
  ]);
  const letter = join(letterDir, "letter.html");
  writeFileSync(letter, letterHtml("logo.png"));
  const formats = [
    ["docx", "Wholesale letter.docx", "The wholesale offer to a café, with its prices (Word)."],
    ["doc", "Wholesale letter.doc", "The wholesale letter as a Word 97 document."],
    ["rtf", "Wholesale letter.rtf", "The wholesale letter as rich text (RTF)."],
    [
      "rtfd",
      "Wholesale letter.rtfd",
      "The wholesale letter with its logo, as a TextEdit package (RTFD).",
    ],
    ["odt", "Wholesale letter.odt", "The wholesale letter as OpenDocument text."],
  ] as const;
  for (const [format, name, about] of formats) {
    const target = folder.place(join(docs, name));
    run("textutil", ["-convert", format, "-output", target, letter], { cwd: letterDir });
    if (format === "docx" || format === "odt")
      writeFileSync(target, pinZipDates(readFileSync(target), STORY_DATE));
    folder.note(join(docs, name), about);
  }
}

/** What each code file is, for the README. */
const CODE_ABOUT: Record<string, string> = {
  "roast_profile.py": "Rate of rise and development time from a roast log (Python).",
  "inventory.rs": "How many roasts the green coffee on the shelf still makes (Rust).",
  "subscription.ts": "The plans and each one's margin (TypeScript).",
  "shipping.js": "When a box roasted on Monday arrives (JavaScript).",
  "coffees.json": "The coffees in rotation (JSON).",
  "roast-schedule.yaml": "Next week's roasts by day (YAML).",
  "roaster.toml": "The roaster's and probe logger's settings (TOML).",
  "backup-orders.sh": "Backs up the orders database each day (shell script).",
  "brand.css": "The brand's colours and type as CSS variables (CSS).",
  "schema.sql": "The orders database's tables; Orders.sqlite is built from it (SQL).",
};

function writeCode(folder: SampleFolder): void {
  for (const [name, text] of Object.entries(codeFiles())) {
    const about = CODE_ABOUT[name];
    if (about === undefined) throw new Error(`${name} has no description`);
    folder.write(join(FOLDER.code, name), text, about);
  }
  chmodSync(join(folder.root, FOLDER.code, "backup-orders.sh"), 0o755);
  folder.write(
    join(FOLDER.code, "Terminal.nterm"),
    TERMINAL_FILE_TEXT,
    "A terminal: opens a shell in this folder (Northroom terminal).",
  );
}

function writeFonts(folder: SampleFolder): void {
  for (const font of FONTS) {
    const name = path.basename(font.from);
    copyFileSync(font.from, folder.place(join(FOLDER.fonts, name)));
    folder.note(join(FOLDER.fonts, name), font.about);
  }
}

function writeOther(folder: SampleFolder, work: string): void {
  const photo = join(work, "contact.png");
  convert(join(work, "badge.png"), photo, ["-vf", "scale=256:256:flags=lanczos"]);
  folder.write(
    join(FOLDER.other, `${BUSINESS.owner}.vcf`),
    contactCard(readFileSync(photo)),
    "The founder's contact card, with the badge as its photo (vCard).",
  );
  folder.write(
    join(FOLDER.other, "Launch calendar.ics"),
    launchCalendar(),
    "Launch day, with every launch step in its notes (iCalendar).",
  );
  const kit = zipFile([
    ["Press kit/Logo.svg", logoSvg()],
    [
      "Press kit/Launch announcement.png",
      readFileSync(join(folder.root, FOLDER.pictures, "Launch announcement.png")),
    ],
    [
      "Press kit/About.txt",
      `${BUSINESS.fullName} roasts small batches in ${BUSINESS.city}, Maine. ${BUSINESS.subscription} starts April 1, 2026.\n`,
    ],
  ]);
  folder.write(
    join(FOLDER.other, "Press kit.zip"),
    pinZipDates(kit, STORY_DATE),
    "The logo, the announcement and a short blurb, zipped: nothing previews a zip, so the app shows its fallback.",
  );
}

function readme(folder: SampleFolder): string {
  const sections = Object.values(FOLDER).map((name) => {
    const files = folder.written
      .filter((file) => path.dirname(file.path) === name)
      .sort((a, b) => a.path.localeCompare(b.path))
      .map((file) => `- **${path.basename(file.path)}**: ${file.about}`);
    return [`## ${name}`, "", FOLDER_ABOUT[name] ?? "", "", ...files, ""].join("\n");
  });
  return [
    "# Harbor Coffee",
    "",
    `Sample files for Northroom: one of every type the app shows, all from one small roastery in ${BUSINESS.city}, Maine, getting ready to launch ${BUSINESS.subscription}, a coffee subscription. Every picture, sound, film and model was made by \`apps/native/scripts/sample-files.ts\`.`,
    "",
    ...sections,
  ].join("\n");
}

/** Dates every file and folder under `root`, deepest first, to the story's day. */
function dateEverything(root: string): void {
  for (const entry of readdirSync(root)) {
    const target = join(root, entry);
    if (statSync(target).isDirectory()) dateEverything(target);
    utimesSync(target, STORY_DATE, STORY_DATE);
  }
  utimesSync(root, STORY_DATE, STORY_DATE);
}

function main(): void {
  const root = readTarget();
  const work = mkdtempSync(join(tmpdir(), "northroom-samples-"));
  try {
    const folder = new SampleFolder(root);
    const chrome = new Chrome(work);
    const shots = drawShots(chrome, work);
    writePictures(folder, chrome, work, shots);
    const { film, bed } = makeFilm(work, shots);
    writeVideo(folder, film);
    writeSound(folder, work, bed);
    writeModels(folder, work);
    writeSpreadsheets(folder, work);
    writeDocuments(folder, chrome, work, shots);
    writeCode(folder);
    writeFonts(folder);
    writeOther(folder, work);
    folder.write("README.md", readme(folder), "This list.");
    dateEverything(root);
    console.log(root);
  } finally {
    rmSync(work, { recursive: true, force: true });
  }
}

main();
