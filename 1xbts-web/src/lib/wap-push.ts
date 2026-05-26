// Parse a WAP MMS M-Notification.ind PDU into labeled fields.
//
// The bytes typically arrive wrapped by the CDMA WAP teleservice framing
// from WAP-259 §6.5 (MSG_TYPE TOTAL_SEGMENTS SEGMENT_NUMBER SRC_PORT
// DST_PORT then the WSP PDU). We sniff for that prefix and skip past it
// when present. The remaining bytes are MMS encoding 1.0 per OMA-WAP-209.
//
// Returns null on anything we can't confidently parse so callers fall
// through to a hex dump.

export interface ParsedNotification {
  transactionId?: string;
  mmsVersion?: string;
  messageClass?: string;
  messageSize?: number;
  expiryRelativeSeconds?: number;
  contentLocation?: string;
  from?: string;
  // The unparsed remainder, if any — exposed so the caller can render
  // it as hex when fields didn't account for the whole PDU.
  unparsedBytes?: number;
}

// MMS field tokens (high bit set on the wire). Values from
// OMA-WAP-MMS-ENC-V1_2 Table 13.
const FIELD_BCC = 0x81;
const FIELD_CC = 0x82;
const FIELD_CONTENT_LOCATION = 0x83;
const FIELD_CONTENT_TYPE = 0x84;
const FIELD_DATE = 0x85;
const FIELD_DELIVERY_REPORT = 0x86;
const FIELD_DELIVERY_TIME = 0x87;
const FIELD_EXPIRY = 0x88;
const FIELD_FROM = 0x89;
const FIELD_MESSAGE_CLASS = 0x8a;
const FIELD_MESSAGE_ID = 0x8b;
const FIELD_MESSAGE_TYPE = 0x8c;
const FIELD_MMS_VERSION = 0x8d;
const FIELD_MESSAGE_SIZE = 0x8e;
const FIELD_PRIORITY = 0x8f;
const FIELD_READ_REPLY = 0x90;
const FIELD_REPORT_ALLOWED = 0x91;
const FIELD_RESPONSE_STATUS = 0x92;
const FIELD_RESPONSE_TEXT = 0x93;
const FIELD_SENDER_VISIBILITY = 0x94;
const FIELD_STATUS = 0x95;
const FIELD_SUBJECT = 0x96;
const FIELD_TO = 0x97;
const FIELD_TRANSACTION_ID = 0x98;

const MESSAGE_TYPE_M_NOTIFICATION_IND = 0x82;

const MESSAGE_CLASSES: Record<number, string> = {
  0x80: "Personal",
  0x81: "Advertisement",
  0x82: "Informational",
  0x83: "Auto",
};

class Reader {
  private buf: Uint8Array;
  private pos = 0;

  constructor(buf: Uint8Array) {
    this.buf = buf;
  }

  remaining(): number { return this.buf.length - this.pos; }

  peekU8(): number | null {
    return this.pos < this.buf.length ? this.buf[this.pos] : null;
  }

  readU8(): number {
    if (this.pos >= this.buf.length) throw new Error("eof");
    return this.buf[this.pos++];
  }

  // Read a WSP "uintvar" (variable-length unsigned int, 7 bits per byte,
  // continuation bit in high position).
  readUintvar(): number {
    let value = 0;
    let shift = 0;
    for (let i = 0; i < 5; i++) {
      const b = this.readU8();
      value = (value << 7) | (b & 0x7f);
      if ((b & 0x80) === 0) return value;
      shift += 7;
      if (shift > 28) throw new Error("uintvar overflow");
    }
    throw new Error("uintvar too long");
  }

  // Read a WSP "Text-string": NUL-terminated ASCII bytes. The first byte
  // may be a 0x7F "quote" marker that we skip.
  readTextString(): string {
    let start = this.pos;
    if (this.buf[start] === 0x7f) start = ++this.pos;
    while (this.pos < this.buf.length && this.buf[this.pos] !== 0x00) this.pos++;
    const slice = this.buf.subarray(start, this.pos);
    if (this.pos < this.buf.length) this.pos++; // consume NUL
    return new TextDecoder("utf-8", { fatal: false }).decode(slice);
  }

  // Read a WSP "Long-integer" or "Short-integer".
  readInteger(): number {
    const first = this.readU8();
    if (first >= 0x80) return first & 0x7f; // short integer
    // long integer: first byte is length
    const len = first;
    if (len === 0 || len > 30) throw new Error("integer length out of range");
    let value = 0;
    for (let i = 0; i < len; i++) value = (value * 256) + this.readU8();
    return value;
  }

  // Read a WSP "Value-length": either length byte (<=30) or 0x1f followed
  // by a uintvar. Returns the length of the following value.
  readValueLength(): number {
    const first = this.readU8();
    if (first <= 30) return first;
    if (first === 0x1f) return this.readUintvar();
    throw new Error(`unexpected value-length byte 0x${first.toString(16)}`);
  }
}

// Detect the WAP-259 §6.5 framing prefix our bridge produces.
function stripWdpFraming(bytes: Uint8Array): Uint8Array {
  if (bytes.length < 7) return bytes;
  // The framing is plausible if byte 1 (TOTAL_SEGMENTS) and byte 2
  // (SEGMENT_NUMBER) are small and byte 5 (DST_PORT high) == 0x0B (port
  // 0x0B84 = 2948 = WAP Push connectionless).
  const totalSeg = bytes[1];
  const seg = bytes[2];
  const dstHi = bytes[5];
  if (totalSeg >= 1 && totalSeg <= 8 && seg < totalSeg && dstHi === 0x0b) {
    return bytes.subarray(7);
  }
  return bytes;
}

// Peel a WSP connectionless Push PDU wrapper (WAP-230 §8.2.4):
//
//   TID(1) PDU-Type(1) Headers-Len(uintvar) Content-Type+Headers Data
//
// Returns the inner Data (the MMS PDU) when the wrapper is recognized,
// otherwise returns the input unchanged.
function stripWspPush(bytes: Uint8Array): Uint8Array {
  if (bytes.length < 3) return bytes;
  // PDU type 0x06 = Push (connection-less and connection-mode both use
  // this value for the connectionless transport variant we deal with).
  if (bytes[1] !== 0x06) return bytes;
  // Headers-Len is a uintvar starting at byte 2.
  let i = 2;
  let len = 0;
  while (i < bytes.length && i < 7) {
    const b = bytes[i++];
    len = (len << 7) | (b & 0x7f);
    if ((b & 0x80) === 0) break;
  }
  const dataStart = i + len;
  if (dataStart > bytes.length) return bytes;
  return bytes.subarray(dataStart);
}

export function parseMNotificationInd(bytes: Uint8Array): ParsedNotification | null {
  if (bytes.length === 0) return null;
  const inner = stripWspPush(stripWdpFraming(bytes));

  let result: ParsedNotification = {};
  const r = new Reader(inner);

  // MMS PDUs start with header sequence; X-Mms-Message-Type comes
  // first. The token can appear in either order with X-Mms-Transaction-ID
  // and MMS-Version, but Message-Type=m-notification-ind is the marker
  // we use to confirm we're looking at the right thing.
  let sawNotificationInd = false;
  let safety = 64;

  try {
    while (r.remaining() > 0 && safety-- > 0) {
      const token = r.peekU8();
      if (token === null) break;
      if ((token & 0x80) === 0) break; // not a known header token
      r.readU8();
      switch (token) {
        case FIELD_MESSAGE_TYPE: {
          const t = r.readU8();
          if (t === MESSAGE_TYPE_M_NOTIFICATION_IND) sawNotificationInd = true;
          break;
        }
        case FIELD_TRANSACTION_ID:
          result.transactionId = r.readTextString();
          break;
        case FIELD_MMS_VERSION: {
          const v = r.readU8();
          const major = (v & 0x70) >> 4;
          const minor = v & 0x0f;
          result.mmsVersion = `${major}.${minor}`;
          break;
        }
        case FIELD_FROM: {
          const len = r.readValueLength();
          // From has a leading 0x80 ("Address-present-token") or 0x81
          // ("Insert-address-token"). Skip the token byte then read text.
          if (len === 0) break;
          const subEnd = (r as unknown as { pos: number }).pos + len;
          const marker = r.readU8();
          if (marker === 0x80) {
            result.from = r.readTextString();
          }
          // Make sure we land at the declared end.
          (r as unknown as { pos: number }).pos = subEnd;
          break;
        }
        case FIELD_MESSAGE_CLASS: {
          const next = r.peekU8();
          if (next !== null && next >= 0x80) {
            r.readU8();
            result.messageClass = MESSAGE_CLASSES[next] ?? `0x${next.toString(16)}`;
          } else {
            result.messageClass = r.readTextString();
          }
          break;
        }
        case FIELD_MESSAGE_SIZE:
          result.messageSize = r.readInteger();
          break;
        case FIELD_EXPIRY: {
          const len = r.readValueLength();
          const subEnd = (r as unknown as { pos: number }).pos + len;
          const tok = r.readU8();
          // 0x80 = Absolute-token, 0x81 = Relative-token. Most MMSCs
          // emit relative seconds.
          if (tok === 0x81) {
            result.expiryRelativeSeconds = r.readInteger();
          }
          (r as unknown as { pos: number }).pos = subEnd;
          break;
        }
        case FIELD_CONTENT_LOCATION:
          result.contentLocation = r.readTextString();
          break;
        case FIELD_CONTENT_TYPE:
        case FIELD_DATE:
        case FIELD_DELIVERY_REPORT:
        case FIELD_PRIORITY:
        case FIELD_READ_REPLY:
        case FIELD_REPORT_ALLOWED:
        case FIELD_SENDER_VISIBILITY:
        case FIELD_STATUS:
          // Single-byte well-known values; skip.
          r.readU8();
          break;
        case FIELD_MESSAGE_ID:
        case FIELD_RESPONSE_TEXT:
        case FIELD_SUBJECT:
        case FIELD_TO:
        case FIELD_CC:
        case FIELD_BCC:
          r.readTextString();
          break;
        case FIELD_RESPONSE_STATUS:
        case FIELD_DELIVERY_TIME:
          r.readU8();
          break;
        default:
          // Unknown — bail rather than scramble pos.
          return sawNotificationInd ? result : null;
      }
    }
  } catch {
    return sawNotificationInd ? result : null;
  }

  if (!sawNotificationInd) return null;
  if (r.remaining() > 0) result.unparsedBytes = r.remaining();
  return result;
}
