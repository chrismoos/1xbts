export function formatEsn(esn: number): string {
  return `0x${(esn >>> 0).toString(16).toUpperCase().padStart(8, "0")}`;
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
