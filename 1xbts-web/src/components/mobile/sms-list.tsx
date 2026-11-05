"use client";

import { useEffect, useState } from "react";
import { liveSocket, useLive } from "@/lib/live";
import { Pager } from "@/components/pager";
import { smsStateColor } from "@/lib/sms-state";
import { teleserviceKind, teleserviceName } from "@/lib/teleservice";
import { ErrorNotice } from "@/components/error-notice";

const HISTORY_LIMIT = 200;
const PAGE_SIZES = [25, 50, 100] as const;

interface SmsRow {
  smsId: string;
  originatingNumber: string;
  destinationNumber: string;
  text: string;
  state: string;
  failureReason?: string;
  createdAt?: string;
  teleserviceId?: number;
  rawUserDataHex?: string;
}

type Filter = "all" | "received" | "sent";

const FILTERS: { id: Filter; label: string }[] = [
  { id: "all", label: "All" },
  { id: "received", label: "Received" },
  { id: "sent", label: "Sent" },
];

function parseTime(value?: string): number | null {
  const ms = value ? Date.parse(value) : NaN;
  return Number.isFinite(ms) ? ms : null;
}

function relative(ms: number): string {
  const sec = Math.max(0, Math.round((Date.now() - ms) / 1000));
  if (sec < 60) return `${sec}s ago`;
  const min = Math.round(sec / 60);
  if (min < 60) return `${min}m ago`;
  const hr = Math.round(min / 60);
  if (hr < 24) return `${hr}h ago`;
  return new Date(ms).toLocaleDateString([], { month: "short", day: "numeric" });
}

const NO_ROWS: SmsRow[] = [];

export function SmsList({ phone, refreshKey = 0 }: { phone?: string; refreshKey?: number }) {
  const key = phone ? `/api/sms-history?${new URLSearchParams({ phone, limit: String(HISTORY_LIMIT) })}` : null;
  const live = useLive<{ submissions?: SmsRow[] }>(key);
  const rows = live.data?.submissions ?? NO_ROWS;
  const { loading } = live;
  const error = live.errorBody ?? live.error;
  const [page, setPage] = useState(0);
  const [pageSize, setPageSize] = useState<number>(PAGE_SIZES[0]);
  const [filter, setFilter] = useState<Filter>("all");
  const [query, setQuery] = useState("");

  useEffect(() => {
    if (key && refreshKey > 0) liveSocket().refresh(key);
  }, [key, refreshKey]);

  if (!phone) {
    return <p className="text-dimmed text-sm">No phone number on this mobile. SMS history is kept per phone number.</p>;
  }
  if (loading && rows.length === 0) return <p className="text-dimmed text-sm">Loading...</p>;
  if (error) return <ErrorNotice error={error} />;
  if (rows.length === 0) return <p className="text-dimmed text-sm">No messages.</p>;

  const q = query.trim().toLowerCase();
  const filtered = rows.filter((sms) => {
    const received = sms.destinationNumber === phone;
    if (filter === "received" && !received) return false;
    if (filter === "sent" && received) return false;
    return !q || `${sms.originatingNumber} ${sms.destinationNumber} ${sms.text}`.toLowerCase().includes(q);
  });
  const pages = Math.max(1, Math.ceil(filtered.length / pageSize));
  const current = Math.min(page, pages - 1);
  const shown = filtered.slice(current * pageSize, (current + 1) * pageSize);
  return (
    <>
      <div className="mb-3 flex flex-wrap items-center gap-2">
        <div className="inline-flex rounded border border-border bg-surface p-0.5">
          {FILTERS.map((f) => (
            <button
              key={f.id}
              type="button"
              onClick={() => {
                setFilter(f.id);
                setPage(0);
              }}
              className={`rounded px-2.5 py-1 text-xs transition-colors ${
                filter === f.id ? "bg-surface-raised text-secondary" : "text-muted hover:text-secondary"
              }`}
            >
              {f.label}
            </button>
          ))}
        </div>
        <input
          type="search"
          value={query}
          onChange={(e) => {
            setQuery(e.target.value);
            setPage(0);
          }}
          placeholder="Search number or text"
          className="min-w-48 flex-1 glass-input text-sm"
        />
      </div>
      {shown.length === 0 ? (
        <p className="text-dimmed text-sm">No messages match.</p>
      ) : (
        <div className="space-y-0.5">
          {shown.map((sms) => {
            const received = sms.destinationNumber === phone;
            const binary = teleserviceKind(sms.teleserviceId) === "wap-push";
            const ms = parseTime(sms.createdAt);
            return (
              <div
                key={sms.smsId}
                className="grid grid-cols-[auto_minmax(0,1fr)_auto] items-center gap-3 rounded-lg px-3 py-2.5 hover:bg-hover"
              >
                <span
                  className={`grid h-8 w-8 place-items-center rounded-full text-sm ${
                    received ? "bg-accent-green-bg text-accent-green" : "bg-accent-blue-bg text-accent-blue"
                  }`}
                  title={received ? "Received by the phone" : "Sent by the phone"}
                >
                  {received ? "↓" : "↑"}
                </span>
                <div className="min-w-0">
                  <div className="flex items-center gap-2 text-xs text-muted">
                    <span className="font-mono text-secondary">{sms.originatingNumber || "-"}</span>
                    <span>→</span>
                    <span className="font-mono text-secondary">{sms.destinationNumber || "-"}</span>
                    <span className="hidden sm:inline">· {teleserviceName(sms.teleserviceId)}</span>
                  </div>
                  <div className={`mt-0.5 truncate text-sm ${binary ? "font-mono text-dimmed" : "text-primary"}`}>
                    {binary
                      ? `[binary, ${sms.rawUserDataHex ? sms.rawUserDataHex.length / 2 : 0} bytes]`
                      : sms.text || <span className="text-dimmed">(empty)</span>}
                  </div>
                  {sms.failureReason && <div className="mt-0.5 text-xs text-accent-red">{sms.failureReason}</div>}
                </div>
                <div className="flex flex-col items-end gap-1">
                  <span className={`rounded-md px-2 py-0.5 text-[11px] ${smsStateColor(sms.state)}`}>{sms.state}</span>
                  <span className="text-[11px] text-muted" title={ms != null ? new Date(ms).toLocaleString() : undefined}>
                    {ms != null ? relative(ms) : "-"}
                  </span>
                </div>
              </div>
            );
          })}
        </div>
      )}
      <Pager
        page={current}
        pageSize={pageSize}
        total={filtered.length}
        pageSizes={PAGE_SIZES}
        onPage={setPage}
        onPageSize={(size) => {
          setPageSize(size);
          setPage(0);
        }}
      />
    </>
  );
}
