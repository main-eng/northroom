/**
 * The drawn pieces every picture is built from, as SVG: the logo, a bag of coffee, a mug, a bean,
 * and the soft shadows and grain that make a flat drawing read as a photograph.
 */
import { COLOUR, TYPE, type Coffee } from "./story";

/** The mark: a sun rising over two waves, in a ring, drawn in `colour` in a 100 by 100 box. */
export const logoMark = (colour: string, strokeWidth = 4) => `
<g fill="none" stroke="${colour}" stroke-width="${strokeWidth}" stroke-linecap="round">
  <circle cx="50" cy="50" r="46"/>
  <path d="M31 57 A19 19 0 0 1 69 57" fill="${colour}" stroke="none"/>
  <path d="M19 66 q7.75 -6 15.5 0 t15.5 0 t15.5 0 t15.5 0"/>
  <path d="M25 77 q6.25 -5 12.5 0 t12.5 0 t12.5 0 t12.5 0"/>
</g>`;

/** The wordmark under the mark, centred on x = 0 at its baseline, in `colour`. */
export const wordmark = (colour: string, sizePx: number) => `
<text x="0" y="0" text-anchor="middle" fill="${colour}" font-family="${TYPE.display}" font-size="${sizePx}" letter-spacing="${sizePx * 0.28}" font-weight="600">HARBOR</text>
<text x="0" y="${sizePx * 0.62}" text-anchor="middle" fill="${colour}" font-family="${TYPE.text}" font-size="${sizePx * 0.32}" letter-spacing="${sizePx * 0.12}" font-weight="600">COFFEE ROASTERS</text>`;

/** The logo as a file: the mark over the wordmark, on nothing. */
export function logoSvg(): string {
  return `<?xml version="1.0" encoding="UTF-8"?>
<svg xmlns="http://www.w3.org/2000/svg" width="480" height="420" viewBox="0 0 480 420">
  <title>Harbor Coffee Roasters</title>
  <g transform="translate(160 40) scale(1.6)">${logoMark(COLOUR.green)}</g>
  <g transform="translate(240 286)">${wordmark(COLOUR.ink, 64)}</g>
  <rect x="200" y="356" width="80" height="3" rx="1.5" fill="${COLOUR.green}"/>
</svg>
`;
}

/** Filters and the grain every scene shares, defined once per picture. */
export const sharedDefs = (seed: number) => `
<filter id="soft" x="-50%" y="-50%" width="200%" height="200%"><feGaussianBlur stdDeviation="14"/></filter>
<filter id="softer" x="-50%" y="-50%" width="200%" height="200%"><feGaussianBlur stdDeviation="34"/></filter>
<filter id="contact" x="-50%" y="-50%" width="200%" height="200%"><feGaussianBlur stdDeviation="5"/></filter>
<filter id="blur2" x="-20%" y="-20%" width="140%" height="140%"><feGaussianBlur stdDeviation="2"/></filter>
<filter id="grain" x="0" y="0" width="100%" height="100%">
  <feTurbulence type="fractalNoise" baseFrequency="0.85" numOctaves="2" seed="${seed}" stitchTiles="stitch"/>
  <feColorMatrix type="saturate" values="0"/>
  <feComponentTransfer><feFuncA type="table" tableValues="0 0.07"/></feComponentTransfer>
</filter>`;

/** A full-picture veil of fine grain, as a camera's sensor adds. */
export const grain = (width: number, height: number) =>
  `<rect width="${width}" height="${height}" filter="url(#grain)" style="mix-blend-mode:multiply"/>`;

export interface BagLook {
  body: string;
  /** The seal's and valve's colour, a shade off the body. */
  trim: string;
  label: string;
  labelInk: string;
  accent: string;
}

export const BAG_LOOKS = {
  green: {
    body: COLOUR.green,
    trim: COLOUR.greenDark,
    label: COLOUR.paper,
    labelInk: COLOUR.ink,
    accent: COLOUR.green,
  },
  ink: {
    body: "#1d1d1b",
    trim: "#0c0c0b",
    label: COLOUR.paper,
    labelInk: COLOUR.ink,
    accent: COLOUR.green,
  },
  paper: {
    body: "#ece6d9",
    trim: "#d9d1c0",
    label: COLOUR.green,
    labelInk: COLOUR.paper,
    accent: COLOUR.paper,
  },
} as const satisfies Record<string, BagLook>;

/**
 * A stand-up bag of coffee in a 300 by 440 box: a crimped seal, a one-way valve and a printed
 * label, lit from the left. `id` keeps its gradients apart from another bag's in the same picture.
 */
export function bag(id: string, look: BagLook, coffee: Coffee): string {
  const body =
    "M18 54 C18 47 22 43 29 43 L271 43 C278 43 282 47 282 54 L292 410 C292 425 282 436 267 436 L33 436 C18 436 8 425 8 410 Z";
  return `
<defs>
  <clipPath id="${id}-clip"><path d="${body}"/><rect x="16" y="8" width="268" height="44" rx="5"/></clipPath>
  <linearGradient id="${id}-light" x1="0" x2="1" y1="0" y2="0">
    <stop offset="0" stop-color="#000" stop-opacity="0.34"/>
    <stop offset="0.1" stop-color="#fff" stop-opacity="0.08"/>
    <stop offset="0.2" stop-color="#fff" stop-opacity="0.2"/>
    <stop offset="0.32" stop-color="#fff" stop-opacity="0.04"/>
    <stop offset="0.62" stop-color="#000" stop-opacity="0.02"/>
    <stop offset="0.88" stop-color="#000" stop-opacity="0.16"/>
    <stop offset="1" stop-color="#000" stop-opacity="0.4"/>
  </linearGradient>
  <linearGradient id="${id}-fall" x1="0" x2="0" y1="0" y2="1">
    <stop offset="0" stop-color="#fff" stop-opacity="0.1"/>
    <stop offset="0.25" stop-color="#fff" stop-opacity="0"/>
    <stop offset="0.86" stop-color="#000" stop-opacity="0"/>
    <stop offset="1" stop-color="#000" stop-opacity="0.24"/>
  </linearGradient>
  <pattern id="${id}-crimp" width="5" height="44" patternUnits="userSpaceOnUse">
    <rect width="2" height="44" fill="#000" opacity="0.14"/>
  </pattern>
</defs>
<path d="${body}" fill="${look.body}"/>
<rect x="16" y="8" width="268" height="44" rx="5" fill="${look.trim}"/>
<rect x="22" y="14" width="256" height="30" fill="url(#${id}-crimp)"/>
<path d="M268 8 l8 8 l-8 8" fill="none" stroke="#000" stroke-opacity="0.25" stroke-width="1.5"/>
<circle cx="150" cy="96" r="11" fill="${look.trim}"/>
<circle cx="150" cy="96" r="4.5" fill="none" stroke="#000" stroke-opacity="0.35" stroke-width="1.5"/>
<g>
  <rect x="54" y="148" width="192" height="232" rx="3" fill="${look.label}"/>
  <g transform="translate(131 166) scale(0.38)">${logoMark(look.accent === COLOUR.paper ? COLOUR.paper : COLOUR.green, 5)}</g>
  <g transform="translate(150 230)">${wordmark(look.labelInk, 20)}</g>
  <rect x="128" y="258" width="44" height="1.6" fill="${look.labelInk}" opacity="0.5"/>
  <text x="150" y="290" text-anchor="middle" font-family="${TYPE.display}" font-size="21" font-weight="600" fill="${look.labelInk}">${coffee.name}</text>
  <text x="150" y="309" text-anchor="middle" font-family="${TYPE.text}" font-size="9" font-weight="500" letter-spacing="1.2" fill="${look.labelInk}" opacity="0.8">${coffee.origin.toUpperCase()}</text>
  <text x="150" y="326" text-anchor="middle" font-family="${TYPE.text}" font-size="9" font-style="italic" fill="${look.labelInk}" opacity="0.75">${coffee.notes}</text>
  <text x="150" y="364" text-anchor="middle" font-family="${TYPE.text}" font-size="8" font-weight="600" letter-spacing="1.6" fill="${look.labelInk}" opacity="0.7">${coffee.grams} G · WHOLE BEAN</text>
</g>
<g clip-path="url(#${id}-clip)">
  <rect x="0" y="0" width="300" height="440" fill="url(#${id}-light)"/>
  <rect x="0" y="0" width="300" height="440" fill="url(#${id}-fall)"/>
  <path d="M30 52 C70 120 60 200 40 300" stroke="#fff" stroke-opacity="0.07" stroke-width="10" fill="none" filter="url(#blur2)"/>
  <path d="M262 60 C240 150 250 260 270 380" stroke="#000" stroke-opacity="0.08" stroke-width="12" fill="none" filter="url(#blur2)"/>
  <path d="M8 404 C80 396 220 396 292 404" stroke="#000" stroke-opacity="0.12" stroke-width="6" fill="none" filter="url(#blur2)"/>
</g>`;
}

/**
 * A ceramic mug seen from the side, in a 260 by 230 box, its opening at y = 40, with coffee in it.
 */
export function mug(id: string, glaze: { light: string; mid: string; dark: string }): string {
  return `
<defs>
  <linearGradient id="${id}-glaze" x1="0" x2="1">
    <stop offset="0" stop-color="${glaze.dark}"/>
    <stop offset="0.22" stop-color="${glaze.light}"/>
    <stop offset="0.55" stop-color="${glaze.mid}"/>
    <stop offset="1" stop-color="${glaze.dark}"/>
  </linearGradient>
  <radialGradient id="${id}-coffee" cx="0.42" cy="0.45" r="0.6">
    <stop offset="0" stop-color="#4a2b17"/>
    <stop offset="0.7" stop-color="#2b170b"/>
    <stop offset="1" stop-color="#6e4628"/>
  </radialGradient>
</defs>
<path d="M210 74 C266 70 270 156 206 166" fill="none" stroke="${glaze.dark}" stroke-width="20" stroke-linecap="round"/>
<path d="M210 74 C258 72 262 152 206 162" fill="none" stroke="${glaze.mid}" stroke-width="12" stroke-linecap="round"/>
<path d="M22 40 L218 40 L207 190 C205 205 195 214 180 214 L60 214 C45 214 35 205 33 190 Z" fill="url(#${id}-glaze)"/>
<ellipse cx="120" cy="40" rx="98" ry="15" fill="${glaze.light}"/>
<ellipse cx="120" cy="41" rx="90" ry="11.5" fill="${glaze.dark}"/>
<ellipse cx="120" cy="43" rx="87" ry="9.5" fill="url(#${id}-coffee)"/>
<ellipse cx="96" cy="41" rx="26" ry="2.6" fill="#fff" opacity="0.18"/>`;
}

/** A roasted bean, about 30 by 20, lit from the top left, its crease along it. */
export const bean = (x: number, y: number, angle: number, scale: number, shade: number) => `
<g transform="translate(${x.toFixed(1)} ${y.toFixed(1)}) rotate(${angle.toFixed(1)}) scale(${scale.toFixed(3)})">
  <ellipse cx="0" cy="0" rx="15" ry="10.5" fill="url(#bean${shade})"/>
  <path d="M-12.5 1.2 C-6 -4.5 5 5.5 12.5 -1" fill="none" stroke="#160b04" stroke-width="2" stroke-linecap="round" opacity="0.85"/>
  <path d="M-12 0 C-6 -5.5 5 4.5 12 -2" fill="none" stroke="#a57a52" stroke-width="0.8" stroke-linecap="round" opacity="0.35"/>
  <ellipse cx="-5" cy="-5.2" rx="6" ry="2.2" fill="#ffe8cc" opacity="0.1" transform="rotate(-12)"/>
</g>`;

/** The three bean shades `bean` picks from, as gradients. */
export const beanDefs = () =>
  [
    ["#6b4329", "#341d0f", "#170b04"],
    ["#5d3920", "#2b170a", "#120802"],
    ["#77502f", "#3e2414", "#1b0e06"],
  ]
    .map(
      ([light, mid, dark], index) =>
        `<radialGradient id="bean${index}" cx="0.36" cy="0.3" r="0.8"><stop offset="0" stop-color="${light}"/><stop offset="0.55" stop-color="${mid}"/><stop offset="1" stop-color="${dark}"/></radialGradient>`,
    )
    .join("");

/** `count` beans scattered by `random` over `area`, skipping any that land in `keepClear`. */
export function scatterBeans(
  random: () => number,
  count: number,
  area: { x: number; y: number; width: number; height: number },
  scale: { least: number; most: number },
  keepClear: (x: number, y: number) => boolean = () => false,
): string {
  const beans: string[] = [];
  while (beans.length < count) {
    const x = area.x + random() * area.width;
    const y = area.y + random() * area.height;
    const angle = random() * 360;
    const size = scale.least + random() * (scale.most - scale.least);
    const shade = Math.floor(random() * 3);
    if (keepClear(x, y)) continue;
    beans.push(bean(x, y, angle, size, shade));
  }
  return beans.join("");
}
