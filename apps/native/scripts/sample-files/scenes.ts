/**
 * The pictures, each a designed page Chrome draws: product photographs of the bags, a cup from
 * above, the subscription box and a field of beans, plus the flat brand pieces (the announcement,
 * the brand sheet, the label artwork and the badge) and the frames of the steaming mug.
 */
import {
  BAG_LOOKS,
  bag,
  bean,
  beanDefs,
  grain,
  logoMark,
  mug,
  scatterBeans,
  sharedDefs,
  wordmark,
} from "./art";
import {
  BREAKWATER,
  BUSINESS,
  COLOUR,
  LIGHTHOUSE,
  MORNING_TIDE,
  PLANS,
  TYPE,
  seededRandom,
} from "./story";

/** The size every photograph is designed at, in CSS pixels; Chrome draws it larger for the film. */
export const SCENE = { widthPx: 1600, heightPx: 900 } as const;

/** A page holding one SVG picture that fills it exactly. */
export const page = (svg: string, ground: string = COLOUR.paper) =>
  `<!doctype html><html><head><meta charset="utf-8"><style>html,body{margin:0;padding:0;overflow:hidden;background:${ground}}svg{display:block}</style></head><body>${svg}</body></html>`;

const svg = (width: number, height: number, seed: number, inner: string) =>
  `<svg xmlns="http://www.w3.org/2000/svg" width="${width}" height="${height}" viewBox="0 0 ${width} ${height}"><defs>${sharedDefs(seed)}${beanDefs()}</defs>${inner}</svg>`;

const MUG_GLAZE = { light: "#fbf9f4", mid: "#ece6da", dark: "#c9c1b1" } as const;

/** A soft contact shadow under something standing at `baseY`, and the longer one it casts back. */
const standingShadow = (centreX: number, baseY: number, halfWidth: number) => `
<path d="M${centreX - halfWidth} ${baseY} L${centreX + halfWidth} ${baseY} L${centreX + halfWidth * 1.9} ${baseY - 70} L${centreX + halfWidth * 0.3} ${baseY - 70} Z" fill="#3b301f" opacity="0.13" filter="url(#softer)"/>
<ellipse cx="${centreX}" cy="${baseY}" rx="${halfWidth * 1.02}" ry="11" fill="#1d160c" opacity="0.38" filter="url(#contact)"/>`;

const paperSweep = (id: string, top: string, middle: string, bottom: string) => `
<linearGradient id="${id}" x1="0" y1="0" x2="0" y2="1">
  <stop offset="0" stop-color="${top}"/>
  <stop offset="0.6" stop-color="${middle}"/>
  <stop offset="1" stop-color="${bottom}"/>
</linearGradient>`;

const vignette = (width: number, height: number, strength: number) => `
<radialGradient id="vignette" cx="0.5" cy="0.5" r="0.75"><stop offset="0.55" stop-color="#000" stop-opacity="0"/><stop offset="1" stop-color="#000" stop-opacity="${strength}"/></radialGradient>
<rect width="${width}" height="${height}" fill="url(#vignette)"/>`;

/** Two bags and a mug on a paper sweep in window light: the shop's product photograph. */
export function bagsScene(): string {
  const { widthPx: w, heightPx: h } = SCENE;
  const beans = scatterBeans(
    seededRandom(11),
    7,
    { x: 380, y: 760, width: 760, height: 70 },
    { least: 1.05, most: 1.25 },
    (x) => x > 520 && x < 1000,
  );
  return page(
    svg(
      w,
      h,
      3,
      `<defs>${paperSweep("sweep", "#e5dfd2", "#f1ece2", "#ddd5c5")}
  <radialGradient id="glow" cx="0.28" cy="0.2" r="0.85"><stop offset="0" stop-color="#fff" stop-opacity="0.6"/><stop offset="1" stop-color="#fff" stop-opacity="0"/></radialGradient></defs>
<rect width="${w}" height="${h}" fill="url(#sweep)"/>
<rect width="${w}" height="${h}" fill="url(#glow)"/>
<g opacity="0.07" filter="url(#softer)" fill="#2a2216">
  <path d="M1180 -40 L1240 -40 L1010 640 L950 640 Z"/>
  <path d="M1420 -40 L1480 -40 L1250 640 L1190 640 Z"/>
  <path d="M1000 250 L1600 210 L1600 250 L990 290 Z"/>
</g>
<g transform="translate(800 520) scale(1.22) translate(-965 -560)">
${standingShadow(968, 712, 128)}
${standingShadow(690, 730, 146)}
${standingShadow(1240, 772, 98)}
<g transform="translate(830 311) scale(0.92)">${bag("black", BAG_LOOKS.ink, BREAKWATER)}</g>
<g transform="translate(540 294)">${bag("green", BAG_LOOKS.green, MORNING_TIDE)}</g>
<g transform="translate(1118 558)">${mug("mug", MUG_GLAZE)}</g>
${beans}
</g>
${vignette(w, h, 0.2)}
${grain(w, h)}`,
    ),
  );
}

/** A cup of black coffee on its saucer from above, beans, a tasting card and a bag's corner. */
export function cupScene(): string {
  const { widthPx: w, heightPx: h } = SCENE;
  const saucer = { x: 760, y: 470 };
  const beans = scatterBeans(
    seededRandom(23),
    16,
    { x: 120, y: 80, width: 1400, height: 760 },
    { least: 1.4, most: 1.7 },
    (x, y) =>
      Math.hypot(x - saucer.x, y - saucer.y) < 300 ||
      (x > 1060 && x < 1460 && y > 110 && y < 420) ||
      (x < 360 && y < 380),
  );
  return page(
    svg(
      w,
      h,
      5,
      `<defs>
  <linearGradient id="linen" x1="0" y1="0" x2="1" y2="1"><stop offset="0" stop-color="#efe9dd"/><stop offset="1" stop-color="#e2dbcc"/></linearGradient>
  <filter id="weave" x="0" y="0" width="100%" height="100%">
    <feTurbulence type="fractalNoise" baseFrequency="0.9 0.04" numOctaves="2" seed="9"/>
    <feColorMatrix type="saturate" values="0"/>
    <feComponentTransfer><feFuncA type="table" tableValues="0 0.06"/></feComponentTransfer>
  </filter>
  <radialGradient id="saucer" cx="0.46" cy="0.44" r="0.56"><stop offset="0" stop-color="#fbf9f4"/><stop offset="0.72" stop-color="#f3efe6"/><stop offset="0.92" stop-color="#e3ddcf"/><stop offset="1" stop-color="#d3cbbb"/></radialGradient>
  <radialGradient id="rim" cx="0.42" cy="0.4" r="0.6"><stop offset="0" stop-color="#fdfcf8"/><stop offset="1" stop-color="#e2dbcd"/></radialGradient>
  <radialGradient id="wall" cx="0.55" cy="0.56" r="0.55"><stop offset="0.8" stop-color="#efeae0"/><stop offset="1" stop-color="#cfc7b7"/></radialGradient>
  <radialGradient id="coffee" cx="0.44" cy="0.42" r="0.56"><stop offset="0" stop-color="#3f2415"/><stop offset="0.62" stop-color="#26140a"/><stop offset="0.9" stop-color="#3e2414"/><stop offset="1" stop-color="#8a5c37"/></radialGradient>
</defs>
<rect width="${w}" height="${h}" fill="url(#linen)"/>
<rect width="${w}" height="${h}" filter="url(#weave)"/>
<g transform="translate(-60 -120) rotate(-14)">
  <rect x="40" y="40" width="420" height="520" rx="10" fill="#000" opacity="0.18" filter="url(#soft)" transform="translate(18 26)"/>
  <rect x="40" y="40" width="420" height="520" rx="10" fill="${COLOUR.green}"/>
  <rect x="110" y="250" width="280" height="300" rx="4" fill="${COLOUR.paper}"/>
  <g transform="translate(222 280) scale(0.56)">${logoMark(COLOUR.green, 5)}</g>
  <g transform="translate(250 372)">${wordmark(COLOUR.ink, 30)}</g>
  <rect x="226" y="404" width="48" height="2" fill="${COLOUR.ink}" opacity="0.4"/>
  <text x="250" y="446" text-anchor="middle" font-family="${TYPE.display}" font-size="30" font-weight="600" fill="${COLOUR.ink}">${MORNING_TIDE.name}</text>
  <text x="250" y="474" text-anchor="middle" font-family="${TYPE.text}" font-size="12" letter-spacing="1.6" fill="${COLOUR.ink}" opacity="0.7">${MORNING_TIDE.origin.toUpperCase()}</text>
</g>
<g transform="translate(1250 270) rotate(7)">
  <rect x="-170" y="-110" width="340" height="220" rx="6" fill="#000" opacity="0.2" filter="url(#soft)" transform="translate(10 16)"/>
  <rect x="-170" y="-110" width="340" height="220" rx="6" fill="#fbf9f4"/>
  <text x="-136" y="-58" font-family="${TYPE.text}" font-size="12" font-weight="600" letter-spacing="2.4" fill="${COLOUR.green}">TASTING CARD · NO. 01</text>
  <text x="-136" y="-12" font-family="${TYPE.display}" font-size="36" font-weight="600" fill="${COLOUR.ink}">${MORNING_TIDE.name}</text>
  <text x="-136" y="18" font-family="${TYPE.text}" font-size="15" fill="${COLOUR.ink}" opacity="0.75">${MORNING_TIDE.origin} · ${MORNING_TIDE.process}</text>
  <line x1="-136" x2="136" y1="44" y2="44" stroke="${COLOUR.ink}" stroke-opacity="0.15"/>
  <text x="-136" y="74" font-family="${TYPE.text}" font-size="15" font-style="italic" fill="${COLOUR.ink}" opacity="0.8">${MORNING_TIDE.notes}</text>
</g>
<g transform="translate(${saucer.x} ${saucer.y})">
  <circle r="255" fill="#2b2012" opacity="0.22" filter="url(#soft)" transform="translate(22 30)"/>
  <circle r="255" fill="url(#saucer)"/>
  <circle r="186" fill="none" stroke="#dcd5c7" stroke-width="7" filter="url(#blur2)"/>
  <g transform="rotate(38)">
    <rect x="150" y="-26" width="96" height="52" rx="26" fill="#2b2012" opacity="0.25" filter="url(#contact)" transform="translate(10 12)"/>
    <rect x="150" y="-26" width="96" height="52" rx="26" fill="url(#rim)"/>
    <rect x="168" y="-11" width="56" height="22" rx="11" fill="#d9d2c4"/>
  </g>
  <circle r="176" fill="#2b2012" opacity="0.28" filter="url(#soft)" transform="translate(14 20)"/>
  <circle r="170" fill="url(#rim)"/>
  <circle r="151" fill="url(#wall)"/>
  <circle r="139" fill="url(#coffee)"/>
  <ellipse cx="-44" cy="-58" rx="56" ry="20" fill="#fff" opacity="0.16" filter="url(#blur2)" transform="rotate(-32 -44 -58)"/>
  <ellipse cx="52" cy="70" rx="30" ry="9" fill="#fff" opacity="0.05" filter="url(#blur2)" transform="rotate(-32 52 70)"/>
</g>
${beans}
${vignette(w, h, 0.16)}
${grain(w, h)}`,
    ),
  );
}

/** The subscription box opened in front of a green arch, three bags standing in it. */
export function boxScene(): string {
  const { widthPx: w, heightPx: h } = SCENE;
  const tissue = Array.from({ length: 15 }, (_, index) => {
    const x = 548 + index * 36;
    return `${x},${index % 2 === 0 ? 628 : 606}`;
  }).join(" ");
  return page(
    svg(
      w,
      h,
      7,
      `<defs>${paperSweep("wallpaper", "#efeadf", "#f3efe6", "#e1dacb")}
  <linearGradient id="arch" x1="0" y1="0" x2="1" y2="1"><stop offset="0" stop-color="${COLOUR.greenLight}"/><stop offset="1" stop-color="${COLOUR.greenDark}"/></linearGradient>
  <linearGradient id="front" x1="0" y1="0" x2="1" y2="0"><stop offset="0" stop-color="#0f0f0e"/><stop offset="0.3" stop-color="#262624"/><stop offset="1" stop-color="#121211"/></linearGradient>
  <linearGradient id="lid" x1="0" y1="0" x2="0" y2="1"><stop offset="0" stop-color="#f7f4ed"/><stop offset="1" stop-color="#ddd6c7"/></linearGradient>
</defs>
<rect width="${w}" height="${h}" fill="url(#wallpaper)"/>
<path d="M380 760 L380 440 A420 420 0 0 1 1220 440 L1220 760 Z" fill="url(#arch)"/>
<rect x="0" y="730" width="${w}" height="${h - 730}" fill="#e6dfd1"/>
<rect x="0" y="726" width="${w}" height="14" fill="#000" opacity="0.06" filter="url(#contact)"/>
<ellipse cx="820" cy="842" rx="360" ry="22" fill="#1b140b" opacity="0.4" filter="url(#soft)"/>
<path d="M552 572 L1048 572 L1076 210 L524 210 Z" fill="url(#lid)"/>
<path d="M552 572 L1048 572 L1076 210 L524 210 Z" fill="none" stroke="#000" stroke-opacity="0.08" stroke-width="2"/>
<text x="800" y="282" text-anchor="middle" font-family="${TYPE.text}" font-size="14" font-weight="600" letter-spacing="3.6" fill="${COLOUR.green}">HELLO FROM HARBOR</text>
<text x="800" y="330" text-anchor="middle" font-family="${TYPE.display}" font-size="36" font-weight="600" fill="${COLOUR.ink}">Roasted Monday.</text>
<text x="800" y="374" text-anchor="middle" font-family="${TYPE.display}" font-size="36" font-weight="600" fill="${COLOUR.ink}">Yours by Thursday.</text>
<path d="M552 572 L1048 572 L1080 640 L520 640 Z" fill="#0b0b0a"/>
<polygon points="530,640 ${tissue} 1070,640" fill="${COLOUR.greenLight}"/>
<g transform="translate(560 476) rotate(-8 108 160) scale(0.72)">${bag("left", BAG_LOOKS.paper, LIGHTHOUSE)}</g>
<g transform="translate(862 476) rotate(7 108 160) scale(0.72)">${bag("right", BAG_LOOKS.ink, BREAKWATER)}</g>
<g transform="translate(694 452) scale(0.72)">${bag("middle", BAG_LOOKS.green, MORNING_TIDE)}</g>
<path d="M520 640 L1080 640 L1080 836 L520 836 Z" fill="url(#front)"/>
<path d="M520 640 L1080 640" stroke="#3a3a37" stroke-width="2"/>
<g transform="translate(782 690) scale(0.36)">${logoMark(COLOUR.paper, 5)}</g>
<text x="800" y="778" text-anchor="middle" font-family="${TYPE.text}" font-size="15" font-weight="600" letter-spacing="5" fill="${COLOUR.paper}">${BUSINESS.subscription.toUpperCase()}</text>
${vignette(w, h, 0.18)}
${grain(w, h)}`,
    ),
  );
}

/** The row of the bean bed the lens is focused on, in pixels from the top. */
const BEAN_FOCUS_Y = 470;

/**
 * A bed of roasted beans seen from a low angle: small and soft at the back, sharp across the
 * middle and large and soft in front, as a macro lens sees them.
 */
export function beansScene(): string {
  const { widthPx: w, heightPx: h } = SCENE;
  const random = seededRandom(31);
  const bands: { blur: number; beans: string[] }[] = [];
  for (let y = -40; y < h + 120; ) {
    const scale = 1.5 + (Math.max(y, 0) / h) * 3.2;
    const blur = Math.min(Math.round(Math.abs(y - BEAN_FOCUS_Y) / 70), 9);
    const last = bands[bands.length - 1];
    const band = last !== undefined && last.blur === blur ? last : { blur, beans: [] };
    if (band !== last) bands.push(band);
    for (let x = -60 + random() * 30; x < w + 60; x += 27 * scale) {
      const jitterX = (random() - 0.5) * 14 * scale;
      const jitterY = (random() - 0.5) * 8 * scale;
      const size = scale * (0.92 + random() * 0.16);
      band.beans.push(
        bean(x + jitterX, y + jitterY, random() * 360, size, Math.floor(random() * 3)),
      );
    }
    y += 9 * scale;
  }
  const layers = bands.map(({ blur, beans }) =>
    blur === 0
      ? `<g>${beans.join("")}</g>`
      : `<g filter="url(#depth${blur})">${beans.join("")}</g>`,
  );
  const filters = Array.from(
    { length: 9 },
    (_, index) =>
      `<filter id="depth${index + 1}" x="-5%" y="-5%" width="110%" height="110%"><feGaussianBlur stdDeviation="${((index + 1) * 0.9).toFixed(1)}"/></filter>`,
  ).join("");
  return page(
    svg(
      w,
      h,
      13,
      `<defs>${filters}
  <linearGradient id="light" x1="0" y1="0" x2="1" y2="1"><stop offset="0" stop-color="#ffd9a8" stop-opacity="0.14"/><stop offset="0.55" stop-color="#ffd9a8" stop-opacity="0"/></linearGradient>
  <linearGradient id="haze" x1="0" y1="0" x2="0" y2="1"><stop offset="0" stop-color="#0d0805" stop-opacity="0.55"/><stop offset="0.35" stop-color="#0d0805" stop-opacity="0"/></linearGradient>
</defs>
<rect width="${w}" height="${h}" fill="#140c07"/>
${layers.join("")}
<rect width="${w}" height="${h}" fill="url(#haze)"/>
<rect width="${w}" height="${h}" fill="url(#light)"/>
${vignette(w, h, 0.45)}
${grain(w, h)}`,
    ),
    "#140c07",
  );
}

/** The launch announcement: flat, on green, the three bags fanned beside the words. */
export function announcementScene(): string {
  const { widthPx: w, heightPx: h } = SCENE;
  const from = Math.min(...PLANS.map((plan) => plan.dollars));
  return page(
    svg(
      w,
      h,
      17,
      `<rect width="${w}" height="${h}" fill="${COLOUR.green}"/>
<circle cx="1240" cy="460" r="310" fill="${COLOUR.greenDark}"/>
<g transform="translate(120 124)">
  <g transform="scale(0.6)">${logoMark(COLOUR.paper, 4)}</g>
  <text x="82" y="38" font-family="${TYPE.text}" font-size="18" font-weight="600" letter-spacing="5" fill="${COLOUR.paper}">${BUSINESS.subscription.toUpperCase()}</text>
</g>
<text font-family="${TYPE.display}" font-size="96" font-weight="600" fill="${COLOUR.paper}" letter-spacing="-1">
  <tspan x="116" y="352">Fresh coffee,</tspan>
  <tspan x="116" y="470">on your schedule.</tspan>
</text>
<text font-family="${TYPE.text}" font-size="30" fill="${COLOUR.paper}" opacity="0.86">
  <tspan x="120" y="560">Roasted on Monday, at your door by Thursday.</tspan>
  <tspan x="120" y="604">From $${from} a month. Pause or stop any time.</tspan>
</text>
<rect x="120" y="668" width="260" height="68" rx="34" fill="${COLOUR.paper}"/>
<text x="250" y="712" text-anchor="middle" font-family="${TYPE.text}" font-size="24" font-weight="600" fill="${COLOUR.green}">Starts April 1</text>
<g opacity="0.25" fill="#000" filter="url(#soft)">
  <rect x="1000" y="300" width="200" height="300" rx="10" transform="rotate(-12 1100 450) translate(12 24)"/>
  <rect x="1150" y="270" width="200" height="300" rx="10" transform="translate(12 24)"/>
  <rect x="1300" y="300" width="200" height="300" rx="10" transform="rotate(12 1400 450) translate(12 24)"/>
</g>
<g transform="rotate(-12 1100 450) translate(976 300) scale(0.68)">${bag("a-left", BAG_LOOKS.paper, LIGHTHOUSE)}</g>
<g transform="rotate(12 1400 450) translate(1290 300) scale(0.68)">${bag("a-right", BAG_LOOKS.ink, BREAKWATER)}</g>
<g transform="translate(1110 250) scale(0.76)">${bag("a-middle", { ...BAG_LOOKS.paper, body: "#f6f3ec", trim: "#e3dccd", label: COLOUR.green }, MORNING_TIDE)}</g>`,
    ),
    COLOUR.green,
  );
}

/** The brand on one sheet: the logo, the colours with their values, and the two typefaces. */
export function brandSheetScene(): string {
  const { widthPx: w, heightPx: h } = SCENE;
  const swatches = [
    ["Harbor Green", COLOUR.green],
    ["Ink", COLOUR.ink],
    ["Paper", COLOUR.paper],
    ["Sand", COLOUR.sand],
    ["Roast", COLOUR.roast],
  ]
    .map(([name, hex], index) => {
      const x = 860 + index * 136;
      return `<rect x="${x}" y="120" width="116" height="168" rx="10" fill="${hex}" stroke="#000" stroke-opacity="0.1"/>
<text x="${x}" y="320" font-family="${TYPE.text}" font-size="16" font-weight="600" fill="${COLOUR.ink}">${name}</text>
<text x="${x}" y="344" font-family="${TYPE.text}" font-size="14" fill="${COLOUR.ink}" opacity="0.6">${hex?.toUpperCase()}</text>`;
    })
    .join("");
  return page(
    svg(
      w,
      h,
      19,
      `<rect width="${w}" height="${h}" fill="${COLOUR.paper}"/>
<rect x="60" y="60" width="680" height="780" rx="18" fill="#fbf9f4" stroke="#000" stroke-opacity="0.06"/>
<g transform="translate(300 170) scale(2)">${logoMark(COLOUR.green, 4)}</g>
<g transform="translate(400 506)">${wordmark(COLOUR.ink, 74)}</g>
<rect x="356" y="586" width="88" height="4" rx="2" fill="${COLOUR.green}"/>
<text x="400" y="770" text-anchor="middle" font-family="${TYPE.text}" font-size="16" letter-spacing="2" fill="${COLOUR.ink}" opacity="0.55">PRIMARY LOCKUP · KEEP ONE MARK-HEIGHT CLEAR</text>
<text x="860" y="96" font-family="${TYPE.text}" font-size="15" font-weight="600" letter-spacing="3" fill="${COLOUR.green}">COLOUR</text>
${swatches}
<text x="860" y="430" font-family="${TYPE.text}" font-size="15" font-weight="600" letter-spacing="3" fill="${COLOUR.green}">TYPE</text>
<text x="860" y="520" font-family="${TYPE.display}" font-size="76" font-weight="600" fill="${COLOUR.ink}">Optima</text>
<text x="1140" y="520" font-family="${TYPE.text}" font-size="17" fill="${COLOUR.ink}" opacity="0.6">Display · names and headlines</text>
<text x="860" y="572" font-family="${TYPE.display}" font-size="30" fill="${COLOUR.ink}">Aa Bb Cc Dd Ee Ff Gg 0123456789</text>
<line x1="860" x2="1540" y1="612" y2="612" stroke="#000" stroke-opacity="0.1"/>
<text x="860" y="690" font-family="${TYPE.text}" font-size="64" font-weight="600" fill="${COLOUR.ink}">Avenir Next</text>
<text x="1270" y="690" font-family="${TYPE.text}" font-size="17" fill="${COLOUR.ink}" opacity="0.6">Text · labels and copy</text>
<text x="860" y="742" font-family="${TYPE.text}" font-size="22" fill="${COLOUR.ink}" opacity="0.8">Roasted on Monday, at your door by Thursday.</text>
<text x="860" y="776" font-family="${TYPE.text}" font-size="22" fill="${COLOUR.ink}" opacity="0.8">Small batches from the harbor in Portland, Maine.</text>`,
    ),
  );
}

/** The size the label artwork is drawn at. */
export const LABEL = { widthPx: 800, heightPx: 1000 } as const;

/** The bag's front label as print artwork: the label at full size with crop marks around it. */
export function labelScene(): string {
  const { widthPx: w, heightPx: h } = LABEL;
  const crop = (x: number, y: number, dx: number, dy: number) =>
    `<path d="M${x + dx * 16} ${y} h${dx * 40} M${x} ${y + dy * 16} v${dy * 40}" stroke="${COLOUR.ink}" stroke-width="1.5"/>`;
  const left = 140;
  const top = 130;
  const right = w - 140;
  const bottom = h - 130;
  const roastDots = [0, 1, 2, 3, 4]
    .map(
      (index) =>
        `<circle cx="${328 + index * 36}" cy="760" r="10" fill="${index < 3 ? COLOUR.green : "none"}" stroke="${COLOUR.green}" stroke-width="2.5"/>`,
    )
    .join("");
  return page(
    svg(
      w,
      h,
      21,
      `<rect width="${w}" height="${h}" fill="#dcd6ca"/>
${crop(left, top, -1, -1)}${crop(right, top, 1, -1)}${crop(left, bottom, -1, 1)}${crop(right, bottom, 1, 1)}
<rect x="${left}" y="${top}" width="${right - left}" height="${bottom - top}" fill="${COLOUR.paper}"/>
<rect x="${left + 20}" y="${top + 20}" width="${right - left - 40}" height="${bottom - top - 40}" fill="none" stroke="${COLOUR.green}" stroke-width="2"/>
<g transform="translate(340 200) scale(1.2)">${logoMark(COLOUR.green, 4)}</g>
<g transform="translate(400 392)">${wordmark(COLOUR.ink, 46)}</g>
<rect x="360" y="440" width="80" height="3" fill="${COLOUR.green}"/>
<text x="400" y="530" text-anchor="middle" font-family="${TYPE.display}" font-size="56" font-weight="600" fill="${COLOUR.ink}">${MORNING_TIDE.name}</text>
<text x="400" y="580" text-anchor="middle" font-family="${TYPE.text}" font-size="19" font-weight="500" letter-spacing="3" fill="${COLOUR.ink}" opacity="0.8">${MORNING_TIDE.origin.toUpperCase()} · ${MORNING_TIDE.process.toUpperCase()}</text>
<text x="400" y="630" text-anchor="middle" font-family="${TYPE.text}" font-size="22" font-style="italic" fill="${COLOUR.ink}" opacity="0.75">${MORNING_TIDE.notes}</text>
<text x="400" y="718" text-anchor="middle" font-family="${TYPE.text}" font-size="14" font-weight="600" letter-spacing="3" fill="${COLOUR.green}">ROAST</text>
${roastDots}
<text x="400" y="830" text-anchor="middle" font-family="${TYPE.text}" font-size="15" font-weight="600" letter-spacing="3" fill="${COLOUR.ink}" opacity="0.7">${MORNING_TIDE.grams} G · WHOLE BEAN · ${MORNING_TIDE.sku}</text>
<text x="${w / 2}" y="${h - 50}" text-anchor="middle" font-family="${TYPE.text}" font-size="13" letter-spacing="1" fill="${COLOUR.ink}" opacity="0.55">Front label · 85 × 110 mm · trim at crop marks</text>`,
    ),
    "#dcd6ca",
  );
}

/** The size of the round badge. */
export const BADGE = { sizePx: 512 } as const;

/** The mark alone on a green disc, as a sticker or an app icon uses it. */
export function badgeScene(ground: string): string {
  const size = BADGE.sizePx;
  return page(
    `<svg xmlns="http://www.w3.org/2000/svg" width="${size}" height="${size}" viewBox="0 0 512 512">
<rect width="512" height="512" fill="${ground}"/>
<circle cx="256" cy="256" r="236" fill="${COLOUR.green}"/>
<circle cx="256" cy="256" r="212" fill="none" stroke="${COLOUR.paper}" stroke-width="3" stroke-dasharray="2 10" stroke-linecap="round"/>
<g transform="translate(136 136) scale(2.4)">${logoMark(COLOUR.paper, 4)}</g>
</svg>`,
    ground,
  );
}

/** One frame of the mug's steam, `index` of `count`, so the frames loop without a seam. */
function steamFrame(index: number, count: number): string {
  const phase = (index / count) * Math.PI * 2;
  const wisp = (offsetX: number, shift: number, sway: number) => {
    const points = Array.from({ length: 30 }, (_, step) => {
      const y = 150 - step * 4.4;
      const rise = step / 29;
      const x = 240 + offsetX + Math.sin(rise * 5.5 - phase * 1 + shift) * sway * (0.3 + rise);
      return `${x.toFixed(1)},${y.toFixed(1)}`;
    });
    return `<polyline points="${points.join(" ")}" fill="none" stroke="url(#steam)" stroke-width="9" stroke-linecap="round" filter="url(#wisp)"/>`;
  };
  return `<svg xmlns="http://www.w3.org/2000/svg" width="480" height="360" viewBox="0 0 480 360">
<defs>
  <filter id="wisp" x="-50%" y="-20%" width="200%" height="140%"><feGaussianBlur stdDeviation="3.2"/></filter>
  <filter id="contact" x="-50%" y="-50%" width="200%" height="200%"><feGaussianBlur stdDeviation="5"/></filter>
  <linearGradient id="steam" x1="0" y1="1" x2="0" y2="0" gradientUnits="objectBoundingBox"><stop offset="0" stop-color="#fff" stop-opacity="0"/><stop offset="0.25" stop-color="#fff" stop-opacity="0.9"/><stop offset="1" stop-color="#fff" stop-opacity="0"/></linearGradient>
  <radialGradient id="room" cx="0.5" cy="0.4" r="0.8"><stop offset="0" stop-color="#d9e4dc"/><stop offset="1" stop-color="#a9bfb0"/></radialGradient>
</defs>
<rect width="480" height="360" fill="url(#room)"/>
<rect y="300" width="480" height="60" fill="#94ab9c"/>
<ellipse cx="240" cy="304" rx="110" ry="9" fill="#1d2a22" opacity="0.4" filter="url(#contact)"/>
<g transform="translate(137 118) scale(0.86)">${mug("gif-mug", MUG_GLAZE)}</g>
<g transform="translate(-6 -2)">${wisp(-26, 0, 14)}${wisp(4, 2.1, 18)}${wisp(30, 4.2, 12)}</g>
</svg>`;
}

/** The steam's frames laid out in a grid, so Chrome draws them all at once. */
export const STEAM = {
  frames: 24,
  columns: 6,
  widthPx: 480,
  heightPx: 360,
  framesPerSecond: 12,
} as const;

export function steamSheet(): string {
  const frames = Array.from({ length: STEAM.frames }, (_, index) =>
    steamFrame(index, STEAM.frames),
  ).join("");
  return `<!doctype html><html><head><meta charset="utf-8"><style>html,body{margin:0;padding:0;overflow:hidden}body{display:grid;grid-template-columns:repeat(${STEAM.columns},${STEAM.widthPx}px);line-height:0}svg{display:block}</style></head><body>${frames}</body></html>`;
}
