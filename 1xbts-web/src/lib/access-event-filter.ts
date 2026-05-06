import type { AccessEvent } from "@/lib/proto/bsc/v1/service";

export function shouldHideAccessEvent(event: AccessEvent): boolean {
  return /^TrafficPcgMeasurement\(W\d+\)$/.test(event.msgTypeName)
    || event.pduSummary.startsWith("pcg_measurement ");
}
