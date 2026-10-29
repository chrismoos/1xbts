import type { AccessEvent } from "@/lib/proto/bsc/v1/service";
import type { MobileDirectoryEntry } from "@/lib/mobile-directory";

export interface AuthVerdict {
  time?: string;
  procedure?: string;
  result?: string;
  serviceRejected?: boolean;
  imsi?: string;
  esn?: number;
  baseStation?: string;
  pendingSsdUpdate?: boolean;
  rand?: number;
  authData?: number;
  authDataSource?: string;
  count?: number;
}

/** ok: verified. fail: failed. pending: allowed without verification until an SSD update. */
export type VerdictKind = "ok" | "fail" | "pending";

const AUTH_DATA_SOURCE_LABELS: Record<string, string> = {
  AUTH_DATA_SOURCE_IMSI_S1: "IMSI_S1",
  AUTH_DATA_SOURCE_DIALED_DIGITS: "dialed digits",
  AUTH_DATA_SOURCE_DIALED_DIGITS_ZERO_FILLED: "dialed digits, zero-filled",
  AUTH_DATA_SOURCE_BASE_STATION: "from the base station",
};

const PROCEDURE_LABELS: Record<string, string> = {
  AUTHENTICATION_PROCEDURE_REGISTRATION: "Registration",
  AUTHENTICATION_PROCEDURE_ORIGINATION: "Origination",
  AUTHENTICATION_PROCEDURE_PAGE_RESPONSE: "Page response",
  AUTHENTICATION_PROCEDURE_UNIQUE_CHALLENGE: "Unique challenge",
  AUTHENTICATION_PROCEDURE_SSD_UPDATE: "SSD update",
  AUTHENTICATION_PROCEDURE_COUNT_UPDATE: "COUNT update",
  AUTHENTICATION_PROCEDURE_OTASP_KEY_EXCHANGE: "OTASP A-key",
  AUTHENTICATION_PROCEDURE_OTASP_REAUTHENTICATION: "OTASP re-authentication",
  AUTHENTICATION_PROCEDURE_DATA_BURST: "SMS",
};

const RESULT_LABELS: Record<string, string> = {
  AUTHENTICATION_RESULT_SUCCESS: "Success",
  AUTHENTICATION_RESULT_SIGNATURE_MISMATCH: "AUTHR mismatch",
  AUTHENTICATION_RESULT_COUNT_MISMATCH: "COUNT mismatch",
  AUTHENTICATION_RESULT_NOT_PROVISIONED: "No A-key",
  AUTHENTICATION_RESULT_PARAMETERS_MISSING: "Parameters missing",
  AUTHENTICATION_RESULT_RAND_MISMATCH: "RAND mismatch",
  AUTHENTICATION_RESULT_HLR_ERROR: "HLR error",
  AUTHENTICATION_RESULT_SSD_UPDATE_REJECTED: "SSD update rejected",
  AUTHENTICATION_RESULT_TIMEOUT: "Timeout",
};

export function procedureLabel(verdict: AuthVerdict): string {
  return (verdict.procedure && PROCEDURE_LABELS[verdict.procedure]) || "Unknown";
}

export function resultLabel(verdict: AuthVerdict): string {
  return (verdict.result && RESULT_LABELS[verdict.result]) || "Unknown";
}

export function verdictSucceeded(verdict: AuthVerdict): boolean {
  return verdict.result === "AUTHENTICATION_RESULT_SUCCESS";
}

export function verdictKind(verdict: AuthVerdict): VerdictKind {
  if (verdictSucceeded(verdict)) return "ok";
  return verdict.pendingSsdUpdate ? "pending" : "fail";
}

export function verdictStatus(verdict: AuthVerdict): string {
  switch (verdictKind(verdict)) {
    case "ok":
      return "Verified";
    case "pending":
      return "Pending SSD update";
    case "fail":
      return verdict.serviceRejected ? "Rejected" : "Failed, allowed";
  }
}

export function verdictTitle(verdict: AuthVerdict): string {
  return `${procedureLabel(verdict)}: ${verdictStatus(verdict)} (${resultLabel(verdict)})`;
}

export function authDataLabel(verdict: AuthVerdict): string | null {
  if (verdict.authData == null) return null;
  const hex = `0x${verdict.authData.toString(16).toUpperCase().padStart(6, "0")}`;
  const source = verdict.authDataSource && AUTH_DATA_SOURCE_LABELS[verdict.authDataSource];
  return source ? `${hex} (${source})` : hex;
}

export function verdictFeedback(verdict: AuthVerdict): string {
  const procedure = verdict.procedure;
  const onCall =
    procedure === "AUTHENTICATION_PROCEDURE_ORIGINATION" ||
    procedure === "AUTHENTICATION_PROCEDURE_PAGE_RESPONSE";
  if (verdictKind(verdict) === "pending") {
    return onCall
      ? "AUTHR did not match. The call connects only after an SSD update and a unique challenge pass on its traffic channel."
      : "The network has an A-key but no SSD for this mobile. The registration is accepted unverified, and the first call runs an SSD update.";
  }
  switch (verdict.result) {
    case "AUTHENTICATION_RESULT_SUCCESS":
      if (verdict.authDataSource === "AUTH_DATA_SOURCE_DIALED_DIGITS_ZERO_FILLED") {
        return "Verified with zero-filled AUTH_DATA. The dialed number has fewer than six digits.";
      }
      if (procedure === "AUTHENTICATION_PROCEDURE_SSD_UPDATE") {
        return "The mobile accepted AUTHBS. Mobile and network now share a new SSD.";
      }
      return "The mobile's signature matches the network's SSD.";
    case "AUTHENTICATION_RESULT_SIGNATURE_MISMATCH":
      return "The mobile signed with a different SSD or A-key than the network holds.";
    case "AUTHENTICATION_RESULT_NOT_PROVISIONED":
      return "The network has no A-key for this mobile.";
    case "AUTHENTICATION_RESULT_PARAMETERS_MISSING":
      return "The cell requires authentication but the mobile sent no AUTHR.";
    case "AUTHENTICATION_RESULT_RAND_MISMATCH":
      return "RANDC matches no RAND the cell broadcast, so AUTHR could not be checked.";
    case "AUTHENTICATION_RESULT_SSD_UPDATE_REJECTED":
      return "The mobile rejected AUTHBS. Its A-key differs from the network's.";
    case "AUTHENTICATION_RESULT_TIMEOUT":
      return "The mobile did not answer in time.";
    case "AUTHENTICATION_RESULT_HLR_ERROR":
      return "The HLR could not be reached.";
    default:
      return resultLabel(verdict);
  }
}

export function verdictTimeUs(verdict: AuthVerdict): number | null {
  if (!verdict.time) return null;
  const ms = Date.parse(verdict.time);
  return Number.isNaN(ms) ? null : ms * 1000;
}

/** C.S0005-E Table 2.7.3-1: SSD Update Confirmation and Rejection Orders. */
const ORDER_SSD_UPDATE_RESULT = 0b000011;
/** C.S0005-E Table 2.7.3-1: Parameter Update Confirmation Order. */
const ORDER_PARAMETER_UPDATE_CONFIRMATION = 0b000101;

export function accessProcedure(event: AccessEvent): string | null {
  if (event.isPreambleOnly) return null;
  if (event.trafficWalshCode != null) {
    if (event.authChallengeResponse) return "AUTHENTICATION_PROCEDURE_UNIQUE_CHALLENGE";
    if (event.order?.order === ORDER_SSD_UPDATE_RESULT) return "AUTHENTICATION_PROCEDURE_SSD_UPDATE";
    if (event.order?.order === ORDER_PARAMETER_UPDATE_CONFIRMATION) {
      return "AUTHENTICATION_PROCEDURE_COUNT_UPDATE";
    }
    return null;
  }
  if (event.registration) return "AUTHENTICATION_PROCEDURE_REGISTRATION";
  if (event.origination) return "AUTHENTICATION_PROCEDURE_ORIGINATION";
  if (event.pageResponse) return "AUTHENTICATION_PROCEDURE_PAGE_RESPONSE";
  if (event.dataBurst?.burstType === SMS_BURST_TYPE) return "AUTHENTICATION_PROCEDURE_DATA_BURST";
  return null;
}

const SMS_BURST_TYPE = 3;
/** How far before the access a verdict may be stamped, to absorb clock skew. */
const VERDICT_LEAD_US = 1_000_000;
/** How long after the access its verdict may arrive. Registration waits on the HLR. */
const VERDICT_LAG_US = 15_000_000;

/**
 * The MSC verdict for an access-log row. A1 names no air-interface message, so the
 * verdict is matched by mobile, procedure and the closest time after the access.
 */
export function verdictForAccess(
  event: AccessEvent,
  eventTimeUs: number,
  mobile: Pick<MobileDirectoryEntry, "imsi" | "esn"> | undefined,
  verdicts: AuthVerdict[],
): AuthVerdict | null {
  const procedure = accessProcedure(event);
  if (!procedure) return null;
  const esn = event.esn ?? mobile?.esn;
  let best: { verdict: AuthVerdict; delta: number } | null = null;
  for (const verdict of verdicts) {
    if (verdict.procedure !== procedure) continue;
    const sameMobile =
      (mobile?.imsi != null && verdict.imsi === mobile.imsi) ||
      (esn != null && verdict.esn === esn);
    if (!sameMobile) continue;
    const at = verdictTimeUs(verdict);
    if (at == null) continue;
    const delta = at - eventTimeUs;
    if (delta < -VERDICT_LEAD_US || delta > VERDICT_LAG_US) continue;
    if (!best || Math.abs(delta) < Math.abs(best.delta)) best = { verdict, delta };
  }
  return best?.verdict ?? null;
}
