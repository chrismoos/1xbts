export type CallKind = "voice" | "data" | "sms" | "otasp" | "test";

// C.S0017 service option numbers.
const DATA_SOS = new Set([7, 12, 15, 22, 25, 33]);
const SMS_SOS = new Set([6, 14]);
const OTASP_SOS = new Set([18, 19]);
const TEST_SOS = new Set([2, 9, 54, 55, 32798, 32799]);

export function callKind(serviceOption?: number): CallKind {
  if (serviceOption == null) return "voice";
  if (DATA_SOS.has(serviceOption)) return "data";
  if (SMS_SOS.has(serviceOption)) return "sms";
  if (OTASP_SOS.has(serviceOption)) return "otasp";
  if (TEST_SOS.has(serviceOption)) return "test";
  return "voice";
}

export const CALL_KIND_LABEL: Record<CallKind, string> = {
  voice: "Voice",
  data: "Data",
  sms: "SMS",
  otasp: "OTASP",
  test: "Test",
};

export const CALL_KIND_BADGE: Record<CallKind, string> = {
  voice: "bg-surface-raised text-secondary",
  data: "bg-badge-blue-bg text-badge-blue-text",
  sms: "bg-badge-orange-bg text-badge-orange-text",
  otasp: "bg-badge-purple-bg text-badge-purple-text",
  test: "bg-surface-raised text-muted",
};

export function callStateLabel(state?: string): string | undefined {
  return state?.replace(/^Voice(?=[A-Z])/, "");
}
