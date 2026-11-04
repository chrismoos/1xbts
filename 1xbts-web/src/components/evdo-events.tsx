"use client";

import { Fragment } from "react";
import { HrpdAccessDetail, HrpdSessionDetail, HrpdTrafficDetail } from "@/components/message-detail";
import { evdoMessageInfo, type EvdoEvent, type EvdoEventStream } from "@/lib/evdo-messages";
import { formatTime } from "@/lib/message-log";
import type { HrpdAccessEvent, HrpdSessionEvent, HrpdTrafficEvent } from "@/lib/proto/events/v1/an";

export interface EvdoEventRow {
  id: string;
  ts: number;
  stream: EvdoEventStream;
  event: EvdoEvent;
}

const DIRECTION = {
  tx: { label: "↓ TX", title: "Forward link, sent to the access terminal", className: "text-accent-cyan bg-accent-blue-bg" },
  rx: { label: "↑ RX", title: "Reverse link, received from the access terminal", className: "text-accent-green bg-accent-green-bg" },
  none: { label: "AN", title: "Access network state change", className: "text-accent-purple bg-accent-purple-bg" },
} as const;

const CHANNEL_CLASS: Record<string, string> = {
  Access: "text-accent-amber",
  Traffic: "text-accent-cyan",
  Session: "text-accent-purple",
};

const COLUMN_COUNT = 8;

function EventDetail({ row }: { row: EvdoEventRow }) {
  if (row.stream === "session") return <HrpdSessionDetail event={row.event as HrpdSessionEvent} />;
  if (row.stream === "access") return <HrpdAccessDetail event={row.event as HrpdAccessEvent} />;
  return <HrpdTrafficDetail event={row.event as HrpdTrafficEvent} />;
}

export function EvdoEventTable({
  rows,
  expanded,
  onToggle,
}: {
  rows: EvdoEventRow[];
  expanded: Set<string>;
  onToggle: (id: string) => void;
}) {
  if (rows.length === 0) return <p className="text-sm text-dimmed">Waiting for events...</p>;
  return (
    <div className="overflow-x-auto">
      <table className="data-table">
        <thead>
          <tr>
            <th>Time</th>
            <th>Dir</th>
            <th>Channel</th>
            <th>Message</th>
            <th>Protocol</th>
            <th className="text-right">MAC</th>
            <th className="text-right">DRC</th>
            <th className="text-right">Bytes</th>
          </tr>
        </thead>
        <tbody>
          {rows.map((row) => {
            const info = evdoMessageInfo(row.stream, row.event);
            const direction = DIRECTION[info.direction];
            const open = expanded.has(row.id);
            return (
              <Fragment key={row.id}>
                <tr
                  className={`cursor-pointer align-top ${open ? "bg-surface-raised" : ""}`}
                  onClick={() => onToggle(row.id)}>
                  <td className="whitespace-nowrap font-mono text-[11px] text-muted">
                    {formatTime(row.ts).slice(11)}
                  </td>
                  <td>
                    <span
                      title={direction.title}
                      className={`inline-flex rounded-full px-2 py-0.5 text-[11px] font-medium whitespace-nowrap ${direction.className}`}
                    >
                      {direction.label}
                    </span>
                  </td>
                  <td className={`${CHANNEL_CLASS[info.channel] ?? "text-muted"}`}>{info.channel}</td>
                  <td>
                    <span className="font-medium text-primary">{info.name}</span>
                    {info.extraMessages > 0 && <span className="ml-1.5 text-[11px] text-dimmed">+{info.extraMessages}</span>}
                  </td>
                  <td className="text-muted">{info.protocol ?? "—"}</td>
                  <td className="text-right font-mono">{info.macIndex ?? "—"}</td>
                  <td className="text-right font-mono">{info.drc ?? "—"}</td>
                  <td className="text-right font-mono text-muted">{info.bytes ?? "—"}</td>
                </tr>
                {open && (
                  <tr>
                    <td colSpan={COLUMN_COUNT} className="bg-surface-raised px-3 pb-3 pt-1">
                      <EventDetail row={row} />
                    </td>
                  </tr>
                )}
              </Fragment>
            );
          })}
        </tbody>
      </table>
    </div>
  );
}
