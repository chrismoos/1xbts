import {
  PrlExtSysRecord,
  PrlSysRecord,
} from "@/lib/proto/hlr/v1/service";

export function systemReferenceLabel(
  record: PrlSysRecord | PrlExtSysRecord,
  index: number,
): string {
  if ("sid" in record) {
    return `SYS #${index} · SID ${record.sid || "any"}`;
  }
  if (record.cdma2000) {
    return `SYS #${index} · SID ${record.cdma2000.sid || "any"}`;
  }
  if (record.hrpd) return `SYS #${index} · HRPD subnet`;
  if (record.mccMnc) {
    const body =
      record.mccMnc.subtype000 ??
      record.mccMnc.subtype001 ??
      record.mccMnc.subtype010 ??
      record.mccMnc.subtype011;
    return body
      ? `SYS #${index} · MCC ${body.mcc} / MNC ${body.mnc}`
      : `SYS #${index} · MCC-MNC`;
  }
  return `SYS #${index}`;
}
