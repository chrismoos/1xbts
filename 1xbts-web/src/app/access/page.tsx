"use client";

import { useCallback, useState } from "react";
import { Card } from "@/components/card";
import { shouldHideAccessEvent } from "@/components/message-detail";
import { MessageListHeader, MessageRow, messageView } from "@/components/message-row";
import { type LogEntry, makeLogEntryId } from "@/lib/message-log";
import type { AccessEvent } from "@/lib/proto/bsc/v1/service";
import { useBtsList } from "@/lib/use-bts-list";
import { useEventStream, type EventStreamOptions } from "@/lib/use-event-stream";

const REPLAY_EVENTS: EventStreamOptions = { replay: true };

const MAX_EVENTS = 200;

export default function AccessPage() {
  const [entries, setEntries] = useState<LogEntry[]>([]);
  const [expanded, setExpanded] = useState<Set<string>>(new Set());
  const { cells } = useBtsList();

  useEventStream(
    "access",
    useCallback((data: string) => {
      const event: AccessEvent = JSON.parse(data);
      if (shouldHideAccessEvent(event)) return;
      const ts = event.timestampUs ?? Date.now() * 1000;
      const id = makeLogEntryId("rx", ts);
      setEntries((prev) =>
        [{ kind: "rx" as const, id, ts, identity: id, sortKey: id, event, seenCount: 1 }, ...prev].slice(0, MAX_EVENTS),
      );
    }, []),
    REPLAY_EVENTS,
  );

  const toggleExpand = (id: string) => {
    setExpanded((prev) => {
      const next = new Set(prev);
      if (next.has(id)) next.delete(id);
      else next.add(id);
      return next;
    });
  };

  const ctx = { mobile: {}, verdicts: [], cells };

  return (
    <div className="max-w-7xl mx-auto space-y-6">
      <h1 className="text-lg font-bold">Access Channel Inspector</h1>

      <Card title={`Events (${entries.length})`}>
        {entries.length === 0 ? (
          <p className="text-dimmed text-sm">Waiting for access channel events...</p>
        ) : (
          <>
            <MessageListHeader subject="Mobile" />
            {entries.map((entry) => (
              <MessageRow
                key={entry.id}
                entry={entry}
                view={messageView(entry, ctx)}
                open={expanded.has(entry.id)}
                onToggle={() => toggleExpand(entry.id)}
                subject={
                  entry.kind === "rx" ? (
                    <span className="font-mono text-xs text-muted">
                      {entry.event.resolvedAddress || entry.event.address || "unknown"}
                    </span>
                  ) : null
                }
              />
            ))}
          </>
        )}
      </Card>
    </div>
  );
}
