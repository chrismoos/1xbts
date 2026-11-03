"use client";

import { useState, useCallback } from "react";
import Link from "next/link";
import { Card } from "@/components/card";
import { MessageListHeader, MessageRow, messageView } from "@/components/message-row";
import {
  hrpdDirectionLabel,
  shouldHideAccessEvent,
} from "@/components/message-detail";
import {
  a1TimestampUs,
  type A1LogEvent,
} from "@/lib/a1-event";
import {
  fingerprintAccessEvent,
  fingerprintPagingEvent,
  fingerprintTrafficEvent,
} from "@/lib/event-fingerprint";
import {
  mobileForEvent,
  mobileLabel,
  useMobileDirectory,
  type MobileDirectoryEntry,
} from "@/lib/mobile-directory";
import {
  formatHrpdFullUati,
  hrpdTimestampNsToUs,
  isHrpdTelemetryTrafficEvent,
  uatiHex,
  uatiHexDigits,
} from "@/lib/hrpd-correlation";
import { type AuthVerdict } from "@/lib/authentication-verdict";
import { useBtsList } from "@/lib/use-bts-list";
import { useEventStream, type EventStreamOptions } from "@/lib/use-event-stream";
import { type LogEntry, makeLogEntryId, makeSortKey, sortLogEntries } from "@/lib/message-log";
import type { AccessEvent, PagingEvent, TrafficEvent } from "@/lib/proto/bsc/v1/service";
import {
  HrpdAccessEvent,
  HrpdSessionEvent,
  HrpdTrafficEvent,
} from "@/lib/proto/events/v1/an";

const REPLAY_EVENTS: EventStreamOptions = { replay: true };

/** MSC authentication verdicts kept for matching against access rows. */
const MAX_VERDICTS = 500;

// The matched mobile for a message-log entry, as a fixed-width linkable badge.
// A single-letter kind prefix (P/I/E) precedes the value, with the long form in the tooltip.
function MobileCell({
  event,
  mobiles,
}: {
  event: AccessEvent | PagingEvent | TrafficEvent;
  mobiles: MobileDirectoryEntry[];
}) {
  const ms = mobileForEvent(event, mobiles);
  if (!ms) {
    return (
      <span className="font-mono text-xs w-48 shrink-0 text-dimmed flex items-center gap-1.5">
        <span className="w-3 text-center">·</span>
        <span className="truncate">—</span>
      </span>
    );
  }
  const label = mobileLabel(ms);
  return (
    <Link
      href={`/mobiles/${encodeURIComponent(ms.address)}`}
      className="font-mono text-xs w-48 shrink-0 flex items-center gap-1.5 text-accent-blue/90 hover:text-accent-blue hover:underline"
      title={`${label.full} ${label.value} — ${ms.address}`}
      onClick={(e) => e.stopPropagation()}
    >
      <span className="text-dimmed w-3 text-center">{label.kind}</span>
      <span className="truncate">{label.value}</span>
    </Link>
  );
}

function HrpdMobileCell({
  event,
  mobiles,
}: {
  event: HrpdSessionEvent | HrpdAccessEvent | HrpdTrafficEvent;
  mobiles: MobileDirectoryEntry[];
}) {
  const fullUati = formatHrpdFullUati(event.fullUati);
  const sessionKey = event.fullUati?.compactUati32 || event.uati;
  if (sessionKey) {
    const uati = sessionKey >>> 0;
    return (
      <Link
        href={`/hrpd/${uatiHexDigits(uati)}`}
        className="font-mono text-xs w-48 shrink-0 flex items-center gap-1.5 text-accent-purple/90 hover:text-accent-purple hover:underline"
        title={`HRPD session ${fullUati ?? uatiHex(uati)}`}
        onClick={(e) => e.stopPropagation()}
      >
        <span className="text-dimmed w-3 text-center">H</span>
        <span className="truncate">{fullUati ?? uatiHex(uati)}</span>
      </Link>
    );
  }
  const enriched = event as typeof event & {
    identity?: { imsi?: string; esn?: number; meid?: string };
    subscriber?: { subscriberId?: string };
  };
  const ms = mobiles.find((mobile) => {
    if (mobile.subscriberId && enriched.subscriber?.subscriberId === mobile.subscriberId) return true;
    if (mobile.imsi && enriched.identity?.imsi === mobile.imsi) return true;
    if (mobile.esn != null && enriched.identity?.esn === mobile.esn) return true;
    if (mobile.meid && enriched.identity?.meid?.toLowerCase() === mobile.meid.toLowerCase()) return true;
    return false;
  });
  if (!ms) {
    return (
      <span className="font-mono text-xs w-48 shrink-0 text-dimmed flex items-center gap-1.5">
        <span className="w-3 text-center">·</span>
        <span className="truncate">—</span>
      </span>
    );
  }
  const label = mobileLabel(ms);
  return (
    <Link
      href={`/mobiles/${encodeURIComponent(ms.address)}`}
      className="font-mono text-xs w-48 shrink-0 flex items-center gap-1.5 text-accent-blue/90 hover:text-accent-blue hover:underline"
      title={`${label.full} ${label.value} — ${ms.address}`}
      onClick={(e) => e.stopPropagation()}
    >
      <span className="text-dimmed w-3 text-center">{label.kind}</span>
      <span className="truncate">{label.value}</span>
    </Link>
  );
}

// A1 isn't an air-interface event, so match on the bus-resolved identity and
// the message's Mobile Identity IEs rather than the access-event matcher.
function A1MobileCell({
  event,
  mobiles,
}: {
  event: A1LogEvent;
  mobiles: MobileDirectoryEntry[];
}) {
  const imsi = event.identity?.imsi || event.mobileIdentity?.imsi;
  const esn = event.identity?.esn || event.mobileIdentity?.esn;
  const meid = event.identity?.meid || event.mobileIdentity?.meid;
  const ms = mobiles.find((mobile) => {
    if (mobile.subscriberId && event.subscriber?.subscriberId === mobile.subscriberId) return true;
    if (mobile.imsi && imsi === mobile.imsi) return true;
    if (mobile.esn != null && esn === mobile.esn) return true;
    if (mobile.meid && meid?.toLowerCase() === mobile.meid.toLowerCase()) return true;
    return false;
  });
  if (!ms) {
    // No directory entry yet — show the message's own identity (often the only
    // clue on a first registration).
    const fallback = imsi || (esn ? `0x${esn.toString(16).toUpperCase()}` : meid);
    return (
      <span className="font-mono text-xs w-48 shrink-0 text-dimmed flex items-center gap-1.5">
        <span className="w-3 text-center">{fallback ? "I" : "·"}</span>
        <span className="truncate">{fallback || "—"}</span>
      </span>
    );
  }
  const label = mobileLabel(ms);
  return (
    <Link
      href={`/mobiles/${encodeURIComponent(ms.address)}`}
      className="font-mono text-xs w-48 shrink-0 flex items-center gap-1.5 text-accent-blue/90 hover:text-accent-blue hover:underline"
      title={`${label.full} ${label.value} — ${ms.address}`}
      onClick={(e) => e.stopPropagation()}
    >
      <span className="text-dimmed w-3 text-center">{label.kind}</span>
      <span className="truncate">{label.value}</span>
    </Link>
  );
}

// ─── Component ──────────────────────────────────────────────────

const MAX_ENTRIES = 500;
type MessageFilter = "all" | "1x-tx" | "1x-rx" | "evdo-tx" | "evdo-rx" | "a1";

function hrpdTimestampUs(event: HrpdSessionEvent | HrpdAccessEvent | HrpdTrafficEvent): number {
  return hrpdTimestampNsToUs(event.timestampNs);
}

function hrpdFilterKey(entry: LogEntry): MessageFilter | "evdo" {
  if (entry.kind === "tx") return "1x-tx";
  if (entry.kind === "rx") return "1x-rx";
  if (entry.kind === "a1") return "a1";
  if (entry.kind === "hrpd") {
    const direction = "direction" in entry.event ? hrpdDirectionLabel(entry.event.direction) : "EVDO";
    if (direction === "EVDO TX") return "evdo-tx";
    if (direction === "EVDO RX") return "evdo-rx";
    return "evdo";
  }
  return "all";
}

export default function MessagesPage() {
  const [entries, setEntries] = useState<LogEntry[]>([]);
  const [expanded, setExpanded] = useState<Set<string>>(new Set());
  const [filter, setFilter] = useState<MessageFilter>("all");
  const mobiles = useMobileDirectory();
  const [verdicts, setVerdicts] = useState<AuthVerdict[]>([]);
  useEventStream("authentication", useCallback((data: string) => {
    const verdict: AuthVerdict = JSON.parse(data);
    setVerdicts((prev) => [verdict, ...prev].slice(0, MAX_VERDICTS));
  }, []), REPLAY_EVENTS);
  const { cells } = useBtsList();


  const addEntry = useCallback((entry: LogEntry) => {
    setEntries((prev) => {
      const existingIndex = prev.findIndex((candidate) => candidate.identity === entry.identity);
      if (existingIndex >= 0) {
        const existing = prev[existingIndex];
        const merged = { ...entry, id: existing.id, seenCount: existing.seenCount + 1 };
        return sortLogEntries([merged, ...prev.slice(0, existingIndex), ...prev.slice(existingIndex + 1)], MAX_ENTRIES);
      }
      return sortLogEntries([{ ...entry, seenCount: 1 }, ...prev], MAX_ENTRIES);
    });
  }, []);

  // Shared SSE via BroadcastChannel (only one tab holds the connection)
  useEventStream("paging", useCallback((data: string) => {
    const event: PagingEvent = JSON.parse(data);
    if (!("error" in event)) {
      const ts = event.timestampUs ?? (Date.now() * 1000);
      addEntry({
        kind: "tx",
        stream: "paging",
        id: makeLogEntryId("tx", ts),
        ts,
        identity: `tx:paging:${event.eventId || fingerprintPagingEvent(event)}`,
        sortKey: makeSortKey(event.eventId, `tx:paging:${event.eventId || fingerprintPagingEvent(event)}`),
        event,
        seenCount: 1,
      });
    }
  }, [addEntry]), REPLAY_EVENTS);

  useEventStream("traffic", useCallback((data: string) => {
    const event: TrafficEvent = JSON.parse(data);
    if (!("error" in event)) {
      const ts = event.timestampUs ?? (Date.now() * 1000);
      addEntry({
        kind: "tx",
        stream: "traffic",
        id: makeLogEntryId("tx", ts),
        ts,
        identity: `tx:traffic:${event.eventId || fingerprintTrafficEvent(event)}`,
        sortKey: makeSortKey(event.eventId, `tx:traffic:${event.eventId || fingerprintTrafficEvent(event)}`),
        event,
        seenCount: 1,
      });
    }
  }, [addEntry]), REPLAY_EVENTS);

  useEventStream("access", useCallback((data: string) => {
    const event: AccessEvent = JSON.parse(data);
    if (!("error" in event) && !shouldHideAccessEvent(event)) {
      const ts = event.timestampUs ?? (Date.now() * 1000);
      addEntry({
        kind: "rx",
        id: makeLogEntryId("rx", ts),
        ts,
        identity: `rx:access:${event.eventId || fingerprintAccessEvent(event)}`,
        sortKey: makeSortKey(event.eventId, `rx:access:${event.eventId || fingerprintAccessEvent(event)}`),
        event,
        seenCount: 1,
      });
    }
  }, [addEntry]), REPLAY_EVENTS);

  // HRPD/EV-DO events arrive from the aggregated event bus. They carry no
  // wire timestamp, so stamp them with receive time.
  useEventStream("hrpd-session", useCallback((data: string) => {
    const event = HrpdSessionEvent.fromJSON(JSON.parse(data));
    const ts = hrpdTimestampUs(event);
    const sequence = (event as HrpdSessionEvent & { sequence?: string | number }).sequence ?? ts;
    const identity = `hrpd:session:${event.uati}:${event.reason}:${sequence}`;
    addEntry({
      kind: "hrpd", stream: "session",
      id: makeLogEntryId("hrpd", ts), ts,
      identity, sortKey: identity, event, seenCount: 1,
    });
  }, [addEntry]), REPLAY_EVENTS);

  useEventStream("hrpd-access", useCallback((data: string) => {
    const event = HrpdAccessEvent.fromJSON(JSON.parse(data));
    const ts = hrpdTimestampUs(event);
    const sequence = (event as HrpdAccessEvent & { sequence?: string | number }).sequence ?? ts;
    const identity = `hrpd:access:${event.uati}:${event.accessSignature}:${event.reason}:${sequence}`;
    addEntry({
      kind: "hrpd", stream: "access",
      id: makeLogEntryId("hrpd", ts), ts,
      identity, sortKey: identity, event, seenCount: 1,
    });
  }, [addEntry]), REPLAY_EVENTS);

  useEventStream("hrpd-traffic", useCallback((data: string) => {
    const event = HrpdTrafficEvent.fromJSON(JSON.parse(data));
    if (isHrpdTelemetryTrafficEvent(event)) return;
    const ts = hrpdTimestampUs(event);
    const sequence = (event as HrpdTrafficEvent & { sequence?: string | number }).sequence ?? ts;
    const identity = `hrpd:traffic:${event.uati}:${event.reason}:${event.macIndex}:${event.direction}:${sequence}`;
    addEntry({
      kind: "hrpd", stream: "traffic",
      id: makeLogEntryId("hrpd", ts), ts,
      identity, sortKey: identity, event, seenCount: 1,
    });
  }, [addEntry]), REPLAY_EVENTS);



  useEventStream("a1", useCallback((data: string) => {
    const event: A1LogEvent = JSON.parse(data);
    const ts = a1TimestampUs(event);
    const identity = `a1:${event.eventId || `${event.messageTypeValue}:${event.sduHex}:${ts}`}`;
    addEntry({
      kind: "a1",
      id: makeLogEntryId("a1", ts),
      ts,
      identity,
      sortKey: makeSortKey(event.eventId, identity),
      event,
      seenCount: 1,
    });
  }, [addEntry]), REPLAY_EVENTS);

  const toggleExpand = (id: string) => {
    setExpanded((prev) => {
      const next = new Set(prev);
      if (next.has(id)) next.delete(id);
      else next.add(id);
      return next;
    });
  };

  const messageCtx = {
    mobile: {},
    verdicts,
    cells,
    resolveMobile: (event: AccessEvent) => mobileForEvent(event, mobiles),
  };

  const filtered = filter === "all" ? entries : entries.filter((e) => hrpdFilterKey(e) === filter);

  const oneXTxCount = entries.filter((e) => hrpdFilterKey(e) === "1x-tx").length;
  const oneXRxCount = entries.filter((e) => hrpdFilterKey(e) === "1x-rx").length;
  const evdoTxCount = entries.filter((e) => hrpdFilterKey(e) === "evdo-tx").length;
  const evdoRxCount = entries.filter((e) => hrpdFilterKey(e) === "evdo-rx").length;
  const a1Count = entries.filter((e) => hrpdFilterKey(e) === "a1").length;

  return (
    <div className="max-w-7xl mx-auto space-y-4">
      <div className="flex items-center gap-4 flex-wrap">
        <h1 className="text-lg font-bold">Message Log</h1>
        <div className="flex gap-1 ml-auto">
          {(["all", "1x-tx", "1x-rx", "evdo-tx", "evdo-rx", "a1"] as const).map((f) => (
            <button
              key={f}
              onClick={() => setFilter(f)}
              className={`text-xs px-3 py-1 rounded transition-colors ${
                filter === f
                  ? "bg-surface-raised text-primary"
                  : "bg-surface-solid text-muted hover:text-primary"
              }`}
            >
              {f === "all"
                ? `All (${entries.length})`
                : f === "1x-tx"
                  ? `1x TX (${oneXTxCount})`
                  : f === "1x-rx"
                    ? `1x RX (${oneXRxCount})`
                    : f === "evdo-tx"
                      ? `EVDO TX (${evdoTxCount})`
                      : f === "evdo-rx"
                        ? `EVDO RX (${evdoRxCount})`
                        : `A1 (${a1Count})`}
            </button>
          ))}
        </div>
      </div>

      <Card title={`Messages (${filtered.length})`}>
        {filtered.length === 0 ? (
          <p className="text-dimmed text-sm">Waiting for messages...</p>
        ) : (
          <>
            <MessageListHeader subject="Mobile" />
            {filtered.map((entry) => (
              <MessageRow
                key={entry.id}
                entry={entry}
                view={messageView(entry, messageCtx)}
                open={expanded.has(entry.id)}
                onToggle={() => toggleExpand(entry.id)}
                subject={
                  entry.kind === "hrpd" ? (
                    <HrpdMobileCell event={entry.event} mobiles={mobiles} />
                  ) : entry.kind === "a1" ? (
                    <A1MobileCell event={entry.event} mobiles={mobiles} />
                  ) : (
                    <MobileCell event={entry.event} mobiles={mobiles} />
                  )
                }
              />
            ))}
          </>
        )}
      </Card>
    </div>
  );
}
