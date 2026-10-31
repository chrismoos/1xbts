"use client";

import { useCallback, useEffect, useMemo, useState } from "react";
import { createPortal } from "react-dom";
import { BaseStationLink } from "@/components/activity-links";
import { useBaseStations } from "@/lib/use-base-stations";
import { useEventStream } from "@/lib/use-event-stream";
import {
  type AccessClass,
  type AccessPolicy,
  type AdmissionVerdict,
  CLASS_BADGE,
  CLASS_LABELS,
  accessClass,
  latestAdmissionForMobile,
  createSubscriberHref,
  procedureName,
  reasonLabel,
  serviceLabel,
} from "@/lib/admission";

const ACTIVITY_LIMIT = 200;
const PAGE_SIZES = [10, 25, 50] as const;
const DEFAULT_PAGE_SIZE = 10;

export interface AdmissionFilter {
  imsis?: string[];
  esn?: number;
  meid?: string;
}

function matches(verdict: AdmissionVerdict, filter: AdmissionFilter): boolean {
  const unfiltered = !filter.imsis?.length && filter.esn == null && !filter.meid;
  return (
    unfiltered ||
    (verdict.imsi != null && (filter.imsis ?? []).includes(verdict.imsi)) ||
    (filter.esn != null && verdict.esn === filter.esn) ||
    (!!filter.meid && verdict.meid?.toUpperCase() === filter.meid.toUpperCase())
  );
}

export function useAdmissionVerdicts(
  filter: AdmissionFilter,
  limit = ACTIVITY_LIMIT,
  currentOnly = false,
): { events: AdmissionVerdict[]; policy: AccessPolicy | null; error: string | null } {
  const [events, setEvents] = useState<AdmissionVerdict[]>([]);
  const [policy, setPolicy] = useState<AccessPolicy | null>(null);
  const [error, setError] = useState<string | null>(null);
  const filterKey = JSON.stringify(filter);
  const stableFilter: AdmissionFilter = useMemo(() => JSON.parse(filterKey), [filterKey]);

  useEffect(() => {
    const params = new URLSearchParams();
    for (const imsi of stableFilter.imsis ?? []) params.append("imsi", imsi);
    if (stableFilter.esn != null) params.set("esn", String(stableFilter.esn));
    if (stableFilter.meid) params.set("meid", stableFilter.meid);
    params.set("limit", String(limit));
    if (currentOnly) params.set("current", "1");
    let cancelled = false;
    const load = () => fetch(`/api/admission-events?${params}`)
      .then((res) => res.json())
      .then((data) => {
        if (cancelled) return;
        if (data.error) {
          setError(data.error);
          return;
        }
        setEvents(data.events ?? []);
        setPolicy({
          accessMode: data.accessMode,
          registerPolicy: data.registerPolicy,
          authenticationMode: data.authenticationMode,
        });
      })
      .catch((e) => !cancelled && setError(e instanceof Error ? e.message : "load failed"));
    load();
    const interval = currentOnly ? setInterval(load, 10_000) : undefined;
    return () => {
      cancelled = true;
      if (interval) clearInterval(interval);
    };
  }, [stableFilter, limit, currentOnly]);

  useEventStream(
    "admission",
    useCallback(
      (data: string) => {
        const verdict: AdmissionVerdict = JSON.parse(data);
        if (currentOnly) return;
        if (matches(verdict, stableFilter)) setEvents((prev) => [verdict, ...prev].slice(0, limit));
      },
      [stableFilter, limit, currentOnly],
    ),
  );
  return { events, policy, error };
}

export function latestByDevice(events: AdmissionVerdict[]): Map<string, AdmissionVerdict> {
  const latest = new Map<string, AdmissionVerdict>();
  for (const event of events) {
    const key = deviceKey(event.imsi, event.esn, event.meid);
    if (!latest.has(key)) latest.set(key, event);
  }
  return latest;
}

export function deviceKey(imsi?: string, esn?: number, meid?: string): string {
  return `${imsi ?? ""}/${esn ?? ""}/${(meid ?? "").toUpperCase()}`;
}

export function latestFor(
  events: AdmissionVerdict[],
  mobile: { imsi?: string; esn?: number; meid?: string },
): AdmissionVerdict | undefined {
  return latestAdmissionForMobile(events, mobile);
}

export function AccessBadge({ value }: { value?: string }) {
  const kind = accessClass(value);
  if (!kind) return <span className="text-dimmed text-xs">Unknown</span>;
  return (
    <span className={`text-xs px-2 py-0.5 rounded whitespace-nowrap ${CLASS_BADGE[kind]}`}>
      {CLASS_LABELS[kind]}
    </span>
  );
}

const MARK: Record<AccessClass, { glyph: string; className: string }> = {
  admitted: { glyph: "✓", className: "text-accent-green bg-accent-green-bg" },
  limited: { glyph: "!", className: "text-accent-amber bg-accent-amber-bg" },
  provisioning: { glyph: "!", className: "text-accent-amber bg-accent-amber-bg" },
  registration_only: { glyph: "!", className: "text-accent-amber bg-accent-amber-bg" },
  denied: { glyph: "✗", className: "text-accent-red bg-accent-red-bg" },
};

const CARD_WIDTH_PX = 300;
const CARD_GAP_PX = 8;

function feedback(verdict: AdmissionVerdict, kind: AccessClass): string {
  const reason = verdict.reason?.replace(/^ADMISSION_REASON_/, "").toLowerCase();
  switch (reason) {
    case "not_a_subscriber":
      return "Create a subscriber with this identity. The handset is admitted on its next registration.";
    case "subscriber_disabled":
      return "The subscriber is disabled. Set it to active to restore service.";
    case "subscriber_suspended":
      return "The subscriber is suspended. It registers and receives SMS only.";
    case "hardware_staged":
      return "Run OTASP to program the configured IMSI. Ordinary service starts once the handset reports it.";
    case "ambiguous_identity":
      return "More than one subscriber has this ESN or MEID. Remove the duplicate.";
    case "identity_conflict":
      return "The handset's ESN and MEID disagree with the subscriber's configured hardware.";
    case "authentication_required":
      return "The subscriber has an A-key, so provisioning needs a valid authentication signature.";
    case "authentication_recovery_required":
      return "The A-key has no SSD yet. An ordinary call runs the SSD update that recovers it.";
    case "hlr_error":
      return "The HLR could not be reached, so closed access refused the mobile.";
    default:
      return kind === "admitted" ? "The mobile has full service." : "The access class does not permit this service.";
  }
}

export function AccessMark({ verdict }: { verdict: AdmissionVerdict }) {
  const [anchor, setAnchor] = useState<DOMRect | null>(null);
  const kind = accessClass(verdict.admissionClass) ?? "denied";
  const mark = verdict.serviceRejected ? MARK.denied : MARK[kind];
  const rows: [string, string][] = [
    ["Access", CLASS_LABELS[kind]],
    ["Reason", reasonLabel(verdict.reason)],
    ["Service", `${serviceLabel(verdict.service)} ${verdict.serviceRejected ? "refused" : "allowed"}`],
  ];
  if (verdict.configuredImsi && verdict.configuredImsi !== verdict.imsi) {
    rows.push(["Configured", `IMSI ${verdict.configuredImsi}`]);
  }
  if (verdict.time) {
    rows.push([
      "At",
      `${new Date(verdict.time).toLocaleTimeString()}${verdict.baseStation ? ` · ${verdict.baseStation}` : ""}`,
    ]);
  }
  const left = anchor
    ? Math.min(anchor.right + CARD_GAP_PX, window.innerWidth - CARD_WIDTH_PX - CARD_GAP_PX)
    : 0;
  const top = anchor ? Math.max(CARD_GAP_PX, anchor.top - CARD_GAP_PX) : 0;
  return (
    <span
      className={`inline-flex w-5 h-5 shrink-0 items-center justify-center rounded-md text-xs font-bold cursor-default ${mark.className}`}
      onMouseEnter={(e) => setAnchor(e.currentTarget.getBoundingClientRect())}
      onMouseLeave={() => setAnchor(null)}
      aria-label={`${procedureName(verdict.procedure)}: ${CLASS_LABELS[kind]}`}
    >
      {mark.glyph}
      {anchor &&
        createPortal(
          <span
            style={{ left, top, width: CARD_WIDTH_PX }}
            className="fixed z-50 block rounded-lg border border-border-primary bg-popover p-3 text-left font-normal shadow-xl pointer-events-none"
          >
            <span className="flex items-center gap-2 text-sm font-semibold text-primary mb-1.5 capitalize">
              {procedureName(verdict.procedure)}
              <span className={`text-[10.5px] font-semibold px-1.5 rounded-full normal-case ${CLASS_BADGE[kind]}`}>
                {verdict.serviceRejected ? "refused" : CLASS_LABELS[kind].toLowerCase()}
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
              {feedback(verdict, kind)}
            </span>
          </span>,
          document.body,
        )}
    </span>
  );
}

export function AccessActivity({ filter }: { filter: AdmissionFilter }) {
  const { nodes } = useBaseStations();
  const { events, policy, error } = useAdmissionVerdicts(filter);
  const [page, setPage] = useState(0);
  const [pageSize, setPageSize] = useState<number>(DEFAULT_PAGE_SIZE);

  if (error) return <p className="text-xs text-accent-red">{error}</p>;
  if (events.length === 0) {
    return (
      <p className="text-xs text-dimmed">
        {policy?.accessMode === "closed"
          ? "No access decisions recorded since the MSC started."
          : "Access is open. Decisions are recorded for provisioning refusals only."}
      </p>
    );
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
            <th className="font-normal py-1 pr-3">Access</th>
            <th className="font-normal py-1 pr-3">Reason</th>
            <th className="font-normal py-1">Base station</th>
          </tr>
        </thead>
        <tbody>
          {rows.map((verdict, index) => (
            <tr key={`${verdict.time}-${first + index}`} className="border-t border-border-subtle">
              <td className="py-1 pr-3 font-mono">
                {verdict.time ? new Date(verdict.time).toLocaleTimeString() : "—"}
              </td>
              <td className="py-1 pr-2">
                <AccessMark verdict={verdict} />
              </td>
              <td className="py-1 pr-3 capitalize">{procedureName(verdict.procedure)}</td>
              <td className="py-1 pr-3">
                <AccessBadge value={verdict.admissionClass} />
              </td>
              <td className="py-1 pr-3">
                {reasonLabel(verdict.reason)}
                {verdict.serviceRejected && (
                  <span className="text-muted"> · {serviceLabel(verdict.service)} refused</span>
                )}
              </td>
              <td className="py-1 text-muted"><BaseStationLink baseStation={verdict.baseStation} nodes={nodes} /></td>
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

export function AccessStatus({
  imsi,
  esn,
  meid,
  subscriberId,
}: {
  imsi?: string;
  esn?: number;
  meid?: string;
  subscriberId?: string;
}) {
  const { nodes } = useBaseStations();
  const { events, policy } = useAdmissionVerdicts({ imsis: imsi ? [imsi] : [], esn, meid }, 0, true);
  if (policy?.accessMode !== "closed") return null;
  const verdict = latestFor(events, { imsi, esn, meid });
  const kind = accessClass(verdict?.admissionClass);
  return (
    <div className="mb-3 pb-3 border-b border-border-subtle">
      <div className="flex justify-between items-center py-0.5">
        <span className="text-muted text-sm">Access</span>
        {verdict ? <AccessBadge value={verdict.admissionClass} /> : <span className="text-dimmed text-xs">No decision yet</span>}
      </div>
      {verdict && kind !== "admitted" && (
        <div className="text-[11px] text-muted text-right">
          {reasonLabel(verdict.reason)}
          {verdict.serviceRejected && ` · ${serviceLabel(verdict.service)} refused`}
          {verdict.time && ` · ${new Date(verdict.time).toLocaleTimeString()}`}
          {verdict.baseStation && <> · <BaseStationLink baseStation={verdict.baseStation} nodes={nodes} /></>}
        </div>
      )}
      {verdict && !subscriberId && !verdict.subscriberId && kind !== "admitted" && (
        <a
          href={createSubscriberHref({ imsi, esn, meid })}
          className="inline-block mt-2 text-xs px-3 py-1 rounded bg-accent-green-bg text-accent-green border border-accent-green/20 hover:bg-accent-green/15"
        >
          Create subscriber from this mobile
        </a>
      )}
    </div>
  );
}
