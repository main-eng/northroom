// The browser's `atob`, for JavaScript engines without one (JavaScriptCore's bare context):
// shiki decodes its Oniguruma WebAssembly with it, and without it the grammars never load.
// Each four letters of base64 give three bytes, each byte one character of the answer;
// letters outside the alphabet, padding among them, are passed over.
const ALPHABET = "ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
const CHUNK = 8192;

function decodeBase64(text: string): string {
  const values = new Int8Array(128).fill(-1);
  for (let index = 0; index < ALPHABET.length; index++) values[ALPHABET.charCodeAt(index)] = index;
  const bytes: number[] = [];
  let bits = 0;
  let count = 0;
  for (let index = 0; index < text.length; index++) {
    const code = text.charCodeAt(index);
    const value = code < 128 ? values[code] : -1;
    if (value < 0) continue;
    bits = (bits << 6) | value;
    count += 6;
    if (count >= 8) {
      count -= 8;
      bytes.push((bits >> count) & 0xff);
    }
  }
  let out = "";
  for (let start = 0; start < bytes.length; start += CHUNK)
    out += String.fromCharCode(...bytes.slice(start, start + CHUNK));
  return out;
}

globalThis.atob ??= decodeBase64;
