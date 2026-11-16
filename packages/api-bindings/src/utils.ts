// `Uint8Array.fromBase64` (TC39 "Uint8Array to/from base64", shipped in Safari 18.2) decodes in
// one native call. The hand-rolled loop below runs one JS iteration per byte, which is 13,540 of
// them for a single edit-buffer notification, so it is kept only as a fallback for engines that
// do not have the static method yet.
type FromBase64 = (base64: string) => Uint8Array;
const nativeFromBase64 = (Uint8Array as unknown as { fromBase64?: FromBase64 }).fromBase64;

export const b64ToBytes = (base64: string): Uint8Array => {
  if (nativeFromBase64) {
    return nativeFromBase64.call(Uint8Array, base64);
  }

  const binaryString = globalThis.atob(base64);
  const len = binaryString.length;
  const bytes = new Uint8Array(len);
  for (let i = 0; i < len; i += 1) {
    bytes[i] = binaryString.charCodeAt(i);
  }
  return bytes;
};

// Only reachable through `core.ts`'s `window.ipc.postMessage` fallback, which the desktop app
// never takes (`javascript_init(true)` fixes `supportApiProto`). It is kept because
// `tests/fig-api/fig-api-mock` runs with `javascript_init(false)` and that transport is what the
// vitest suite exercises. Do not delete without deleting that harness.
const base64abc = [
  "A",
  "B",
  "C",
  "D",
  "E",
  "F",
  "G",
  "H",
  "I",
  "J",
  "K",
  "L",
  "M",
  "N",
  "O",
  "P",
  "Q",
  "R",
  "S",
  "T",
  "U",
  "V",
  "W",
  "X",
  "Y",
  "Z",
  "a",
  "b",
  "c",
  "d",
  "e",
  "f",
  "g",
  "h",
  "i",
  "j",
  "k",
  "l",
  "m",
  "n",
  "o",
  "p",
  "q",
  "r",
  "s",
  "t",
  "u",
  "v",
  "w",
  "x",
  "y",
  "z",
  "0",
  "1",
  "2",
  "3",
  "4",
  "5",
  "6",
  "7",
  "8",
  "9",
  "+",
  "/",
];

export function bytesToBase64(bytes: Uint8Array): string {
  let result = "";
  let i;
  const l = bytes.length;
  for (i = 2; i < l; i += 3) {
    result += base64abc[bytes[i - 2] >> 2];
    result += base64abc[((bytes[i - 2] & 0x03) << 4) | (bytes[i - 1] >> 4)];
    result += base64abc[((bytes[i - 1] & 0x0f) << 2) | (bytes[i] >> 6)];
    result += base64abc[bytes[i] & 0x3f];
  }
  if (i === l + 1) {
    // 1 octet yet to write
    result += base64abc[bytes[i - 2] >> 2];
    result += base64abc[(bytes[i - 2] & 0x03) << 4];
    result += "==";
  }
  if (i === l) {
    // 2 octets yet to write
    result += base64abc[bytes[i - 2] >> 2];
    result += base64abc[((bytes[i - 2] & 0x03) << 4) | (bytes[i - 1] >> 4)];
    result += base64abc[(bytes[i - 1] & 0x0f) << 2];
    result += "=";
  }
  return result;
}
