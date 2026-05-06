"use client";

import type { AccessEvent, PagingEvent, TrafficEvent } from "@/lib/proto/bsc/v1/service";

function stableStringify(value: unknown): string {
  if (value === null || value === undefined) {
    return "null";
  }
  if (typeof value !== "object") {
    return JSON.stringify(value);
  }
  if (Array.isArray(value)) {
    return `[${value.map((item) => stableStringify(item)).join(",")}]`;
  }

  const entries = Object.entries(value as Record<string, unknown>)
    .filter(([, item]) => item !== undefined)
    .sort(([left], [right]) => left.localeCompare(right));

  return `{${entries
    .map(([key, item]) => `${JSON.stringify(key)}:${stableStringify(item)}`)
    .join(",")}}`;
}

export function fingerprintPagingEvent(event: PagingEvent): string {
  const { timestampUs: _timestampUs, ...stableFields } = event;
  return stableStringify(stableFields);
}

export function fingerprintAccessEvent(event: AccessEvent): string {
  const {
    timestampUs: _timestampUs,
    snrDb: _snrDb,
    signalPowerDb: _signalPowerDb,
    demodQualityPct: _demodQualityPct,
    rxPowerDbm: _rxPowerDbm,
    ...stableFields
  } = event;
  return stableStringify(stableFields);
}

export function fingerprintTrafficEvent(event: TrafficEvent): string {
  const { timestampUs: _timestampUs, ...stableFields } = event;
  return stableStringify(stableFields);
}

export function hasRecentDuplicate<T extends { kind: "tx" | "rx"; fingerprint: string; ts: number }>(
  entries: T[],
  next: T,
  windowMs = 2000
): boolean {
  return entries.some(
    (entry) =>
      entry.kind === next.kind &&
      entry.fingerprint === next.fingerprint &&
      Math.abs(entry.ts - next.ts) <= windowMs
  );
}
