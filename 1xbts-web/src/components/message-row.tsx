"use client";

import type { ReactNode } from "react";
import { AuthenticationMark } from "@/components/authentication-activity";
import {
  A1Detail,
  DetailSection,
  Field,
  AccessDetail,
  HrpdAccessDetail,
  HrpdSessionDetail,
  HrpdTrafficDetail,
  PagingDetail,
  TrafficDetail,
  formatAccessChannel,
  formatAccessSummary,
  formatPagingChannel,
  formatPagingSummary,
  formatTrafficChannel,
  formatTrafficSummary,
  hrpdDirectionClass,
  hrpdDirectionLabel,
} from "@/components/message-detail";
import { VoicePrivacyIndicator } from "@/components/voice-privacy-indicator";
import { cellLabel } from "@/lib/cell";
import { a1DirectionClass, a1DirectionLabel, a1TypeName, formatA1Summary } from "@/lib/a1-event";
import { type AuthVerdict, verdictForAccess } from "@/lib/authentication-verdict";
import { evdoMessageInfo } from "@/lib/evdo-messages";
import type { LogEntry } from "@/lib/message-log";
import type { AccessEvent } from "@/lib/proto/bsc/v1/service";

export type MessageCategory = "paging" | "access" | "traffic" | "hrpd" | "a1";

export const MESSAGE_CATEGORY_LABEL: Record<MessageCategory, string> = {
  paging: "Paging",
  access: "Access",
  traffic: "Traffic",
  hrpd: "EV-DO",
  a1: "A1",
};

export interface MessageView {
  category: MessageCategory;
  lane: "L3" | "A1";
  direction: string;
  directionClass: string;
  channel: string;
  type: string;
  summary: string;
  seq?: number;
  ack?: number;
  ackReq?: boolean;
  bytes?: number;
  cell?: string;
  mark?: ReactNode;
  detail: ReactNode;
}

export interface MessageContext {
  mobile: { imsi?: string; esn?: number };
  resolveMobile?: (event: AccessEvent) => { imsi?: string; esn?: number } | undefined;
  verdicts: AuthVerdict[];
  cells: Parameters<typeof A1Detail>[0]["cells"];
}

function PagingHeader({ h }: { h: { msgTag: number; msgSeq: number; ackSeq: number; ackReq: boolean; validAck: boolean } }) {
  return (
    <DetailSection title="Header" compact>
      <Field label="MSG_TAG" value={`0x${h.msgTag.toString(16).toUpperCase().padStart(2, "0")}`} />
      <Field label="MSG_SEQ" value={h.msgSeq} />
      <Field label="ACK_SEQ" value={h.ackSeq} />
      <Field label="ACK_REQ" value={h.ackReq ? "1" : "0"} />
      <Field label="VALID_ACK" value={h.validAck ? "1" : "0"} />
    </DetailSection>
  );
}

export function messageView(entry: LogEntry, ctx: MessageContext): MessageView {
  if (entry.kind === "tx" && entry.stream === "paging") {
    const h = entry.event.header;
    return {
      category: "paging",
      lane: "L3",
      direction: "1x TX",
      directionClass: "text-accent-blue",
      channel: formatPagingChannel(entry.event),
      cell: entry.event.cell ? cellLabel(entry.event.cell) : undefined,
      type: h?.msgTypeName ?? "Unknown",
      summary: formatPagingSummary(entry.event),
      seq: h?.msgSeq,
      ack: h?.validAck ? h.ackSeq : undefined,
      ackReq: h?.ackReq,
      detail: (
        <div className="space-y-3 text-xs">
          {h && <PagingHeader h={h} />}
          <PagingDetail event={entry.event} />
        </div>
      ),
    };
  }
  if (entry.kind === "tx") {
    const h = entry.event.header;
    return {
      category: "traffic",
      lane: "L3",
      direction: "1x TX",
      directionClass: "text-accent-cyan",
      channel: formatTrafficChannel(entry.event),
      cell: entry.event.cell ? cellLabel(entry.event.cell) : undefined,
      type: h?.msgTypeName ?? "Unknown",
      summary: formatTrafficSummary(entry.event),
      seq: h?.msgSeq,
      ack: h?.validAck ? h.ackSeq : undefined,
      ackReq: h?.ackReq,
      detail: <TrafficDetail event={entry.event} />,
    };
  }
  if (entry.kind === "hrpd") {
    const direction = "direction" in entry.event ? entry.event.direction : undefined;
    const info = evdoMessageInfo(entry.stream, entry.event);
    const fields = info.fields.map((field) => `${field.key} ${field.value}`);
    const base = {
      category: "hrpd" as const,
      lane: "L3" as const,
      direction: direction != null ? hrpdDirectionLabel(direction) : "EV-DO",
      directionClass: direction != null ? hrpdDirectionClass(direction) : hrpdDirectionClass(),
      channel: info.channel === "Session" ? "" : info.channel,
      type: info.name,
      summary: [...fields, info.note].filter(Boolean).join(" · "),
      bytes: info.bytes,
    };
    if (entry.stream === "session") return { ...base, detail: <HrpdSessionDetail event={entry.event} /> };
    if (entry.stream === "access") return { ...base, detail: <HrpdAccessDetail event={entry.event} /> };
    return { ...base, detail: <HrpdTrafficDetail event={entry.event} /> };
  }
  if (entry.kind === "a1") {
    const ev = entry.event;
    return {
      category: "a1",
      lane: "A1",
      direction: a1DirectionLabel(ev.direction),
      directionClass: a1DirectionClass(ev.direction),
      channel: "",
      type: a1TypeName(ev),
      summary: formatA1Summary(ev),
      bytes: ev.sduLengthBytes || undefined,
      cell: ev.cell ? cellLabel(ev.cell) : undefined,
      mark: <VoicePrivacyIndicator status={ev.voicePrivacy} />,
      detail: <A1Detail event={ev} cells={ctx.cells} />,
    };
  }
  const ev = entry.event;
  const verdict = verdictForAccess(ev, entry.ts, ctx.resolveMobile?.(ev) ?? ctx.mobile, ctx.verdicts);
  return {
    category: ev.trafficWalshCode != null ? "traffic" : "access",
    lane: "L3",
    direction: "1x RX",
    directionClass: "text-accent-green",
    channel: formatAccessChannel(ev),
    cell: ev.cell ? cellLabel(ev.cell) : undefined,
    type: ev.msgTypeName,
    summary: formatAccessSummary(ev),
    seq: ev.msgSeq ?? undefined,
    ack: ev.validAck && ev.ackSeq != null ? ev.ackSeq : undefined,
    ackReq: ev.ackReq,
    mark: <AuthenticationMark verdict={verdict} />,
    detail: <AccessDetail event={ev} verdict={verdict} />,
  };
}

export function shortTime(tsUs: number): string {
  const d = new Date(Math.floor(tsUs / 1000));
  return `${d.toLocaleTimeString([], { hour12: false })}.${String(d.getMilliseconds()).padStart(3, "0")}`;
}

type Flow = "down" | "up" | "unknown";

function flowOf(direction: string): Flow {
  if (/\b(TX|DL)\b/.test(direction)) return "down";
  if (/\b(RX|UL)\b/.test(direction)) return "up";
  return "unknown";
}

const FLOW_STYLE: Record<Flow, string> = {
  down: "bg-accent-blue-bg text-accent-blue",
  up: "bg-accent-green-bg text-accent-green",
  unknown: "bg-surface-raised text-muted",
};

const FLOW_GLYPH: Record<Flow, string> = { down: "↓", up: "↑", unknown: "•" };

function Chip({ children, className = "" }: { children: ReactNode; className?: string }) {
  return (
    <span className={`rounded-md bg-surface-raised px-1.5 py-0.5 font-mono text-[10px] leading-none ${className}`}>
      {children}
    </span>
  );
}

function FlowIcon({ view }: { view: MessageView }) {
  const flow = flowOf(view.direction);
  return (
    <span
      className={`grid h-6 w-6 shrink-0 place-items-center rounded-full text-xs font-semibold ${
        view.lane === "A1" ? "bg-badge-purple-bg text-badge-purple-text" : FLOW_STYLE[flow]
      }`}
      title={view.direction}
    >
      {view.lane === "A1" ? <span className="text-[9px]">A1</span> : FLOW_GLYPH[flow]}
    </span>
  );
}

function AckReqIcon({ active }: { active?: boolean }) {
  return (
    <span
      className={`grid h-4 w-4 shrink-0 place-items-center rounded-md font-mono text-[10px] leading-none ${
        active ? "bg-surface-raised text-badge-orange-text" : "invisible"
      }`}
      title={active ? "Acknowledgment requested" : undefined}
      aria-label={active ? "Acknowledgment requested" : undefined}
      aria-hidden={active ? undefined : true}
    >
      ↩
    </span>
  );
}

// Fixed slots, so a missing chip leaves a gap instead of moving its neighbours.
function SeqSlots({ view, entry }: { view: MessageView; entry: LogEntry }) {
  return (
    <span className="grid grid-cols-[2.25rem_3.5rem_3.5rem_1rem] items-center justify-items-end gap-1">
      <span>{entry.seenCount > 1 && <Chip className="text-muted">×{entry.seenCount}</Chip>}</span>
      <span>{view.seq != null && <Chip className="text-accent-amber">SEQ {view.seq}</Chip>}</span>
      <span>
        {view.ack != null ? (
          <Chip className="text-accent-green">ACK {view.ack}</Chip>
        ) : (
          view.bytes != null && <Chip className="text-muted">{view.bytes} B</Chip>
        )}
      </span>
      <AckReqIcon active={view.ackReq} />
    </span>
  );
}

function SeqChips({ view, entry }: { view: MessageView; entry: LogEntry }) {
  return (
    <>
      {entry.seenCount > 1 && <Chip className="text-muted">×{entry.seenCount}</Chip>}
      {view.seq != null && <Chip className="text-accent-amber">SEQ {view.seq}</Chip>}
      {view.ack != null && <Chip className="text-accent-green">ACK {view.ack}</Chip>}
      {view.ackReq && <Chip className="text-badge-orange-text">ACK req</Chip>}
      {view.bytes != null && <Chip className="text-muted">{view.bytes} B</Chip>}
    </>
  );
}

const GRID = "md:grid-cols-[6.5rem_1.5rem_5rem_minmax(0,1fr)_12rem]";
const GRID_WITH_SUBJECT = "md:grid-cols-[6.5rem_1.5rem_11rem_5rem_minmax(0,1fr)_12rem]";

export function MessageListHeader({ subject }: { subject?: string }) {
  return (
    <div
      className={`hidden md:grid ${subject ? GRID_WITH_SUBJECT : GRID} gap-3 px-3 pb-1.5 text-[10px] uppercase tracking-wide text-dimmed`}
    >
      <span>Time</span>
      <span />
      {subject && <span>{subject}</span>}
      <span>Channel</span>
      <span>Message</span>
      <span className="text-right">Sequence</span>
    </div>
  );
}

export function MessageRow({
  entry,
  view,
  open,
  onToggle,
  actions,
  subject,
}: {
  entry: LogEntry;
  view: MessageView;
  open: boolean;
  onToggle?: () => void;
  actions?: ReactNode;
  subject?: ReactNode;
}) {
  return (
    <div data-entry-id={entry.id} className={open ? "my-1.5" : ""}>
      <div
        role={onToggle ? "button" : undefined}
        tabIndex={onToggle ? 0 : undefined}
        onClick={onToggle}
        onKeyDown={(e) => onToggle && (e.key === "Enter" || e.key === " ") && (e.preventDefault(), onToggle())}
        className={`grid grid-cols-[auto_auto_minmax(0,1fr)_auto] ${subject !== undefined ? GRID_WITH_SUBJECT : GRID} items-center gap-3 px-3 py-2 text-sm transition-colors ${
          open ? "rounded-t-xl bg-surface-raised" : "rounded-lg hover:bg-hover"
        } ${onToggle ? "cursor-pointer" : ""}`}
      >
        <span className="font-mono text-[11px] text-muted" title={new Date(entry.ts / 1000).toLocaleString()}>
          {shortTime(entry.ts)}
        </span>
        <FlowIcon view={view} />
        {subject !== undefined && <span className="hidden md:block min-w-0 truncate">{subject}</span>}
        <span className="hidden md:block truncate" title={view.cell ? `Cell/sector ${view.cell}` : undefined}>
          {view.channel ? <Chip className="text-secondary">{view.channel}</Chip> : null}
        </span>
        <span className="flex min-w-0 items-center gap-2">
          <span className="grid w-4 shrink-0 place-items-center">{view.mark}</span>
          <span className="shrink-0 font-medium text-primary">{view.type}</span>
          <span className="hidden sm:block min-w-0 truncate text-xs text-muted">{view.summary}</span>
        </span>
        <span className="flex items-center justify-end gap-1">
          <span className="hidden lg:block">
            <SeqSlots view={view} entry={entry} />
          </span>
          <span
            className={`ml-1 inline-block text-xs text-dimmed transition-transform ${open ? "rotate-90" : ""}`}
            aria-hidden="true"
          >
            ›
          </span>
        </span>
      </div>
      {open && (
        <div className="rounded-b-xl bg-surface-raised px-4 pb-4 pt-1">
          <div className="mb-3 flex flex-wrap items-center gap-1.5 border-t border-border-subtle pt-3">
            <span className="mr-1 text-xs text-muted">{view.direction}</span>
            {view.channel && <Chip className="text-secondary">{view.channel}</Chip>}
            {view.cell && <Chip className="text-muted">cell {view.cell}</Chip>}
            <SeqChips view={view} entry={entry} />
            <span className="ml-auto flex items-center gap-1.5">
              {actions}
              {onToggle && (
                <button
                  type="button"
                  onClick={onToggle}
                  className="rounded-md px-2 py-0.5 text-[11px] text-muted hover:bg-hover hover:text-secondary"
                >
                  Close
                </button>
              )}
            </span>
          </div>
          {view.detail ?? <p className="text-xs text-muted">No decoded detail for this message.</p>}
        </div>
      )}
    </div>
  );
}
