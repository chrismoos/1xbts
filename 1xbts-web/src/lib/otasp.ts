import { OtaspFeatureId } from "@/lib/proto/events/v1/msc";

// Spec-driven OTASP display helpers.
//
// Names map RESULT_CODE / BLOCK_ID / SessionOutcome / BAND_MODE_CAP values to
// human-readable strings per C.S0016-D Tables 3.5.1.2-1, 3.5.2 BLOCK_IDs, and
// 3.5.1.7-2 band-mode capability bits.

export type OtaspOutcomeName =
  | "OTASP_SESSION_OUTCOME_UNSPECIFIED"
  | "OTASP_SESSION_OUTCOME_COMMITTED"
  | "OTASP_SESSION_OUTCOME_SPC_REJECTED"
  | "OTASP_SESSION_OUTCOME_HLR_UNKNOWN"
  | "OTASP_SESSION_OUTCOME_REJECTED"
  | "OTASP_SESSION_OUTCOME_NO_CAPACITY"
  | "OTASP_SESSION_OUTCOME_PROTOCOL_ERROR"
  | "OTASP_SESSION_OUTCOME_NOTHING_TO_COMMIT"
  | "OTASP_SESSION_OUTCOME_TIMED_OUT";

export function outcomeNameToLabel(name: string | number | undefined): string {
  if (name == null) return "In progress";
  const s = typeof name === "number" ? outcomeNumberToName(name) : name;
  switch (s) {
    case "OTASP_SESSION_OUTCOME_COMMITTED":
      return "Committed";
    case "OTASP_SESSION_OUTCOME_NOTHING_TO_COMMIT":
      return "Nothing to commit";
    case "OTASP_SESSION_OUTCOME_SPC_REJECTED":
      return "SPC rejected";
    case "OTASP_SESSION_OUTCOME_HLR_UNKNOWN":
      return "HLR unknown";
    case "OTASP_SESSION_OUTCOME_REJECTED":
      return "Rejected";
    case "OTASP_SESSION_OUTCOME_NO_CAPACITY":
      return "No NAM capacity";
    case "OTASP_SESSION_OUTCOME_PROTOCOL_ERROR":
      return "Protocol error";
    case "OTASP_SESSION_OUTCOME_TIMED_OUT":
      return "Timed out";
    default:
      return "Unknown";
  }
}

export function outcomeNumberToName(value: number): OtaspOutcomeName {
  switch (value) {
    case 1: return "OTASP_SESSION_OUTCOME_COMMITTED";
    case 2: return "OTASP_SESSION_OUTCOME_SPC_REJECTED";
    case 3: return "OTASP_SESSION_OUTCOME_HLR_UNKNOWN";
    case 4: return "OTASP_SESSION_OUTCOME_REJECTED";
    case 5: return "OTASP_SESSION_OUTCOME_NO_CAPACITY";
    case 6: return "OTASP_SESSION_OUTCOME_PROTOCOL_ERROR";
    case 7: return "OTASP_SESSION_OUTCOME_NOTHING_TO_COMMIT";
    case 8: return "OTASP_SESSION_OUTCOME_TIMED_OUT";
    default: return "OTASP_SESSION_OUTCOME_UNSPECIFIED";
  }
}

export function outcomeBadgeColor(name: string | number | undefined): string {
  if (name == null) return "bg-badge-blue-bg text-badge-blue-text";
  const s = typeof name === "number" ? outcomeNumberToName(name) : name;
  switch (s) {
    case "OTASP_SESSION_OUTCOME_COMMITTED":
      return "bg-badge-green-bg text-badge-green-text";
    case "OTASP_SESSION_OUTCOME_NOTHING_TO_COMMIT":
      return "bg-badge-blue-bg text-badge-blue-text";
    default:
      return "bg-badge-red-bg text-badge-red-text";
  }
}

// C.S0016-D Table 3.5.1.2-1 — OTASP RESULT_CODE.
export function resultCodeLabel(code: number): string {
  switch (code) {
    case 0: return "Accepted";
    case 1: return "Rejected - Unknown";
    case 2: return "Rejected - Data Size Mismatch";
    case 3: return "Rejected - Protocol Version Mismatch";
    case 4: return "Rejected - Invalid Parameter";
    case 5: return "Rejected - SID/NID Length Mismatch";
    case 6: return "Rejected - Message Not Expected in this Mode";
    case 7: return "Rejected - BLOCK_ID Not Supported";
    case 8: return "Rejected - PRL Length Mismatch";
    case 9: return "Rejected - CRC Error";
    case 10: return "Rejected - Mobile Station Locked";
    case 11: return "Rejected - Invalid SPC";
    case 12: return "Rejected - SPC Change Denied by User";
    case 13: return "Rejected - Invalid SPASM";
    case 14: return "Rejected - BLOCK_ID Not Expected in this Mode";
    default:
      return `Other (0x${code.toString(16).toUpperCase().padStart(2, "0")})`;
  }
}

// C.S0016-D Table 3.5.1.7-1 — Feature Identifier.
const FEATURE_ID_RESERVED_FOR_FUTURE_STANDARDIZATION_START = 0x0C;
const FEATURE_ID_RESERVED_FOR_FUTURE_STANDARDIZATION_END = 0xBF;
const FEATURE_ID_MANUFACTURER_SPECIFIC_START = 0xC0;
const FEATURE_ID_MANUFACTURER_SPECIFIC_END = 0xFE;

const FEATURE_P_REV = {
  REV_1: 0x01,
  NAM_DOWNLOAD: {
    DATA_P_REV_2: 0x02,
    DATA_P_REV_3_WITH_EHRPD_IMSI: 0x03,
  },
  KEY_EXCHANGE: {
    A_KEY_PROVISIONING: 0x02,
    A_KEY_AND_3G_ROOT_KEY_PROVISIONING: 0x03,
    ROOT_KEY_PROVISIONING: 0x04,
    ENHANCED_3G_ROOT_KEY_PROVISIONING: 0x05,
    SERVICE_KEY_GENERATION: 0x06,
    EHRPD_ROOT_KEY_P_REV_7: 0x07,
    EHRPD_ROOT_KEY_P_REV_8: 0x08,
  },
  SSPR: {
    PREFERRED_ROAMING_LIST: 0x01,
    RESERVED: 0x02,
    EXTENDED_PREFERRED_ROAMING_LIST: 0x03,
  },
  PUZL_REV_2: 0x02,
  PACKET_DATA_3GPD_REV_3: 0x03,
  SECURE_MODE: {
    ROOT_KEY_UNAVAILABLE: 0x01,
    ROOT_KEY_AVAILABLE: 0x02,
  },
} as const;

export function featureIdLabel(id: number): string {
  switch (id) {
    case OtaspFeatureId.OTASP_FEATURE_ID_NAM_DOWNLOAD: return "NAM Download";
    case OtaspFeatureId.OTASP_FEATURE_ID_KEY_EXCHANGE: return "Key Exchange";
    case OtaspFeatureId.OTASP_FEATURE_ID_SSPR: return "SSPR";
    case OtaspFeatureId.OTASP_FEATURE_ID_SERVICE_PROGRAMMING_LOCK: return "Service Programming Lock";
    case OtaspFeatureId.OTASP_FEATURE_ID_OTASP: return "OTASP";
    case OtaspFeatureId.OTASP_FEATURE_ID_PUZL: return "PUZL";
    case OtaspFeatureId.OTASP_FEATURE_ID_PACKET_DATA_3GPD: return "3GPD Packet Data";
    case OtaspFeatureId.OTASP_FEATURE_ID_SECURE_MODE: return "Secure Mode";
    case OtaspFeatureId.OTASP_FEATURE_ID_MMD: return "MMD";
    case OtaspFeatureId.OTASP_FEATURE_ID_SYSTEM_TAG_DOWNLOAD: return "System Tag Download";
    case OtaspFeatureId.OTASP_FEATURE_ID_MMS: return "MMS";
    case OtaspFeatureId.OTASP_FEATURE_ID_MMSS: return "MMSS";
    default:
      if (
        id >= FEATURE_ID_RESERVED_FOR_FUTURE_STANDARDIZATION_START &&
        id <= FEATURE_ID_RESERVED_FOR_FUTURE_STANDARDIZATION_END
      ) {
        return "Reserved for future standardization";
      }
      if (
        id >= FEATURE_ID_MANUFACTURER_SPECIFIC_START &&
        id <= FEATURE_ID_MANUFACTURER_SPECIFIC_END
      ) {
        return "Manufacturer-specific feature";
      }
      return "Reserved";
  }
}

export function featureIdNumber(
  featureId: string | number | undefined,
  featureIdRaw?: number,
): number {
  if (featureIdRaw != null) return featureIdRaw;
  if (typeof featureId === "number") return featureId;
  if (typeof featureId === "string") {
    const value = OtaspFeatureId[featureId as keyof typeof OtaspFeatureId];
    if (typeof value === "number" && value >= 0) return value;
  }
  return OtaspFeatureId.OTASP_FEATURE_ID_NAM_DOWNLOAD;
}

// C.S0016-D Table 3.5.1.7-1 — FEATURE_P_REV is interpreted in the context
// of FEATURE_ID, not as a global revision space.
export function featurePRevLabel(id: number, rev: number): string {
  switch (id) {
    case OtaspFeatureId.OTASP_FEATURE_ID_NAM_DOWNLOAD:
      switch (rev) {
        case FEATURE_P_REV.NAM_DOWNLOAD.DATA_P_REV_2: return "NAM Download";
        case FEATURE_P_REV.NAM_DOWNLOAD.DATA_P_REV_3_WITH_EHRPD_IMSI:
          return "NAM Download with eHRPD IMSI provisioning";
        default: return "Unknown NAM revision";
      }
    case OtaspFeatureId.OTASP_FEATURE_ID_KEY_EXCHANGE:
      switch (rev) {
        case FEATURE_P_REV.KEY_EXCHANGE.A_KEY_PROVISIONING:
          return "A-key provisioning";
        case FEATURE_P_REV.KEY_EXCHANGE.A_KEY_AND_3G_ROOT_KEY_PROVISIONING:
          return "A-key and 3G Root Key provisioning";
        case FEATURE_P_REV.KEY_EXCHANGE.ROOT_KEY_PROVISIONING:
          return "3G Root Key provisioning";
        case FEATURE_P_REV.KEY_EXCHANGE.ENHANCED_3G_ROOT_KEY_PROVISIONING:
          return "Enhanced 3G Root Key provisioning";
        case FEATURE_P_REV.KEY_EXCHANGE.SERVICE_KEY_GENERATION:
          return "Service Key Generation";
        case FEATURE_P_REV.KEY_EXCHANGE.EHRPD_ROOT_KEY_P_REV_7:
          return "eHRPD Root Key revision 7";
        case FEATURE_P_REV.KEY_EXCHANGE.EHRPD_ROOT_KEY_P_REV_8:
          return "eHRPD Root Key revision 8";
        default: return "Unknown Key Exchange revision";
      }
    case OtaspFeatureId.OTASP_FEATURE_ID_SSPR:
      switch (rev) {
        case FEATURE_P_REV.SSPR.PREFERRED_ROAMING_LIST:
          return "Preferred Roaming List";
        case FEATURE_P_REV.SSPR.RESERVED:
          return "Reserved";
        case FEATURE_P_REV.SSPR.EXTENDED_PREFERRED_ROAMING_LIST:
          return "Extended Preferred Roaming List";
        default: return "Unknown SSPR revision";
      }
    case OtaspFeatureId.OTASP_FEATURE_ID_SERVICE_PROGRAMMING_LOCK:
    case OtaspFeatureId.OTASP_FEATURE_ID_OTASP:
    case OtaspFeatureId.OTASP_FEATURE_ID_MMD:
    case OtaspFeatureId.OTASP_FEATURE_ID_SYSTEM_TAG_DOWNLOAD:
    case OtaspFeatureId.OTASP_FEATURE_ID_MMS:
    case OtaspFeatureId.OTASP_FEATURE_ID_MMSS:
      return rev === FEATURE_P_REV.REV_1 ? "Revision 1" : "Unknown revision";
    case OtaspFeatureId.OTASP_FEATURE_ID_PUZL:
      return rev === FEATURE_P_REV.PUZL_REV_2 ? "Revision 2" : "Unknown PUZL revision";
    case OtaspFeatureId.OTASP_FEATURE_ID_PACKET_DATA_3GPD:
      return rev === FEATURE_P_REV.PACKET_DATA_3GPD_REV_3 ? "Revision 3" : "Unknown 3GPD revision";
    case OtaspFeatureId.OTASP_FEATURE_ID_SECURE_MODE:
      switch (rev) {
        case FEATURE_P_REV.SECURE_MODE.ROOT_KEY_UNAVAILABLE:
          return "Secure Mode without root key K";
        case FEATURE_P_REV.SECURE_MODE.ROOT_KEY_AVAILABLE:
          return "Secure Mode with root key K";
        default: return "Unknown Secure Mode revision";
      }
    default:
      if (
        id >= FEATURE_ID_RESERVED_FOR_FUTURE_STANDARDIZATION_START &&
        id <= FEATURE_ID_RESERVED_FOR_FUTURE_STANDARDIZATION_END
      ) {
        return "Reserved feature revision";
      }
      if (
        id >= FEATURE_ID_MANUFACTURER_SPECIFIC_START &&
        id <= FEATURE_ID_MANUFACTURER_SPECIFIC_END
      ) {
        return "Manufacturer-specific revision";
      }
      return "Reserved feature revision";
  }
}

// `feature` discriminates the family space the BLOCK_ID lives in.
// Multiple OTASP feature spaces overload the low BLOCK_IDs (NAM 0x00
// vs Home System Tag 0x00 vs PRL 0x00 vs MMS URI 0x00), so the same
// id resolves to a different label depending on which feature emitted
// the event. UNSPECIFIED / missing feature renders the raw hex —
// being honest beats confidently mis-labelling.
//
// Accepts `feature` as the proto enum string (e.g.
// "OTASP_BLOCK_FEATURE_NAM") or the numeric discriminant.
export function blockIdLabel(id: number, feature?: string | number): string {
  const raw = `BLOCK_ID 0x${id.toString(16).toUpperCase().padStart(2, "0")}`;
  const fam = blockFeatureName(feature);
  switch (fam) {
    case "NAM":
      switch (id) {
        case 0x00: return "CDMA/Analog NAM";
        case 0x01: return "Mobile Directory Number";
        case 0x02: return "CDMA NAM";
        case 0x03: return "IMSI_T";
        default:   return raw;
      }
    case "SYSTEM_TAG":
      return id === 0x00 ? "Home System Tag" : raw;
    case "MMS_URI":
      switch (id) {
        case 0x00: return "MMS URI";
        case 0x01: return "MMS URI Capability";
        default:   return raw;
      }
    case "PRL":
      switch (id) {
        case 0x00: return "PRL (classic)";
        case 0x01: return "PRL (extended)";
        default:   return raw;
      }
    default:
      return raw;
  }
}

function blockFeatureName(v?: string | number): string {
  if (typeof v === "string") {
    if (v.endsWith("NAM")) return "NAM";
    if (v.endsWith("SYSTEM_TAG")) return "SYSTEM_TAG";
    if (v.endsWith("MMS_URI")) return "MMS_URI";
    if (v.endsWith("PRL")) return "PRL";
    return "";
  }
  switch (v) {
    case 1: return "NAM";
    case 2: return "SYSTEM_TAG";
    case 3: return "MMS_URI";
    case 4: return "PRL";
    default: return "";
  }
}

// Format an ESN as 0x followed by 8 uppercase hex digits.
export function formatEsnHex(esn: number | undefined): string {
  if (esn == null || esn === 0) return "—";
  return `0x${(esn >>> 0).toString(16).toUpperCase().padStart(8, "0")}`;
}

// Format a MEID (14 hex chars) as XX:XX:XX:XX:XX:XX:XX uppercase.
export function formatMeidColon(meid: string | undefined): string {
  if (!meid) return "—";
  const s = meid.replace(/[^0-9a-fA-F]/g, "");
  if (s.length !== 14) return meid.toUpperCase();
  return s
    .toUpperCase()
    .match(/.{2}/g)!
    .join(":");
}

// Common feature-code / SO labels used in the SessionStart line.
export function serviceOptionLabel(so: number): string {
  if (so === 18) return "SO 18 OTASP";
  if (so === 19) return "SO 19 OTASP (Rate Set 2)";
  return `SO ${so}`;
}
