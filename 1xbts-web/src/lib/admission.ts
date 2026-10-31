export interface AdmissionVerdict {
  time?: string;
  procedure?: string;
  admissionClass?: string;
  reason?: string;
  service?: string;
  serviceRejected?: boolean;
  imsi?: string;
  esn?: number;
  meid?: string;
  baseStation?: string;
  subscriberId?: string;
  configuredImsi?: string;
}

export interface AccessPolicy {
  accessMode: string;
  registerPolicy: string;
  authenticationMode: string;
}

export interface ObservedMobile {
  imsi?: string;
  esn?: number;
  meid?: string;
  firstSeenAt?: string;
  lastSeenAt?: string;
  admissionClass?: string;
  admissionReason?: string;
  admissionAt?: string;
  admissionBaseStation?: string;
  deniedCount: number;
  lastDeniedAt?: string;
  lastDenialReason?: string;
  lastDeniedBaseStation?: string;
  subscriber?: {
    subscriberId: string;
    phoneNumber: string;
    displayName: string;
    status: string;
    configuredImsi?: string;
  };
}

export type AccessClass = "admitted" | "limited" | "provisioning" | "registration_only" | "denied";

const CLASS_FROM_PROTO: Record<string, AccessClass> = {
  ADMISSION_CLASS_ADMITTED: "admitted",
  ADMISSION_CLASS_LIMITED: "limited",
  ADMISSION_CLASS_PROVISIONING: "provisioning",
  ADMISSION_CLASS_REGISTRATION_ONLY: "registration_only",
  ADMISSION_CLASS_DENIED: "denied",
  MOBILE_ADMISSION_CLASS_ADMITTED: "admitted",
  MOBILE_ADMISSION_CLASS_LIMITED: "limited",
  MOBILE_ADMISSION_CLASS_PROVISIONING: "provisioning",
  MOBILE_ADMISSION_CLASS_REGISTRATION_ONLY: "registration_only",
  MOBILE_ADMISSION_CLASS_DENIED: "denied",
};

export function accessClass(value?: string): AccessClass | undefined {
  if (!value) return undefined;
  return CLASS_FROM_PROTO[value] ?? (value as AccessClass);
}

export const CLASS_LABELS: Record<AccessClass, string> = {
  admitted: "Admitted",
  limited: "Limited",
  provisioning: "Provisioning",
  registration_only: "Registration only",
  denied: "Denied",
};

export const CLASS_BADGE: Record<AccessClass, string> = {
  admitted: "bg-badge-green-bg text-badge-green-text",
  limited: "bg-badge-yellow-bg text-badge-yellow-text",
  provisioning: "bg-badge-blue-bg text-badge-blue-text",
  registration_only: "bg-badge-orange-bg text-badge-orange-text",
  denied: "bg-badge-red-bg text-badge-red-text",
};

export const CLASS_DETAIL: Record<AccessClass, string> = {
  admitted: "All service.",
  limited: "Registers and receives SMS. No calls, data or MO SMS.",
  provisioning: "Registers and may run OTASP only, until the handset reports the configured IMSI.",
  registration_only: "Registers only. No calls, data or SMS.",
  denied: "Registration and service are refused.",
};

const REASON_LABELS: Record<string, string> = {
  not_a_subscriber: "Not a subscriber",
  subscriber_suspended: "Subscriber suspended",
  subscriber_disabled: "Subscriber disabled",
  hardware_staged: "Hardware staged",
  hlr_error: "HLR unavailable",
  service_not_allowed: "Service not allowed",
  registration_policy_any: "Registered under policy any",
  identity_conflict: "Identity conflict",
  ambiguous_identity: "Ambiguous match",
  authentication_recovery_required: "A-key without SSD",
  authentication_required: "Authentication required",
  pseudo_esn: "Pseudo-ESN only",
};

export function reasonLabel(reason?: string): string {
  if (!reason) return "-";
  const key = reason.replace(/^ADMISSION_REASON_/, "").toLowerCase();
  return REASON_LABELS[key] ?? key.replaceAll("_", " ");
}

const SERVICE_LABELS: Record<string, string> = {
  ADMISSION_SERVICE_REGISTRATION: "registration",
  ADMISSION_SERVICE_VOICE: "voice",
  ADMISSION_SERVICE_DATA: "data",
  ADMISSION_SERVICE_SMS: "SMS",
  ADMISSION_SERVICE_OTASP: "OTASP",
  ADMISSION_SERVICE_PAGE_RESPONSE: "page response",
};

export function serviceLabel(service?: string): string {
  return (service && SERVICE_LABELS[service]) || "-";
}

export function procedureName(procedure?: string): string {
  if (!procedure) return "-";
  return procedure.replace(/^AUTHENTICATION_PROCEDURE_/, "").toLowerCase().replaceAll("_", " ");
}

export function createSubscriberHref(mobile: { imsi?: string; esn?: number; meid?: string }): string {
  const params = new URLSearchParams({ new: "1" });
  if (mobile.imsi) params.set("imsi", mobile.imsi);
  if (mobile.esn != null) params.set("esn", mobile.esn.toString(16).toUpperCase().padStart(8, "0"));
  if (mobile.meid) params.set("meid", mobile.meid.toUpperCase());
  return `/subscribers?${params}`;
}

const ESN_MANUFACTURER_SHIFT = 24;
const PSEUDO_ESN_MANUFACTURER = 0x80;

export function latestAdmissionForMobile(
  events: AdmissionVerdict[],
  mobile: { imsi?: string; esn?: number; meid?: string },
): AdmissionVerdict | undefined {
  const matching = events.filter((event) => {
    if (mobile.meid && event.meid) {
      return mobile.meid.toUpperCase() === event.meid.toUpperCase();
    }
    if (mobile.esn != null && event.esn != null) {
      if (mobile.esn !== event.esn) return false;
      if ((mobile.esn >>> ESN_MANUFACTURER_SHIFT) !== PSEUDO_ESN_MANUFACTURER) return true;
      return !!mobile.imsi && event.imsi === mobile.imsi;
    }
    return !mobile.meid && mobile.esn == null && !!mobile.imsi && event.imsi === mobile.imsi;
  });
  return matching.reduce<AdmissionVerdict | undefined>((latest, event) => {
    if (!latest) return event;
    return (Date.parse(event.time ?? "") || 0) > (Date.parse(latest.time ?? "") || 0) ? event : latest;
  }, undefined);
}
