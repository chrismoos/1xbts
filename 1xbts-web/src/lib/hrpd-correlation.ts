import {
  HrpdTrafficReason,
  type HrpdTrafficEvent,
  type HrpdUati,
} from "@/lib/proto/events/v1/an";

export interface HrpdSessionKeyFields {
  uati: number;
  colorCode?: number;
  fullUati?: HrpdUati;
}

export interface HrpdPacketSessionKeyFields {
  accessTechnology?: string;
  mobileAddress?: string;
  sessionId?: string;
  trafficWalshCode?: number;
}

type Uint64Like = string | number | bigint | null | undefined;

function uint64LikeToBigInt(value: Uint64Like): bigint | undefined {
  if (value == null || value === "") return undefined;
  try {
    return BigInt(value);
  } catch {
    return undefined;
  }
}

export function hrpdTimestampNsToUs(timestampNs: Uint64Like): number {
  const ns = uint64LikeToBigInt(timestampNs);
  if (ns == null || ns <= 0n) return Date.now() * 1000;
  const us = Number(ns / 1000n);
  return Number.isFinite(us) && us > 0 ? us : Date.now() * 1000;
}

export function hrpdTimestampNsToMs(timestampNs: Uint64Like): number | undefined {
  const ns = uint64LikeToBigInt(timestampNs);
  if (ns == null || ns <= 0n) return undefined;
  const ms = Number(ns / 1_000_000n);
  return Number.isFinite(ms) && ms > 0 ? ms : undefined;
}

export function isHrpdTelemetryTrafficEvent(event: HrpdTrafficEvent): boolean {
  return (
    event.reason === HrpdTrafficReason.HRPD_TRAFFIC_REASON_DRC_UPDATED ||
    event.reason === HrpdTrafficReason.HRPD_TRAFFIC_REASON_REVERSE_PILOT_SNR_UPDATED
  );
}

export function uatiHexDigits(uati: number): string {
  return (uati >>> 0).toString(16).padStart(8, "0");
}

export function uatiHex(uati: number): string {
  return `0x${uatiHexDigits(uati).toUpperCase()}`;
}

export function hrpdUatiBytes(fullUati?: HrpdUati): number[] {
  if (!fullUati?.value) return [];
  const value = fullUati.value as unknown;
  if (value instanceof Uint8Array) return Array.from(value);
  if (Array.isArray(value)) return value.filter((byte) => typeof byte === "number");
  if (typeof value === "string") {
    try {
      return Array.from(atob(value), (char) => char.charCodeAt(0));
    } catch {
      return [];
    }
  }
  return [];
}

export function formatHrpdFullUati(fullUati?: HrpdUati): string | undefined {
  const bytes = hrpdUatiBytes(fullUati);
  if (bytes.length !== 16) return undefined;
  return bytes.map((byte) => byte.toString(16).padStart(2, "0").toUpperCase()).join(":");
}

export function hrpdReceiveUati(sessionUati: number, colorCode?: number): number {
  if (colorCode == null) return sessionUati >>> 0;
  return (((colorCode & 0xff) << 24) | (sessionUati & 0x00ff_ffff)) >>> 0;
}

export function hrpdRelatedUatis(session: HrpdSessionKeyFields): number[] {
  const values = [
    session.uati >>> 0,
    session.fullUati?.compactUati32 ? session.fullUati.compactUati32 >>> 0 : 0,
    hrpdReceiveUati(session.uati, session.colorCode),
    session.fullUati?.colorCode != null
      ? hrpdReceiveUati(session.fullUati.compactUati32 || session.uati, session.fullUati.colorCode)
      : 0,
    session.uati & 0x00ff_ffff,
  ].filter((value) => value !== 0);
  return [...new Set(values)];
}

export function parseHrpdPacketMobileAddress(address?: string): number | undefined {
  const match = address?.match(/^(?:hrpd-uati-session|uati):([0-9a-fA-F]{1,8})$/);
  if (!match) return undefined;
  const value = Number.parseInt(match[1], 16);
  return Number.isFinite(value) ? value >>> 0 : undefined;
}

export function parseHrpdA10SessionId(sessionId?: string): number | undefined {
  const match = sessionId?.match(/^hrpd-a10-([0-9a-fA-F]{8})-[0-9a-fA-F]{4}$/);
  if (!match) return undefined;
  const value = Number.parseInt(match[1], 16);
  return Number.isFinite(value) ? value >>> 0 : undefined;
}

export function hrpdPacketSessionUati(session: HrpdPacketSessionKeyFields): number | undefined {
  return (
    parseHrpdPacketMobileAddress(session.mobileAddress) ??
    parseHrpdA10SessionId(session.sessionId) ??
    (session.trafficWalshCode ? session.trafficWalshCode >>> 0 : undefined)
  );
}

export function hrpdSessionMatchesPacket(
  hrpdSession: HrpdSessionKeyFields,
  packetSession: HrpdPacketSessionKeyFields,
): boolean {
  if (packetSession.accessTechnology !== "HRPD") return false;
  const packetUati = hrpdPacketSessionUati(packetSession);
  if (packetUati == null) return false;
  const candidates = hrpdRelatedUatis(hrpdSession);
  return candidates.some(
    (candidate) =>
      candidate === packetUati ||
      (candidate & 0x00ff_ffff) === (packetUati & 0x00ff_ffff),
  );
}
