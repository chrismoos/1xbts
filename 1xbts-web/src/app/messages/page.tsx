"use client";

import { useState, useCallback } from "react";
import Link from "next/link";
import { Card } from "@/components/card";
import {
  AccessDetail,
  PagingDetail,
  TrafficDetail,
  formatAccessSummary,
  formatAccessChannel,
  formatAccessTypeName,
  formatPagingChannel,
  formatPagingSummary,
  formatTrafficChannel,
  formatTrafficSummary,
  shouldHideAccessEvent,
} from "@/components/message-detail";
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
import { useEventStream } from "@/lib/use-event-stream";
import { type LogEntry, makeLogEntryId, makeSortKey, sortLogEntries, formatTime } from "@/lib/message-log";
import type { AccessEvent, PagingEvent, TrafficEvent } from "@/lib/proto/bsc/v1/service";

/// Renders the matched mobile (if any) for a message-log entry as a
/// linkable badge. Fixed-width column so log rows stay aligned. Single-letter
/// kind prefix (P/I/E) sits in a dimmed slot followed by the value; the
/// tooltip carries the long-form ("phone", "IMSI", "ESN") and the address.
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

// ─── Component ──────────────────────────────────────────────────

const MAX_ENTRIES = 500;

export default function MessagesPage() {
  const [entries, setEntries] = useState<LogEntry[]>([]);
  const [expanded, setExpanded] = useState<Set<string>>(new Set());
  const [filter, setFilter] = useState<"all" | "tx" | "rx">("all");
  const mobiles = useMobileDirectory();


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
  }, [addEntry]));

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
  }, [addEntry]));

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
  }, [addEntry]));



  const toggleExpand = (id: string) => {
    setExpanded((prev) => {
      const next = new Set(prev);
      if (next.has(id)) next.delete(id);
      else next.add(id);
      return next;
    });
  };

  const filtered = filter === "all" ? entries : entries.filter((e) => e.kind === filter);

  const txCount = entries.filter((e) => e.kind === "tx").length;
  const rxCount = entries.filter((e) => e.kind === "rx").length;

  return (
    <div className="max-w-7xl mx-auto space-y-4">
      <div className="flex items-center gap-4 flex-wrap">
        <h1 className="text-lg font-bold">Message Log</h1>
        <div className="flex gap-1 ml-auto">
          {(["all", "tx", "rx"] as const).map((f) => (
            <button
              key={f}
              onClick={() => setFilter(f)}
              className={`text-xs px-3 py-1 rounded transition-colors ${
                filter === f
                  ? "bg-surface-raised text-primary"
                  : "bg-surface-solid text-muted hover:text-primary"
              }`}
            >
              {f === "all" ? `All (${entries.length})` : f === "tx" ? `TX (${txCount})` : `RX (${rxCount})`}
            </button>
          ))}
        </div>
      </div>

      <Card title={`Messages (${filtered.length})`}>
        {filtered.length === 0 ? (
          <p className="text-dimmed text-sm">Waiting for messages...</p>
        ) : (
          <div className="space-y-0">
            {filtered.map((entry) => {
              const isExpanded = expanded.has(entry.id);
              if (entry.kind === "tx" && entry.stream === "paging") {
                const h = entry.event.header;
                const typeName = h?.msgTypeName ?? "Unknown";
                const detail = formatPagingSummary(entry.event);
                const channel = formatPagingChannel(entry.event);
                return (
                  <div
                    key={entry.id}
                    className="border-b border-border-subtle py-1.5 px-2 -mx-2 rounded"
                  >
                    <div
                      className="flex items-center gap-2 text-sm cursor-pointer hover:bg-hover rounded"
                      onClick={() => toggleExpand(entry.id)}
                    >
                      <div className="flex-1 min-w-0 flex items-center gap-2">
                        <span className="text-muted font-mono text-xs w-[15rem] shrink-0">{formatTime(entry.ts)}</span>
                        <MobileCell event={entry.event} mobiles={mobiles} />
                        <span className="text-accent-blue font-mono text-xs w-6 shrink-0">TX</span>
                        <span className="text-dimmed text-xs shrink-0">{channel}</span>
                        <span className="text-primary font-medium shrink-0">{typeName}</span>
                        {entry.seenCount > 1 && <span className="text-muted text-xs shrink-0">x{entry.seenCount}</span>}
                        {detail && <span className="text-muted text-xs truncate">{detail}</span>}
                      </div>
                      <div className="shrink-0 flex items-center gap-1">
                        <span className="text-accent-amber text-xs font-mono w-14 text-right">{h ? `SEQ=${h.msgSeq}` : ""}</span>
                        <span className="text-accent-green text-xs font-mono w-14 text-right">{h?.validAck ? `ACK=${h.ackSeq}` : ""}</span>
                        <span className="text-badge-orange-text text-xs w-4 text-center" title="ACK required">{h?.ackReq ? "\u21A9" : ""}</span>
                        <span className="text-dimmed text-xs">{isExpanded ? "▾" : "▸"}</span>
                      </div>
                    </div>
                    {isExpanded && (
                      <div className="mt-1.5 ml-8 pb-1">
                        {h && (
                          <div className="text-xs text-muted mb-1">
                            MSG_TAG: 0x{h.msgTag.toString(16).toUpperCase().padStart(2, "0")} | MSG_SEQ: {h.msgSeq} | ACK_SEQ: {h.ackSeq} | ACK_REQ: {h.ackReq ? "1" : "0"} | VALID_ACK: {h.validAck ? "1" : "0"}
                          </div>
                        )}
                        <PagingDetail event={entry.event} />
                      </div>
                    )}
                  </div>
                );
              } else if (entry.kind === "tx") {
                const h = entry.event.header;
                const typeName = h?.msgTypeName ?? "Unknown";
                const detail = formatTrafficSummary(entry.event);
                const channel = formatTrafficChannel(entry.event);
                return (
                  <div
                    key={entry.id}
                    className="border-b border-border-subtle py-1.5 px-2 -mx-2 rounded"
                  >
                    <div
                      className="flex items-center gap-2 text-sm cursor-pointer hover:bg-hover rounded"
                      onClick={() => toggleExpand(entry.id)}
                    >
                      <div className="flex-1 min-w-0 flex items-center gap-2">
                        <span className="text-muted font-mono text-xs w-[15rem] shrink-0">{formatTime(entry.ts)}</span>
                        <MobileCell event={entry.event} mobiles={mobiles} />
                        <span className="text-accent-cyan font-mono text-xs w-6 shrink-0">TX</span>
                        <span className="text-dimmed text-xs shrink-0">{channel}</span>
                        <span className="text-primary font-medium shrink-0">{typeName}</span>
                        {entry.seenCount > 1 && <span className="text-muted text-xs shrink-0">x{entry.seenCount}</span>}
                        {detail && <span className="text-muted text-xs truncate">{detail}</span>}
                      </div>
                      <div className="shrink-0 flex items-center gap-1">
                        <span className="text-accent-amber text-xs font-mono w-14 text-right">{h ? `SEQ=${h.msgSeq}` : ""}</span>
                        <span className="text-accent-green text-xs font-mono w-14 text-right">{h?.validAck ? `ACK=${h.ackSeq}` : ""}</span>
                        <span className="text-badge-orange-text text-xs w-4 text-center" title="ACK required">{h?.ackReq ? "\u21A9" : ""}</span>
                        <span className="text-dimmed text-xs">{isExpanded ? "▾" : "▸"}</span>
                      </div>
                    </div>
                    {isExpanded && (
                      <div className="mt-1.5 ml-8 pb-1">
                        {h && (
                          <div className="text-xs text-muted mb-1">
                            MSG_TAG: 0x{h.msgTag.toString(16).toUpperCase().padStart(2, "0")} | MSG_SEQ: {h.msgSeq} | ACK_SEQ: {h.ackSeq} | ACK_REQ: {h.ackReq ? "1" : "0"} | VALID_ACK: {h.validAck ? "1" : "0"}
                          </div>
                        )}
                        <TrafficDetail event={entry.event} />
                      </div>
                    )}
                  </div>
                );
              } else {
                const ev = entry.event;
                const channel = formatAccessChannel(ev);
                return (
                  <div
                    key={entry.id}
                    className="border-b border-border-subtle py-1.5 px-2 -mx-2 rounded"
                  >
                    <div
                      className="flex items-center gap-2 text-sm cursor-pointer hover:bg-hover rounded"
                      onClick={() => toggleExpand(entry.id)}
                    >
                      <div className="flex-1 min-w-0 flex items-center gap-2">
                        <span className="text-muted font-mono text-xs w-[15rem] shrink-0">{formatTime(entry.ts)}</span>
                        <MobileCell event={ev} mobiles={mobiles} />
                        <span className="text-accent-green font-mono text-xs w-6 shrink-0">RX</span>
                        <span className="text-dimmed text-xs shrink-0">{channel}</span>
                        <span className="text-primary font-medium shrink-0">{formatAccessTypeName(ev)}</span>
                        {entry.seenCount > 1 && <span className="text-muted text-xs shrink-0">x{entry.seenCount}</span>}
                        <span className="text-muted text-xs truncate">{formatAccessSummary(ev)}</span>
                      </div>
                      <div className="shrink-0 flex items-center gap-1">
                        <span className="text-accent-amber text-xs font-mono w-14 text-right">{ev.msgSeq != null ? `SEQ=${ev.msgSeq}` : ""}</span>
                        <span className="text-accent-green text-xs font-mono w-14 text-right">{ev.validAck && ev.ackSeq != null ? `ACK=${ev.ackSeq}` : ""}</span>
                        <span className="text-badge-orange-text text-xs w-4 text-center" title="ACK required">{ev.ackReq ? "\u21A9" : ""}</span>
                        <span className="text-dimmed text-xs">{isExpanded ? "▾" : "▸"}</span>
                      </div>
                    </div>
                    {isExpanded && (
                      <div className="mt-1.5 ml-8 pb-1">
                        <AccessDetail event={ev} />
                      </div>
                    )}
                  </div>
                );
              }
            })}
          </div>
        )}
      </Card>
    </div>
  );
}
