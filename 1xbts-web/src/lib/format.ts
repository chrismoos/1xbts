export function formatEsn(esn: number): string {
  return `0x${(esn >>> 0).toString(16).toUpperCase().padStart(8, "0")}`;
}

export function formatMeid(meid: string): string {
  return meid.trim().toUpperCase();
}

// Decode a hex string ("AB12...") into bytes. Returns an empty array
// when the input is empty or odd-length.
export function hexToBytes(hex: string): Uint8Array {
  if (!hex || hex.length % 2 !== 0) return new Uint8Array();
  const out = new Uint8Array(hex.length / 2);
  for (let i = 0; i < out.length; i++) {
    out[i] = parseInt(hex.slice(i * 2, i * 2 + 2), 16);
  }
  return out;
}

// xxd-style hex + ASCII dump for opaque binary payloads.
export function formatHexDump(bytes: Uint8Array, bytesPerRow = 16): string {
  const lines: string[] = [];
  for (let off = 0; off < bytes.length; off += bytesPerRow) {
    const row = bytes.subarray(off, off + bytesPerRow);
    const hex = Array.from(row, b => b.toString(16).padStart(2, "0")).join(" ");
    const hexCol = hex.padEnd(bytesPerRow * 3 - 1, " ");
    const ascii = Array.from(row, b => (b >= 0x20 && b < 0x7f ? String.fromCharCode(b) : ".")).join("");
    lines.push(`${off.toString(16).padStart(6, "0")}  ${hexCol}  ${ascii}`);
  }
  return lines.join("\n");
}

export function formatTimeMs(ts: number): string {
  const d = new Date(ts);
  const Y = d.getFullYear();
  const M = (d.getMonth() + 1).toString().padStart(2, "0");
  const D = d.getDate().toString().padStart(2, "0");
  const h = d.getHours().toString().padStart(2, "0");
  const m = d.getMinutes().toString().padStart(2, "0");
  const s = d.getSeconds().toString().padStart(2, "0");
  const ms = d.getMilliseconds().toString().padStart(3, "0");
  return `${Y}-${M}-${D} ${h}:${m}:${s}.${ms}`;
}

// Share of total forward power as a percentage with its dB equivalent,
// e.g. "20.0% (−7.0 dB)".
export function formatPowerFraction(fraction: number): string {
  if (!(fraction > 0)) return "0% (off)";
  const db = 10 * Math.log10(fraction);
  return `${(fraction * 100).toFixed(1)}% (${db.toFixed(1)} dB)`;
}
