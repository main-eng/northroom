/**
 * The sounds, made sample by sample so every run makes the same ones: a soft theme of pads, a
 * plucked arpeggio and bass; a shop door bell; and the crackle of beans reaching first crack in
 * the roaster. Each is written as a 16-bit WAV that FFmpeg then encodes.
 */
import { seededRandom } from "./story";

export const SAMPLE_RATE_HZ = 48_000;

/** Two channels of samples between -1 and 1. */
export interface Stereo {
  left: Float32Array;
  right: Float32Array;
}

const silence = (seconds: number): Stereo => {
  const length = Math.round(seconds * SAMPLE_RATE_HZ);
  return { left: new Float32Array(length), right: new Float32Array(length) };
};

const noteHz = (midi: number) => 440 * 2 ** ((midi - 69) / 12);

/** Adds `sample` at `index` panned by `pan` (-1 left to 1 right) with equal power. */
function mix(into: Stereo, index: number, sample: number, pan: number): void {
  if (index < 0 || index >= into.left.length) return;
  const angle = ((pan + 1) * Math.PI) / 4;
  into.left[index] = (into.left[index] as number) + sample * Math.cos(angle);
  into.right[index] = (into.right[index] as number) + sample * Math.sin(angle);
}

/** A smooth rise over `attack` seconds, held, and a fall over `release` seconds before `length`. */
function envelope(t: number, length: number, attack: number, release: number): number {
  if (t < 0 || t > length) return 0;
  const rise = Math.min(t / attack, 1);
  const fall = Math.min((length - t) / release, 1);
  return Math.sin((rise * Math.PI) / 2) ** 2 * Math.sin((fall * Math.PI) / 2) ** 2;
}

/** A plucked string (Karplus-Strong) of `seconds` at `hz`, from a fixed burst of noise. */
function pluck(hz: number, seconds: number, random: () => number): Float32Array {
  const out = new Float32Array(Math.round(seconds * SAMPLE_RATE_HZ));
  const period = Math.round(SAMPLE_RATE_HZ / hz);
  const ring = new Float32Array(period);
  let previous = 0;
  for (let index = 0; index < period; index += 1) {
    // Smoothed noise makes a softer, rounder pluck.
    previous = previous * 0.5 + (random() * 2 - 1) * 0.5;
    ring[index] = previous;
  }
  for (let index = 0; index < out.length; index += 1) {
    const at = index % period;
    const next = (at + 1) % period;
    const value = ring[at] as number;
    out[index] = value;
    ring[at] = 0.4985 * (value + (ring[next] as number));
  }
  return out;
}

/** A small stereo room (Freeverb's combs and all-passes), `wet` of it mixed over the dry sound. */
function reverb(sound: Stereo, wet: number): Stereo {
  const scale = SAMPLE_RATE_HZ / 44_100;
  const combLengths = [1116, 1188, 1277, 1356, 1422, 1491];
  const allPassLengths = [556, 441, 341];
  const room = (input: Float32Array, spread: number): Float32Array => {
    const out = new Float32Array(input.length);
    const combs = combLengths.map((length) => ({
      buffer: new Float32Array(Math.round((length + spread) * scale)),
      at: 0,
      store: 0,
    }));
    const allPasses = allPassLengths.map((length) => ({
      buffer: new Float32Array(Math.round((length + spread) * scale)),
      at: 0,
    }));
    for (let index = 0; index < input.length; index += 1) {
      const dry = (input[index] as number) * 0.015;
      let sum = 0;
      for (const comb of combs) {
        const delayed = comb.buffer[comb.at] as number;
        comb.store = delayed * 0.75 + comb.store * 0.25;
        comb.buffer[comb.at] = dry + comb.store * 0.86;
        comb.at = (comb.at + 1) % comb.buffer.length;
        sum += delayed;
      }
      for (const pass of allPasses) {
        const delayed = pass.buffer[pass.at] as number;
        pass.buffer[pass.at] = sum + delayed * 0.5;
        pass.at = (pass.at + 1) % pass.buffer.length;
        sum = delayed - sum;
      }
      out[index] = sum;
    }
    return out;
  };
  const left = room(sound.left, 0);
  const right = room(sound.right, 23);
  const blend = (dry: Float32Array, room: Float32Array) =>
    dry.map((sample, index) => sample * (1 - wet) + (room[index] as number) * wet * 3);
  return { left: blend(sound.left, left), right: blend(sound.right, right) };
}

/** Fades the ends and scales the loudest sample to `peak`. */
function finish(
  sound: Stereo,
  fadeInSeconds: number,
  fadeOutSeconds: number,
  peak: number,
): Stereo {
  const length = sound.left.length;
  let loudest = 0;
  for (let index = 0; index < length; index += 1) {
    loudest = Math.max(
      loudest,
      Math.abs(sound.left[index] as number),
      Math.abs(sound.right[index] as number),
    );
  }
  const gain = loudest === 0 ? 0 : peak / loudest;
  const fadeIn = fadeInSeconds * SAMPLE_RATE_HZ;
  const fadeOut = fadeOutSeconds * SAMPLE_RATE_HZ;
  const shape = (sample: number, index: number) => {
    const level = Math.min(index / fadeIn, (length - 1 - index) / fadeOut, 1);
    return sample * gain * Math.sin((Math.max(level, 0) * Math.PI) / 2);
  };
  return { left: sound.left.map(shape), right: sound.right.map(shape) };
}

/** The theme's chords, two seconds each: Fmaj7, Em7, Dm7, Cmaj7, as MIDI notes, root first. */
const CHORDS = [
  [41, 53, 57, 60, 64],
  [40, 52, 55, 59, 62],
  [38, 50, 53, 57, 60],
  [36, 48, 52, 55, 59],
] as const;
const CHORD_SECONDS = 2;
const STEP_SECONDS = 0.25;

/** The Harbor theme, `seconds` long: warm pads, a plucked arpeggio, a soft bass and a shaker. */
export function theme(seconds: number): Stereo {
  const sound = silence(seconds);
  const random = seededRandom(5);
  const chordCount = Math.ceil(seconds / CHORD_SECONDS);
  for (let chordIndex = 0; chordIndex < chordCount; chordIndex += 1) {
    const [root, ...tones] = CHORDS[chordIndex % CHORDS.length] as readonly number[];
    const start = chordIndex * CHORD_SECONDS;
    const first = Math.round(start * SAMPLE_RATE_HZ);
    const length = CHORD_SECONDS + 0.6;
    for (let index = 0; index < length * SAMPLE_RATE_HZ; index += 1) {
      const t = index / SAMPLE_RATE_HZ;
      const time = start + t;
      const pad = envelope(t, length, 0.7, 0.8);
      tones.forEach((tone, voice) => {
        const hz = noteHz(tone);
        const tone1 = Math.sin(2 * Math.PI * hz * 1.0015 * time);
        const tone2 = Math.sin(2 * Math.PI * hz * 0.9985 * time + voice);
        const sample = (tone1 + tone2 + 0.2 * Math.sin(4 * Math.PI * hz * time)) * 0.03 * pad;
        mix(sound, first + index, sample, voice % 2 === 0 ? -0.35 : 0.35);
      });
      const bassHz = noteHz((root as number) - 0);
      const bass = envelope(t, CHORD_SECONDS + 0.2, 0.04, 0.5);
      const bassSample =
        (Math.sin(2 * Math.PI * bassHz * time) + 0.25 * Math.sin(4 * Math.PI * bassHz * time)) *
        0.11 *
        bass;
      mix(sound, first + index, bassSample, 0);
    }
    if (chordIndex === 0) continue;
    const pattern = [0, 2, 1, 3, 2, 1, 3, 2];
    pattern.forEach((toneIndex, step) => {
      if (random() < 0.18) return;
      const tone = (tones[toneIndex] as number) + 12;
      const string = pluck(noteHz(tone), 1.4, random);
      const at = Math.round((start + step * STEP_SECONDS) * SAMPLE_RATE_HZ);
      const pan = step % 2 === 0 ? -0.3 : 0.3;
      string.forEach((sample, index) => mix(sound, at + index, sample * 0.11, pan));
    });
    for (let step = 0; step < 8; step += 1) {
      const at = Math.round((start + step * STEP_SECONDS + 0.125) * SAMPLE_RATE_HZ);
      let filtered = 0;
      for (let index = 0; index < 0.05 * SAMPLE_RATE_HZ; index += 1) {
        const noise = random() * 2 - 1;
        const high = noise - filtered;
        filtered = filtered * 0.6 + noise * 0.4;
        const decay = Math.exp(-index / (0.012 * SAMPLE_RATE_HZ));
        mix(sound, at + index, high * decay * (step % 2 === 0 ? 0.018 : 0.012), 0.45);
      }
    }
  }
  return finish(reverb(sound, 0.28), 0.4, 1.6, 0.85);
}

/** The shop door's bell: two struck notes, ding and dong, ringing out (Risset's bell partials). */
export function doorBell(): Stereo {
  const sound = silence(3.4);
  const partials = [
    [0.56, 1, 1],
    [0.92, 0.67, 0.9],
    [1.19, 1, 0.65],
    [1.7, 1.8, 0.55],
    [2.0, 2.67, 0.325],
    [2.74, 1.67, 0.35],
    [3.0, 1.46, 0.25],
    [3.76, 1.33, 0.2],
    [4.07, 1.33, 0.15],
  ] as const;
  const strikes = [
    { at: 0.05, hz: 1568, pan: -0.25 },
    { at: 0.5, hz: 1245, pan: 0.25 },
  ];
  for (const strike of strikes) {
    const first = Math.round(strike.at * SAMPLE_RATE_HZ);
    const ringSeconds = 2.6;
    for (let index = 0; index < ringSeconds * SAMPLE_RATE_HZ; index += 1) {
      const t = index / SAMPLE_RATE_HZ;
      const attack = Math.min(t / 0.002, 1);
      let sample = 0;
      for (const [ratio, amplitude, decay] of partials) {
        sample +=
          Math.sin(2 * Math.PI * strike.hz * ratio * t) *
          amplitude *
          Math.exp(-t / (decay * ringSeconds * 0.45));
      }
      mix(sound, first + index, sample * attack * 0.05, strike.pan);
    }
  }
  return finish(reverb(sound, 0.3), 0.001, 0.6, 0.8);
}

/** One second-order band-pass filter (RBJ), centred on `hz` with quality `q`. */
function bandPass(hz: number, q: number) {
  const omega = (2 * Math.PI * hz) / SAMPLE_RATE_HZ;
  const alpha = Math.sin(omega) / (2 * q);
  const a0 = 1 + alpha;
  const b0 = alpha / a0;
  const b2 = -alpha / a0;
  const a1 = (-2 * Math.cos(omega)) / a0;
  const a2 = (1 - alpha) / a0;
  let x1 = 0;
  let x2 = 0;
  let y1 = 0;
  let y2 = 0;
  return (x: number) => {
    const y = b0 * x + b2 * x2 - a1 * y1 - a2 * y2;
    x2 = x1;
    x1 = x;
    y2 = y1;
    y1 = y;
    return y;
  };
}

/** Beans tumbling in the roaster's drum as they reach first crack: a rumble, and pops that build. */
export function firstCrack(seconds: number): Stereo {
  const sound = silence(seconds);
  const random = seededRandom(43);
  let brown = 0;
  const hiss = bandPass(3200, 0.6);
  for (let index = 0; index < sound.left.length; index += 1) {
    const t = index / SAMPLE_RATE_HZ;
    brown = (brown + (random() * 2 - 1) * 0.02) * 0.995;
    const drum = 0.75 + 0.25 * Math.sin(2 * Math.PI * 0.9 * t);
    const fan = hiss(random() * 2 - 1) * 0.05;
    mix(sound, index, brown * 2.2 * drum + fan, 0);
  }
  // Cracks come slowly at first, most often two thirds of the way in, then thin out.
  for (let t = 0.3; t < seconds - 0.2; ) {
    const busy = Math.exp(-(((t / seconds - 0.62) / 0.22) ** 2));
    const perSecond = 0.8 + busy * 9;
    t += -Math.log(1 - random()) / perSecond;
    const filter = bandPass(1400 + random() * 2600, 3 + random() * 4);
    const loudness = 0.15 + random() * 0.6;
    const pan = random() * 1.6 - 0.8;
    const first = Math.round(t * SAMPLE_RATE_HZ);
    const length = Math.round((0.004 + random() * 0.01) * SAMPLE_RATE_HZ);
    for (let index = 0; index < length * 6; index += 1) {
      const burst = index < length ? random() * 2 - 1 : 0;
      const decay = Math.exp(-index / (length * 0.9));
      mix(sound, first + index, filter(burst) * decay * loudness * 3, pan);
    }
  }
  return finish(reverb(sound, 0.12), 0.8, 1.2, 0.8);
}

/** A 16-bit PCM WAV of `sound`. */
export function wavFile(sound: Stereo): Buffer {
  const frames = sound.left.length;
  const data = Buffer.alloc(frames * 4);
  for (let index = 0; index < frames; index += 1) {
    const clamp = (sample: number) =>
      Math.max(-32_768, Math.min(32_767, Math.round(sample * 32_767)));
    data.writeInt16LE(clamp(sound.left[index] as number), index * 4);
    data.writeInt16LE(clamp(sound.right[index] as number), index * 4 + 2);
  }
  const header = Buffer.alloc(44);
  header.write("RIFF", 0, "ascii");
  header.writeUInt32LE(36 + data.length, 4);
  header.write("WAVEfmt ", 8, "ascii");
  header.writeUInt32LE(16, 16);
  header.writeUInt16LE(1, 20);
  header.writeUInt16LE(2, 22);
  header.writeUInt32LE(SAMPLE_RATE_HZ, 24);
  header.writeUInt32LE(SAMPLE_RATE_HZ * 4, 28);
  header.writeUInt16LE(4, 32);
  header.writeUInt16LE(16, 34);
  header.write("data", 36, "ascii");
  header.writeUInt32LE(data.length, 40);
  return Buffer.concat([header, data]);
}
