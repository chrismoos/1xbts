import type { AccessEvent, PagingEvent, TrafficEvent } from "@/lib/proto/bsc/v1/service";

export type LogEntry =
  | { kind: "tx"; stream: "paging"; id: string; ts: number; identity: string; sortKey: string; event: PagingEvent; seenCount: number }
  | { kind: "tx"; stream: "traffic"; id: string; ts: number; identity: string; sortKey: string; event: TrafficEvent; seenCount: number }
  | { kind: "rx"; id: string; ts: number; identity: string; sortKey: string; event: AccessEvent; seenCount: number };

export function makeLogEntryId(kind: "tx" | "rx", ts: number): string {
  if (typeof crypto !== "undefined" && typeof crypto.randomUUID === "function") {
    return `${kind}-${ts}-${crypto.randomUUID()}`;
  }
  return `${kind}-${ts}-${Math.random().toString(36).slice(2)}`;
}

export function makeSortKey(eventId: string | undefined, identity: string): string {
  if (eventId) {
    const parts = eventId.split("-");
    const suffix = parts[parts.length - 1];
    if (/^[0-9a-fA-F]+$/.test(suffix)) {
      return suffix.toLowerCase();
    }
    return eventId;
  }
  return identity;
}

export function sortLogEntries(entries: LogEntry[], maxEntries: number): LogEntry[] {
  return [...entries]
    .sort((left, right) => {
      if (left.ts !== right.ts) {
        return right.ts - left.ts;
      }
      return right.sortKey.localeCompare(left.sortKey);
    })
    .slice(0, maxEntries);
}

export function formatTime(tsUs: number): string {
  const tsMs = Math.floor(tsUs / 1000);
  const us = tsUs % 1000;
  const d = new Date(tsMs);
  const Y = d.getFullYear();
  const M = (d.getMonth() + 1).toString().padStart(2, "0");
  const D = d.getDate().toString().padStart(2, "0");
  const h = d.getHours().toString().padStart(2, "0");
  const m = d.getMinutes().toString().padStart(2, "0");
  const s = d.getSeconds().toString().padStart(2, "0");
  const ms = d.getMilliseconds().toString().padStart(3, "0");
  const usStr = us.toString().padStart(3, "0");
  return `${Y}-${M}-${D} ${h}:${m}:${s}.${ms}${usStr}`;
}
