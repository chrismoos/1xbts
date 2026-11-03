import type { CellId } from "@/lib/proto/bsc/v1/service";
import { cellToken } from "@/lib/cell";

export const FER_WARN_PCT = 1;
export const FER_BAD_PCT = 3;

export function ferColor(pct: number): string {
  if (pct < FER_WARN_PCT) return "text-accent-green";
  if (pct < FER_BAD_PCT) return "text-accent-amber";
  return "text-accent-red";
}

interface TrafficChannelRow<M, P> {
  walshCode?: number;
  channelType: string;
  cell?: CellId;
  mobile?: M;
  serviceOption?: number;
  trafficPower?: P;
  powerFraction?: number;
}

export interface TrafficCall<M, P> {
  key: string;
  walshCode: number;
  cell?: CellId;
  mobile?: M;
  serviceOption?: number;
  power?: P;
  powerFraction?: number;
}

// The channel list reports a call's forward and reverse channels as separate rows on one (cell, Walsh) pair.
export function mergeCalls<M, P>(channels: TrafficChannelRow<M, P>[]): TrafficCall<M, P>[] {
  const calls = new Map<string, TrafficCall<M, P>>();
  for (const ch of channels) {
    if (ch.channelType !== "traffic" || ch.walshCode == null) continue;
    const key = `${cellToken(ch.cell) ?? ""}:${ch.walshCode}`;
    const call = calls.get(key) ?? { key, walshCode: ch.walshCode, cell: ch.cell };
    call.mobile ??= ch.mobile;
    call.serviceOption ??= ch.serviceOption;
    call.power ??= ch.trafficPower;
    call.powerFraction ??= ch.powerFraction;
    calls.set(key, call);
  }
  return [...calls.values()].sort((a, b) => a.walshCode - b.walshCode);
}

export function lifetimeFerPct(power?: { framesTotal: number; framesCrcError: number }): number | null {
  return power && power.framesTotal > 0 ? (100 * power.framesCrcError) / power.framesTotal : null;
}
