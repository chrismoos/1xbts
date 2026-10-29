"use client";

import { useCallback, useEffect, useState } from "react";
import { createPortal } from "react-dom";
import { useEventStream } from "@/lib/use-event-stream";
import {
  type AuthVerdict,
  type VerdictKind,
  authDataLabel,
  procedureLabel,
  resultLabel,
  verdictFeedback,
  verdictKind,
  verdictStatus,
} from "@/lib/authentication-verdict";

const ACTIVITY_LIMIT = 200;
const PAGE_SIZES = [10, 25, 50] as const;
const DEFAULT_PAGE_SIZE = 10;

export function useAuthenticationVerdicts(
  imsis: string[],
  esn?: number,
): { events: AuthVerdict[]; error: string | null } {
  const [events, setEvents] = useState<AuthVerdict[]>([]);
  const [error, setError] = useState<string | null>(null);
  const imsiKey = imsis.join(",");

  useEffect(() => {
    if (imsis.length === 0 && esn == null) return;
    const params = new URLSearchParams();
    for (const imsi of imsis) params.append("imsi", imsi);
    if (esn != null) params.set("esn", String(esn));
    params.set("limit", String(ACTIVITY_LIMIT));
    let cancelled = false;
    fetch(`/api/authentication-events?${params}`)
      .then((res) => res.json())
      .then((data) => {
        if (cancelled) return;
        if (data.error) setError(data.error);
        else setEvents(data.events ?? []);
      })
      .catch((e) => !cancelled && setError(e instanceof Error ? e.message : "load failed"));
    return () => {
      cancelled = true;
    };
    // imsiKey stands in for the imsis array.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [imsiKey, esn]);

  useEventStream(
    "authentication",
    useCallback(
      (data: string) => {
        const verdict: AuthVerdict = JSON.parse(data);
        const matches =
          (verdict.imsi != null && imsiKey.split(",").includes(verdict.imsi)) ||
          (esn != null && verdict.esn === esn);
        if (matches) setEvents((prev) => [verdict, ...prev].slice(0, ACTIVITY_LIMIT));
      },
      [imsiKey, esn],
    ),
  );
  return { events, error };
}

export function AuthenticationActivity({ imsis, esn }: { imsis: string[]; esn?: number }) {
  const { events, error } = useAuthenticationVerdicts(imsis, esn);
  const [page, setPage] = useState(0);
  const [pageSize, setPageSize] = useState<number>(DEFAULT_PAGE_SIZE);

  if (error) return <p className="text-xs text-accent-red">{error}</p>;
  if (events.length === 0) {
    return <p className="text-xs text-dimmed">No authentication activity since the MSC started.</p>;
  }
  const pages = Math.max(1, Math.ceil(events.length / pageSize));
  const current = Math.min(page, pages - 1);
  const first = current * pageSize;
  const rows = events.slice(first, first + pageSize);
  return (
    <div>
      <table className="w-full text-xs">
        <thead>
          <tr className="text-muted text-left">
            <th className="font-normal py-1 pr-3">Time</th>
            <th className="font-normal py-1 pr-2 w-6" />
            <th className="font-normal py-1 pr-3">Procedure</th>
            <th className="font-normal py-1 pr-3">Result</th>
            <th className="font-normal py-1 pr-3">Service</th>
            <th className="font-normal py-1">Base station</th>
          </tr>
        </thead>
        <tbody>
          {rows.map((verdict, index) => (
            <tr
              key={`${verdict.time}-${verdict.procedure}-${first + index}`}
              className="border-t border-border-subtle"
            >
              <td className="py-1 pr-3 font-mono">
                {verdict.time ? new Date(verdict.time).toLocaleTimeString() : "—"}
              </td>
              <td className="py-1 pr-2">
                <AuthenticationMark verdict={verdict} />
              </td>
              <td className="py-1 pr-3">{procedureLabel(verdict)}</td>
              <td className="py-1 pr-3">{resultLabel(verdict)}</td>
              <td className={`py-1 pr-3 ${verdict.serviceRejected ? "text-accent-red" : "text-muted"}`}>
                {verdict.serviceRejected ? "rejected" : "allowed"}
              </td>
              <td className="py-1 text-muted">{verdict.baseStation || "—"}</td>
            </tr>
          ))}
        </tbody>
      </table>
      <div className="flex items-center justify-end gap-3 mt-2 text-xs text-muted">
        <label className="flex items-center gap-1">
          Rows
          <select
            value={pageSize}
            onChange={(e) => {
              setPageSize(Number(e.target.value));
              setPage(0);
            }}
            className="bg-surface-raised border border-border-input rounded px-1 py-0.5 text-secondary"
          >
            {PAGE_SIZES.map((size) => (
              <option key={size} value={size}>
                {size}
              </option>
            ))}
          </select>
        </label>
        <span className="font-mono">
          {first + 1}–{Math.min(events.length, first + pageSize)} of {events.length}
        </span>
        <button
          type="button"
          disabled={current === 0}
          onClick={() => setPage(current - 1)}
          className="px-2 py-0.5 rounded border border-border-input bg-surface-raised text-secondary disabled:text-dimmed disabled:cursor-default"
          aria-label="Previous page"
        >
          ‹
        </button>
        <button
          type="button"
          disabled={current >= pages - 1}
          onClick={() => setPage(current + 1)}
          className="px-2 py-0.5 rounded border border-border-input bg-surface-raised text-secondary disabled:text-dimmed disabled:cursor-default"
          aria-label="Next page"
        >
          ›
        </button>
      </div>
    </div>
  );
}

const MARK_GLYPH: Record<VerdictKind, string> = { ok: "✓", fail: "✗", pending: "!" };
const MARK_CLASS: Record<VerdictKind, string> = {
  ok: "text-accent-green bg-accent-green-bg",
  fail: "text-accent-red bg-accent-red-bg",
  pending: "text-accent-amber bg-accent-amber-bg",
};

function hex(value: number, digits: number): string {
  return `0x${value.toString(16).toUpperCase().padStart(digits, "0")}`;
}

const CARD_WIDTH_PX = 288;
const CARD_GAP_PX = 8;

export function AuthenticationMark({ verdict }: { verdict: AuthVerdict | null }) {
  const [anchor, setAnchor] = useState<DOMRect | null>(null);
  if (!verdict) return <span className="inline-block w-5 shrink-0" />;
  const kind = verdictKind(verdict);
  const authData = authDataLabel(verdict);
  const rows: [string, string][] = [
    ["Result", resultLabel(verdict)],
    ["Service", verdict.serviceRejected ? "rejected" : "allowed"],
  ];
  if (verdict.rand != null) {
    rows.push(["RAND", `${hex(verdict.rand, 8)} (RANDC ${hex(verdict.rand >>> 24, 2)})`]);
  }
  if (authData) rows.push(["AUTH_DATA", authData]);
  if (verdict.count != null) rows.push(["COUNT", String(verdict.count)]);
  if (verdict.time) {
    rows.push([
      "At",
      `${new Date(verdict.time).toLocaleTimeString()}${verdict.baseStation ? ` · ${verdict.baseStation}` : ""}`,
    ]);
  }
  // Cards and tables clip overflow, so the card is placed in a portal at a fixed position.
  const left = anchor
    ? Math.min(anchor.right + CARD_GAP_PX, window.innerWidth - CARD_WIDTH_PX - CARD_GAP_PX)
    : 0;
  const top = anchor ? Math.max(CARD_GAP_PX, anchor.top - CARD_GAP_PX) : 0;
  return (
    <span
      className={`inline-flex w-5 h-5 shrink-0 items-center justify-center rounded-md text-xs font-bold cursor-default ${MARK_CLASS[kind]}`}
      onClick={(e) => e.stopPropagation()}
      onMouseEnter={(e) => setAnchor(e.currentTarget.getBoundingClientRect())}
      onMouseLeave={() => setAnchor(null)}
      aria-label={`${procedureLabel(verdict)}: ${verdictStatus(verdict)}`}
    >
      {MARK_GLYPH[kind]}
      {anchor &&
        createPortal(
          <span
            style={{ left, top, width: CARD_WIDTH_PX }}
            className="fixed z-50 block rounded-lg border border-border-primary bg-popover p-3 text-left font-normal shadow-xl pointer-events-none"
          >
            <span className="flex items-center gap-2 text-sm font-semibold text-primary mb-1.5">
              {procedureLabel(verdict)}
              <span className={`text-[10.5px] font-semibold px-1.5 rounded-full ${MARK_CLASS[kind]}`}>
                {verdictStatus(verdict)}
              </span>
            </span>
            <span className="grid grid-cols-[4.5rem_1fr] gap-x-2 gap-y-0.5 text-xs">
              {rows.map(([label, value]) => (
                <span key={label} className="contents">
                  <span className="text-muted">{label}</span>
                  <span className="font-mono text-secondary break-words">{value}</span>
                </span>
              ))}
            </span>
            <span className="block mt-2 pt-2 border-t border-border-primary text-xs text-secondary">
              {verdictFeedback(verdict)}
            </span>
          </span>,
          document.body,
        )}
    </span>
  );
}
