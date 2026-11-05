import {
  hrpdRelatedUatis,
  parseHrpdPacketMobileAddress,
  type HrpdPacketSessionKeyFields,
} from "@/lib/hrpd-correlation";
import type { MobileDirectoryEntry } from "@/lib/mobile-directory";
import {
  HardwareVerdict,
  SessionState,
  type HrpdHardwareIdResponse,
  type Session,
} from "@/lib/proto/an/v1/service";
import { ListA10SessionsResponse } from "@/lib/proto/pcf/v1/service";

// C.S0024 HardwareIDType values.
const HARDWARE_ID_TYPE_MEID = 0x00ffff;
const HARDWARE_ID_TYPE_ESN = 0x010000;
const MEID_BYTES = 7;
const ESN_BYTES = 4;

// Forward data rate in kbps per DRC value, from the C.S0024 DRC rate table.
// Values 13 and 14 exist only on Rev A.
const DRC_RATE_KBPS = [0, 38.4, 76.8, 153.6, 307.2, 307.2, 614.4, 614.4, 921.6, 1228.8, 1228.8, 1843.2, 2457.6, 1536, 3072];

export type EvdoVerdict = "admitted" | "refused" | "unknown";
export type EvdoLinkState = "connected" | "dormant" | "idle";

export interface EvdoHardwareId {
  meid?: string;
  esn?: number;
  label: string;
  value: string;
}

export interface EvdoSummary {
  uati: number;
  colorCode: number;
  connected: boolean;
  verdict: EvdoVerdict;
  refusedReason?: string;
  macIndex?: number;
  drcIndex?: number;
  lastActivityAtMs?: number;
  packetPhase?: string;
  packetSessionId?: string;
  nai?: string;
}

export interface EvdoIdentity {
  hardwareId?: EvdoHardwareId;
  subscriberId?: string;
  imsi?: string;
  nai?: string;
  mobile?: MobileDirectoryEntry;
  matchedBy: string[];
}

export interface EvdoPacketSession extends HrpdPacketSessionKeyFields {
  sessionId: string;
  phase: string;
  subscriberId?: string;
  subscriberImsi?: string;
  imsi?: string;
  meid?: string;
  esn?: number;
  peerIp?: string;
  mnId?: string;
  pcfSessionId?: number;
  mnSessionReferenceId?: number;
  createdAtMs?: number;
}

// An A10 connection as the PCF reports it. The AN sets its A8 key and A9
// Correlation ID to the session UATI.
export interface EvdoA10Session {
  pcfSessionId: number;
  mnSessionReferenceId: number;
  a8Key: number;
  correlationId?: number;
  imsi?: string;
}

const UATI_24_MASK = 0x00ff_ffff;
const A10_PACKET_SESSION_ID = /^hrpd-a10-([0-9a-fA-F]{8})-([0-9a-fA-F]{4})$/;
const HEX = 16;

export function bytesToArray(value: unknown): number[] {
  if (value instanceof Uint8Array) return Array.from(value);
  if (Array.isArray(value)) return value.filter((byte) => typeof byte === "number");
  if (typeof value === "string") {
    try {
      return Array.from(atob(value), (char) => char.charCodeAt(0));
    } catch {
      return [];
    }
  }
  if (value && typeof value === "object") {
    const buffer = value as { data?: unknown; length?: unknown };
    if (Array.isArray(buffer.data)) return buffer.data.filter((byte) => typeof byte === "number");
    if (typeof buffer.length === "number") {
      const bytes: number[] = [];
      for (let i = 0; i < buffer.length; i += 1) {
        const byte = (value as Record<number, unknown>)[i];
        if (typeof byte === "number") bytes.push(byte);
      }
      return bytes;
    }
  }
  return [];
}

function hex(bytes: number[]): string {
  return bytes.map((byte) => byte.toString(16).padStart(2, "0")).join("").toUpperCase();
}

export function normalizeMeid(meid?: string): string | undefined {
  const value = meid?.replace(/[^0-9a-fA-F]/g, "").toUpperCase();
  return value ? value : undefined;
}

export function evdoHardwareId(response?: HrpdHardwareIdResponse): EvdoHardwareId | undefined {
  if (!response) return undefined;
  const bytes = bytesToArray(response.hardwareIdValue);
  if (response.hardwareIdType === HARDWARE_ID_TYPE_MEID && bytes.length === MEID_BYTES) {
    const meid = hex(bytes);
    return { meid, label: "MEID", value: meid };
  }
  if (response.hardwareIdType === HARDWARE_ID_TYPE_ESN && bytes.length === ESN_BYTES) {
    const esn = bytes.reduce((acc, byte) => (acc << 8) | byte, 0) >>> 0;
    return { esn, label: "ESN", value: `0x${hex(bytes)}` };
  }
  return {
    label: `Hardware ID 0x${response.hardwareIdType.toString(16).toUpperCase().padStart(6, "0")}`,
    value: hex(bytes) || "-",
  };
}

export function drcRateLabel(drcIndex?: number): string | undefined {
  if (drcIndex == null) return undefined;
  const kbps = DRC_RATE_KBPS[drcIndex];
  if (kbps == null) return `DRC ${drcIndex}`;
  if (kbps === 0) return "Null rate";
  return kbps >= 1000 ? `${Number((kbps / 1000).toFixed(4))} Mbps` : `${kbps} kbps`;
}

export function evdoVerdict(session: Pick<Session, "admission">): EvdoVerdict {
  switch (session.admission?.hardwareVerdict) {
    case HardwareVerdict.HARDWARE_VERDICT_ADMITTED:
      return "admitted";
    case HardwareVerdict.HARDWARE_VERDICT_REFUSED:
      return "refused";
    default:
      return "unknown";
  }
}

export function evdoLinkState(connected: boolean, packetPhase?: string): EvdoLinkState {
  if (connected) return "connected";
  return packetPhase === "dormant" || packetPhase === "active" ? "dormant" : "idle";
}

export function isEvdoSessionOpen(session: Pick<Session, "state">): boolean {
  return session.state === SessionState.SESSION_STATE_OPEN || session.state === SessionState.SESSION_STATE_AMP_SETUP;
}

function packetA10Key(packet: EvdoPacketSession): { pcfSessionId: number; reference?: number } | undefined {
  if (packet.pcfSessionId != null) {
    return { pcfSessionId: packet.pcfSessionId, reference: packet.mnSessionReferenceId };
  }
  const match = packet.sessionId.match(A10_PACKET_SESSION_ID);
  if (!match) return undefined;
  return { pcfSessionId: Number.parseInt(match[1], HEX), reference: Number.parseInt(match[2], HEX) };
}

function sessionUatis24(session: Session): Set<number> {
  return new Set(hrpdRelatedUatis(session).map((uati) => uati & UATI_24_MASK));
}

// An error body or a PCF that is not running yields no sessions, so the join
// falls back to IMSI matching.
export function parseA10Sessions(raw: unknown): EvdoA10Session[] {
  const body = raw as { sessions?: unknown[]; error?: string } | undefined;
  if (!body || body.error || !Array.isArray(body.sessions)) return [];
  return ListA10SessionsResponse.fromJSON(body).sessions;
}

export function a10ForEvdo(session: Session, a10s: EvdoA10Session[]): EvdoA10Session | undefined {
  const uatis = sessionUatis24(session);
  return a10s.find(
    (a10) =>
      uatis.has(a10.a8Key & UATI_24_MASK) ||
      (a10.correlationId != null && uatis.has(a10.correlationId & UATI_24_MASK)),
  );
}

function newestFirst<T extends EvdoPacketSession>(left: T, right: T): number {
  return (right.createdAtMs ?? 0) - (left.createdAtMs ?? 0);
}

// The PCF links the UATI to its A10 key, and the PDSN session names the same
// key.
export function packetSessionForEvdo<T extends EvdoPacketSession>(
  session: Session,
  packets: T[],
  a10s: EvdoA10Session[] = [],
): T | undefined {
  const open = packets.filter((packet) => packet.phase !== "closed");
  const a10 = a10ForEvdo(session, a10s);
  if (a10) {
    const viaPcf = open.find((packet) => {
      const key = packetA10Key(packet);
      return (
        key?.pcfSessionId === a10.pcfSessionId &&
        (key.reference == null || key.reference === a10.mnSessionReferenceId)
      );
    });
    if (viaPcf) return viaPcf;
  }
  const imsi = session.admission?.accessAuthImsi || a10?.imsi;
  if (imsi) {
    const viaImsi = open
      .filter((packet) => packetA10Key(packet) != null && packet.mnId === imsi)
      .sort(newestFirst)[0];
    if (viaImsi) return viaImsi;
  }
  const uatis = sessionUatis24(session);
  return open.find((packet) => {
    const uati = parseHrpdPacketMobileAddress(packet.mobileAddress);
    return uati != null && uatis.has(uati & UATI_24_MASK);
  });
}

export function evdoIdentity(
  session: Session,
  mobiles: MobileDirectoryEntry[],
  packet?: EvdoPacketSession,
  a10?: EvdoA10Session,
): EvdoIdentity {
  const hardwareId = evdoHardwareId(session.hardwareIdResponse);
  const subscriberId = session.admission?.subscriberId || packet?.subscriberId || undefined;
  const imsi =
    session.admission?.accessAuthImsi ||
    a10?.imsi ||
    packet?.subscriberImsi ||
    packet?.imsi ||
    (packet && packetA10Key(packet) ? packet.mnId : undefined) ||
    undefined;
  const nai = session.admission?.accessAuthNai || undefined;
  const matchedBy: string[] = [];
  const meid = normalizeMeid(hardwareId?.meid);
  let mobile: MobileDirectoryEntry | undefined;
  if (subscriberId) {
    mobile = mobiles.find((candidate) => candidate.subscriberId === subscriberId);
    if (mobile) matchedBy.push("subscriber");
  }
  if (meid) {
    const byMeid = mobiles.find((candidate) => normalizeMeid(candidate.meid) === meid);
    if (byMeid && (!mobile || byMeid === mobile)) {
      mobile = byMeid;
      matchedBy.push("MEID");
    }
  }
  if (hardwareId?.esn != null) {
    const byEsn = mobiles.find((candidate) => candidate.esn === hardwareId.esn);
    if (byEsn && (!mobile || byEsn === mobile)) {
      mobile = byEsn;
      matchedBy.push("ESN");
    }
  }
  if (imsi) {
    const byImsi = mobiles.find((candidate) => candidate.imsi === imsi);
    if (byImsi && (!mobile || byImsi === mobile)) {
      mobile = byImsi;
      matchedBy.push("IMSI");
    }
  }
  return { hardwareId, subscriberId, imsi, nai, mobile, matchedBy };
}

export function evdoSummary(session: Session, packet?: EvdoPacketSession): EvdoSummary {
  return {
    uati: session.uati >>> 0,
    colorCode: session.colorCode,
    connected: session.connected,
    verdict: evdoVerdict(session),
    refusedReason: session.admission?.reason || undefined,
    macIndex: session.macIndex,
    drcIndex: session.drcIndex,
    lastActivityAtMs: session.lastActivityAtMs || undefined,
    packetPhase: packet?.phase,
    packetSessionId: packet?.sessionId,
    nai: session.admission?.accessAuthNai || undefined,
  };
}

export function formatAge(ms?: number, now = Date.now()): string {
  if (!ms) return "—";
  const seconds = Math.max(0, Math.round((now - ms) / 1000));
  if (seconds < 60) return `${seconds} s`;
  const minutes = Math.round(seconds / 60);
  if (minutes < 60) return `${minutes} min`;
  return `${Math.round(minutes / 60)} h`;
}
