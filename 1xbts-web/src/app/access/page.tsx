"use client";

import { useEffect, useState } from "react";
import { Card } from "@/components/card";
import {
  AccessDetail,
  formatAccessSummary,
  formatAccessTypeName,
  shouldHideAccessEvent,
} from "@/components/message-detail";
import type { AccessEvent } from "@/lib/proto/bsc/v1/service";

export default function AccessPage() {
  const [events, setEvents] = useState<AccessEvent[]>([]);
  const [connected, setConnected] = useState(false);
  const [expanded, setExpanded] = useState<Set<number>>(new Set());

  useEffect(() => {
    const es = new EventSource("/api/access-events");
    es.onopen = () => setConnected(true);
    es.onmessage = (e) => {
      const event: AccessEvent = JSON.parse(e.data);
      if (shouldHideAccessEvent(event)) return;
      setEvents((prev) => [event, ...prev].slice(0, 200));
    };
    es.onerror = () => setConnected(false);
    return () => es.close();
  }, []);

  const toggleExpand = (idx: number) => {
    setExpanded((prev) => {
      const next = new Set(prev);
      if (next.has(idx)) next.delete(idx);
      else next.add(idx);
      return next;
    });
  };

  return (
    <div className="max-w-7xl mx-auto space-y-6">
      <h1 className="text-lg font-bold">Access Channel Inspector</h1>

      <Card title={`Events (${events.length})`}>
        {events.length === 0 ? (
          <p className="text-dimmed text-sm">
            Waiting for access channel events...
          </p>
        ) : (
          <div className="space-y-0">
            {events.map((event, i) => {
              const isExpanded = expanded.has(i);
              return (
                <div
                  key={i}
                  className="border-b border-border-subtle py-1.5 cursor-pointer hover:bg-hover px-2 -mx-2 rounded"
                  onClick={() => toggleExpand(i)}
                >
                  <div className="flex items-center gap-2 text-sm">
                    <span className="text-accent-green font-mono text-xs w-10 shrink-0">
                      RX
                    </span>
                    <span className="text-primary font-medium">
                      {formatAccessTypeName(event)}
                    </span>
                    <span className="text-muted text-xs">
                      {event.resolvedAddress || event.address || "unknown"}
                    </span>
                    <span className="text-muted text-xs truncate max-w-md">
                      {formatAccessSummary(event)}
                    </span>
                    {event.msgSeq != null && (
                      <span className="text-accent-amber text-xs">
                        SEQ={event.msgSeq}
                      </span>
                    )}
                    {event.mobPRev != null && (
                      <span className="text-dimmed text-xs">
                        P_REV={event.mobPRev}
                      </span>
                    )}
                    <span className="text-dimmed text-xs ml-auto">
                      {isExpanded ? "▾" : "▸"}
                    </span>
                  </div>
                  {isExpanded && (
                    <div className="mt-1.5 ml-12 pb-1">
                      <AccessDetail event={event} />
                    </div>
                  )}
                </div>
              );
            })}
          </div>
        )}
      </Card>
    </div>
  );
}
