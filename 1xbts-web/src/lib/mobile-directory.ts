import { useEffect, useState } from "react";

import type { AccessEvent, PagingEvent, TrafficEvent } from "@/lib/proto/bsc/v1/service";

/// Subset of the BSC's MobileInfo gRPC payload the message log needs to
/// resolve "which mobile sent / received this event" and render a link.
export interface MobileDirectoryEntry {
  address: string;
  esn?: number;
  imsi?: string;
  meid?: string;
  phoneNumber?: string;
  subscriberId?: string;
  trafficWalshCode?: number;
}

/// Polls `/api/mobiles` periodically and returns the latest snapshot.
/// Polling is shared per component instance; for many consumers consider
/// hoisting to a shared store later.
export function useMobileDirectory(intervalMs = 1500): MobileDirectoryEntry[] {
  const [mobiles, setMobiles] = useState<MobileDirectoryEntry[]>([]);

  useEffect(() => {
    let cancelled = false;
    const load = async () => {
      try {
        const res = await fetch("/api/mobiles");
        if (!res.ok) return;
        const data = await res.json();
        if (cancelled || !Array.isArray(data)) return;
        setMobiles(data as MobileDirectoryEntry[]);
      } catch {
        // ignore — next tick will retry
      }
    };
    void load();
    const id = setInterval(load, intervalMs);
    return () => {
      cancelled = true;
      clearInterval(id);
    };
  }, [intervalMs]);

  return mobiles;
}

/// Best-effort match of a Paging/Traffic/Access event to a known mobile.
/// Returns undefined when no entry in the directory is recognisably the
/// counterpart (e.g. event arrived before the mobile registered, or the
/// mobile has since been evicted).
export function mobileForEvent(
  event: AccessEvent | PagingEvent | TrafficEvent,
  mobiles: MobileDirectoryEntry[],
): MobileDirectoryEntry | undefined {
  if (mobiles.length === 0) return undefined;

  // AccessEvent (RX from mobile) carries the richest identification.
  if ("subscriberId" in event && event.subscriberId) {
    const m = mobiles.find((m) => m.subscriberId === event.subscriberId);
    if (m) return m;
  }
  if ("resolvedAddress" in event && event.resolvedAddress) {
    const m = mobiles.find((m) => m.address === event.resolvedAddress);
    if (m) return m;
  }
  if ("address" in event && typeof event.address === "string" && event.address) {
    const m = mobiles.find((m) => m.address === event.address);
    if (m) return m;
  }
  if ("esn" in event && typeof event.esn === "number") {
    const esn = event.esn;
    const m = mobiles.find((m) => m.esn === esn);
    if (m) return m;
  }
  if ("meid" in event && typeof event.meid === "string" && event.meid) {
    const meid = event.meid;
    const m = mobiles.find((m) => m.meid === meid);
    if (m) return m;
  }

  // PagingEvent uses header.resolvedAddress / header.address.{esn,imsiS,...}
  const header = "header" in event ? event.header : undefined;
  if (header?.resolvedAddress) {
    const m = mobiles.find((m) => m.address === header.resolvedAddress);
    if (m) return m;
  }
  if (header?.address?.esn != null) {
    const esn = header.address.esn;
    const m = mobiles.find((m) => m.esn === esn);
    if (m) return m;
  }

  // TrafficEvent (and some access/paging events) carries a walsh code.
  const walsh =
    ("trafficWalshCode" in event ? event.trafficWalshCode : undefined) ??
    ("walshCode" in event ? event.walshCode : undefined);
  if (typeof walsh === "number") {
    const m = mobiles.find((m) => m.trafficWalshCode === walsh);
    if (m) return m;
  }

  return undefined;
}

/// Identifier kind used to render the message-log MS column. The single
/// letters keep the column tight; the cell renders a tooltip with the full
/// "phone / IMSI / ESN" name.
export type MobileLabelKind = "P" | "I" | "E" | "M" | "?";

export interface MobileLabel {
  kind: MobileLabelKind;
  value: string;
  /// Long-form name for tooltip rendering ("phone", "IMSI", "ESN").
  full: string;
}

/// Annotated label for a mobile, in priority order:
///   * `P` — HLR-resolved subscriber phone number
///   * `I` — IMSI known but no subscriber
///   * `E` — ESN-only mobile
///   * `?` — raw address as last resort
export function mobileLabel(mobile: MobileDirectoryEntry): MobileLabel {
  if (mobile.phoneNumber) {
    return { kind: "P", value: mobile.phoneNumber, full: "phone" };
  }
  if (mobile.imsi) {
    return { kind: "I", value: mobile.imsi, full: "IMSI" };
  }
  if (mobile.esn != null) {
    return {
      kind: "E",
      value: `0x${(mobile.esn >>> 0).toString(16).toUpperCase().padStart(8, "0")}`,
      full: "ESN",
    };
  }
  if (mobile.meid) {
    return { kind: "M", value: mobile.meid.toUpperCase(), full: "MEID" };
  }
  return { kind: "?", value: mobile.address, full: "address" };
}
