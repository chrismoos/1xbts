import {
  HrpdAccessReason,
  HrpdDirection,
  HrpdSessionReason,
  HrpdTrafficReason,
  type HrpdAccessEvent,
  type HrpdDecodedMessage,
  type HrpdSessionEvent,
  type HrpdTrafficEvent,
} from "@/lib/proto/events/v1/an";

export type EvdoEventStream = "session" | "access" | "traffic";
export type EvdoEvent = HrpdSessionEvent | HrpdAccessEvent | HrpdTrafficEvent;

export interface EvdoMessageField {
  key: string;
  value: string;
}

export interface EvdoMessageInfo {
  name: string;
  protocol?: string;
  fields: EvdoMessageField[];
  note?: string;
  extraMessages: number;
  direction: "tx" | "rx" | "none";
  channel: string;
  macIndex?: number;
  drc?: number;
  bytes?: number;
}

// C.S0024 protocol types, matching the AN's stream 0 protocol names.
const PROTOCOL_NAMES: Record<number, string> = {
  0x01: "Control Channel MAC",
  0x02: "Access Channel MAC",
  0x03: "Forward Traffic Channel MAC",
  0x04: "Reverse Traffic Channel MAC",
  0x05: "Key Exchange",
  0x06: "Authentication",
  0x07: "Encryption",
  0x08: "Security",
  0x09: "Packet Consolidation",
  0x0a: "Air Link Management",
  0x0b: "Initialization State",
  0x0c: "Idle State",
  0x0d: "Connected State",
  0x0e: "Route Update",
  0x0f: "Overhead Messages",
  0x10: "Session Management",
  0x11: "Address Management",
  0x12: "Session Configuration",
  0x13: "Stream",
  0x14: "Default Signaling",
  0x15: "Default Packet",
  0x16: "Default Packet",
  0x17: "Default Packet",
  0x1b: "Multimode Capability Discovery",
};

const SESSION_REASON_NAMES: Partial<Record<HrpdSessionReason, string>> = {
  [HrpdSessionReason.HRPD_SESSION_REASON_OPENED]: "Session opened",
  [HrpdSessionReason.HRPD_SESSION_REASON_CLOSED]: "Session closed",
  [HrpdSessionReason.HRPD_SESSION_REASON_UATI_ASSIGNED]: "UATI assigned",
  [HrpdSessionReason.HRPD_SESSION_REASON_PROTOCOL_NEGOTIATED]: "Protocols negotiated",
  [HrpdSessionReason.HRPD_SESSION_REASON_UATI_RELEASED]: "UATI released",
};

const ACCESS_REASON_NAMES: Partial<Record<HrpdAccessReason, string>> = {
  [HrpdAccessReason.HRPD_ACCESS_REASON_CONNECTION_REQUEST]: "Connection Request",
  [HrpdAccessReason.HRPD_ACCESS_REASON_UATI_REQUEST]: "UATI Request",
  [HrpdAccessReason.HRPD_ACCESS_REASON_ROUTE_UPDATE]: "Route Update",
  [HrpdAccessReason.HRPD_ACCESS_REASON_KEEP_ALIVE]: "Keep Alive",
  [HrpdAccessReason.HRPD_ACCESS_REASON_UNKNOWN]: "Unknown access message",
};

const TRAFFIC_REASON_NAMES: Partial<Record<HrpdTrafficReason, string>> = {
  [HrpdTrafficReason.HRPD_TRAFFIC_REASON_FRAME_DECODED]: "Traffic frame",
  [HrpdTrafficReason.HRPD_TRAFFIC_REASON_DRC_UPDATED]: "DRC report",
  [HrpdTrafficReason.HRPD_TRAFFIC_REASON_ACK_RECEIVED]: "ACK",
  [HrpdTrafficReason.HRPD_TRAFFIC_REASON_CONNECTION_CLOSE]: "Connection closed",
  [HrpdTrafficReason.HRPD_TRAFFIC_REASON_REVERSE_PILOT_SNR_UPDATED]: "Reverse pilot SNR",
};

const BASE64_PADDING = /=+$/;
const BASE64_BITS_PER_CHAR = 6;
const BITS_PER_BYTE = 8;

// A decoded message reports protocol type 0 when the decoder did not know it.
// The physical layer, which is type 0, sends no signaling messages, so 0 reads
// as unknown here.
export function evdoProtocolName(protocolType?: number): string | undefined {
  return protocolType ? PROTOCOL_NAMES[protocolType] : undefined;
}

const DEFAULT_PACKET_PREFIX = "DefaultPacket";

function decodedProtocol(decoded?: HrpdDecodedMessage): string | undefined {
  if (!decoded) return undefined;
  return (
    evdoProtocolName(decoded.protocolType) ??
    (decoded.typeName.startsWith(DEFAULT_PACKET_PREFIX) ? PROTOCOL_NAMES[0x15] : undefined)
  );
}

export function readableMessageName(typeName: string): string {
  return typeName
    .replace(/([a-z0-9])([A-Z])/g, "$1 $2")
    .replace(/([A-Z]+)([A-Z][a-z])/g, "$1 $2")
    .trim();
}

function byteCount(payload: unknown, explicit?: number): number | undefined {
  if (explicit) return explicit;
  if (payload instanceof Uint8Array) return payload.length || undefined;
  if (typeof payload === "string" && payload) {
    return Math.floor((payload.replace(BASE64_PADDING, "").length * BASE64_BITS_PER_CHAR) / BITS_PER_BYTE) || undefined;
  }
  return undefined;
}

// A decoder summary reads "<TypeName> [<Protocol word>] key=value ...". The type
// and protocol words are already shown in their own columns, so only the
// key=value fields and any remaining words are kept.
function summaryFields(decoded: HrpdDecodedMessage): { fields: EvdoMessageField[]; note?: string } {
  const leading = new Set([decoded.typeName, (evdoProtocolName(decoded.protocolType) ?? "").replace(/ /g, "")]);
  const fields: EvdoMessageField[] = [];
  const words: string[] = [];
  for (const token of decoded.summary.split(/\s+/).filter(Boolean)) {
    const eq = token.indexOf("=");
    if (eq > 0) {
      fields.push({ key: token.slice(0, eq).replace(/_/g, " "), value: token.slice(eq + 1) });
    } else if (fields.length > 0 || words.length > 0 || !leading.has(token)) {
      words.push(token);
    }
  }
  const note = words.join(" ");
  return { fields, note: note || undefined };
}

function decodedMacIndex(fields: EvdoMessageField[]): number | undefined {
  const value = Number.parseInt(fields.find((field) => field.key === "mac")?.value ?? "", 10);
  return Number.isFinite(value) ? value : undefined;
}

function directionOf(direction?: HrpdDirection): EvdoMessageInfo["direction"] {
  if (direction === HrpdDirection.HRPD_DIRECTION_TX) return "tx";
  if (direction === HrpdDirection.HRPD_DIRECTION_RX) return "rx";
  return "none";
}

export function evdoMessageInfo(stream: EvdoEventStream, event: EvdoEvent): EvdoMessageInfo {
  if (stream === "session") {
    const session = event as HrpdSessionEvent;
    return {
      name: SESSION_REASON_NAMES[session.reason] ?? "Session event",
      fields: [{ key: "color", value: String(session.colorCode) }],
      extraMessages: 0,
      direction: "none",
      channel: "Session",
    };
  }
  const decodedMessages = (event as HrpdAccessEvent | HrpdTrafficEvent).decodedMessages ?? [];
  const decoded = decodedMessages.find((message) => message.typeName || message.summary);
  const base = decoded ? summaryFields(decoded) : { fields: [] as EvdoMessageField[] };
  if (stream === "access") {
    const access = event as HrpdAccessEvent;
    return {
      name: decoded?.typeName ? readableMessageName(decoded.typeName) : (ACCESS_REASON_NAMES[access.reason] ?? "Access capsule"),
      protocol: decodedProtocol(decoded),
      ...base,
      extraMessages: Math.max(0, decodedMessages.length - 1),
      direction: directionOf(access.direction) === "none" ? "rx" : directionOf(access.direction),
      channel: "Access",
      bytes: byteCount(access.payload, access.payloadLengthBytes),
    };
  }
  const traffic = event as HrpdTrafficEvent;
  const telemetry =
    traffic.reason === HrpdTrafficReason.HRPD_TRAFFIC_REASON_REVERSE_PILOT_SNR_UPDATED && traffic.reversePilotSnrDbTenths
      ? [{ key: "snr", value: `${(traffic.reversePilotSnrDbTenths / 10).toFixed(1)} dB` }]
      : [];
  return {
    name: decoded?.typeName ? readableMessageName(decoded.typeName) : (TRAFFIC_REASON_NAMES[traffic.reason] ?? "Traffic event"),
    protocol: decodedProtocol(decoded),
    fields: [...base.fields, ...telemetry],
    note: base.note,
    extraMessages: Math.max(0, decodedMessages.length - 1),
    direction: directionOf(traffic.direction),
    channel: "Traffic",
    macIndex: traffic.macIndex || decodedMacIndex(base.fields),
    drc: traffic.drcValue || undefined,
    bytes: byteCount(traffic.payload, traffic.payloadLengthBytes),
  };
}
