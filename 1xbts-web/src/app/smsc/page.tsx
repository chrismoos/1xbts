"use client";

import { Fragment, useState, useEffect, useCallback } from "react";
import { Card } from "@/components/card";
import { KV, WapBody } from "@/components/wap-body";
import { formatTimeMs as formatTime, hexToBytes } from "@/lib/format";
import { smsStateColor } from "@/lib/sms-state";
import { teleserviceKind, teleserviceName } from "@/lib/teleservice";

// ─── Types ──────────────────────────────────────────────────────

interface SmsSubmission {
  smsId: string;
  originatingNumber: string;
  destinationNumber: string;
  originatingSubscriberId?: string;
  destinationSubscriberId?: string;
  destinationEsn?: number;
  destinationImsi?: string;
  text: string;
  state: string;
  failureReason?: string;
  createdAt?: string;
  updatedAt?: string;
  teleserviceId?: number;
  rawUserDataHex?: string;
}

// ─── Helpers ────────────────────────────────────────────────────

function formatIsoTime(value?: string): string {
  if (!value) return "-";
  const ts = Date.parse(value);
  return Number.isFinite(ts) ? formatTime(ts) : "-";
}

// ─── Per-row expanded detail ────────────────────────────────────

function SubmissionDetail({ sms }: { sms: SmsSubmission }) {
  const kind = teleserviceKind(sms.teleserviceId);
  const teleHex = sms.teleserviceId !== undefined
    ? `0x${sms.teleserviceId.toString(16).toUpperCase().padStart(4, "0")}`
    : "-";

  return (
    <div className="flex flex-col gap-3">
      <div className="grid grid-cols-1 md:grid-cols-2 gap-x-6 gap-y-1">
        <KV label="sms_id" value={sms.smsId} />
        <KV label="Teleservice" value={`${teleserviceName(sms.teleserviceId)} (${teleHex})`} />
        {sms.originatingSubscriberId && (
          <KV label="From subscriber" value={sms.originatingSubscriberId} />
        )}
        {sms.destinationSubscriberId && (
          <KV label="To subscriber" value={sms.destinationSubscriberId} />
        )}
        {sms.destinationEsn !== undefined && sms.destinationEsn !== 0 && (
          <KV label="Destination ESN" value={`0x${sms.destinationEsn.toString(16).toUpperCase()}`} />
        )}
        {sms.destinationImsi && (
          <KV label="Destination IMSI" value={sms.destinationImsi} />
        )}
        {sms.updatedAt && <KV label="Updated" value={formatIsoTime(sms.updatedAt)} />}
      </div>
      <div>
        <div className="text-xs text-muted mb-1">Body</div>
        {kind === "wap-push" && sms.rawUserDataHex ? (
          <WapBody bytes={hexToBytes(sms.rawUserDataHex)} />
        ) : sms.text ? (
          <pre className="text-xs font-mono text-secondary bg-surface-solid p-2 rounded whitespace-pre-wrap break-all">
            {sms.text}
          </pre>
        ) : (
          <span className="text-xs text-dimmed">(empty)</span>
        )}
      </div>
    </div>
  );
}

// ─── Send SMS Form ──────────────────────────────────────────────

function SendSmsForm({ onSent }: { onSent: () => void }) {
  const [to, setTo] = useState("");
  const [from, setFrom] = useState("");
  const [text, setText] = useState("");
  const [sending, setSending] = useState(false);
  const [result, setResult] = useState<{ ok: boolean; msg: string } | null>(null);

  const handleSubmit = async (e: React.FormEvent) => {
    e.preventDefault();
    const destination = to.trim();
    if (!destination || !text) return;
    setSending(true);
    setResult(null);
    try {
      // Explicit prefix `IMSI:<digits>` routes to a non-subscriber mobile by
      // IMSI (no HLR lookup). Anything else is treated as a subscriber phone
      // number and resolved through HLR.
      const imsiMatch = destination.match(/^IMSI:(\d+)$/i);
      const res = await fetch("/api/sms", {
        method: "POST",
        headers: { "Content-Type": "application/json" },
        body: JSON.stringify({
          destinationImsi: imsiMatch ? imsiMatch[1] : undefined,
          destinationNumber: imsiMatch ? undefined : destination,
          originatingNumber: from || undefined,
          text,
        }),
      });
      const data = await res.json();
      if (data.accepted) {
        setResult({ ok: true, msg: "SMS accepted" });
        setText("");
        onSent();
      } else {
        setResult({ ok: false, msg: data.message || "Rejected" });
      }
    } catch (err) {
      setResult({ ok: false, msg: err instanceof Error ? err.message : "Failed" });
    } finally {
      setSending(false);
    }
  };

  return (
    <Card title="Send SMS">
      <form onSubmit={handleSubmit} className="flex flex-col gap-3">
        <div className="flex gap-3">
          <div className="flex-1">
            <label className="text-xs text-muted block mb-1">To (phone or IMSI:&lt;digits&gt;)</label>
            <input
              type="text"
              value={to}
              onChange={(e) => setTo(e.target.value)}
              className="w-full glass-input font-mono"
              placeholder="5551234 or IMSI:999999999912345"
            />
          </div>
          <div className="flex-1">
            <label className="text-xs text-muted block mb-1">From (optional)</label>
            <input
              type="text"
              value={from}
              onChange={(e) => setFrom(e.target.value)}
              className="w-full glass-input font-mono"
              placeholder="originating number"
            />
          </div>
        </div>
        <div>
          <label className="text-xs text-muted block mb-1">Message</label>
          <textarea
            value={text}
            onChange={(e) => setText(e.target.value)}
            rows={2}
            className="w-full glass-input resize-none"
            placeholder="Message text..."
          />
        </div>
        <div className="flex items-center gap-3">
          <button
            type="submit"
            disabled={sending || !to || !text}
            className="bg-accent-green-bg text-accent-green border border-accent-green/20 hover:bg-accent-green/15 disabled:bg-surface-raised disabled:text-muted text-sm px-4 py-1.5 rounded transition-colors"
          >
            {sending ? "Sending..." : "Send"}
          </button>
          {result && (
            <span className={`text-xs ${result.ok ? "text-accent-green" : "text-accent-red"}`}>
              {result.msg}
            </span>
          )}
        </div>
      </form>
    </Card>
  );
}

// ─── SMS History Table ──────────────────────────────────────────

const PAGE_SIZE = 25;

function SmsHistoryTable() {
  const [submissions, setSubmissions] = useState<SmsSubmission[]>([]);
  const [total, setTotal] = useState(0);
  const [page, setPage] = useState(0);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState<string | null>(null);
  const [stateFilter, setStateFilter] = useState<string | "">("");
  const [expandedId, setExpandedId] = useState<string | null>(null);

  const load = useCallback(async () => {
    try {
      const params = new URLSearchParams({
        limit: String(PAGE_SIZE),
        offset: String(page * PAGE_SIZE),
      });
      if (stateFilter) params.set("state", stateFilter);
      const res = await fetch(`/api/sms-history?${params}`);
      if (!res.ok) throw new Error(`HTTP ${res.status}`);
      const data = await res.json();
      if (data.error) throw new Error(data.error);
      setSubmissions(data.submissions || []);
      setTotal(data.total ?? 0);
      setError(null);
    } catch (err) {
      setError(err instanceof Error ? err.message : "unknown");
    } finally {
      setLoading(false);
    }
  }, [page, stateFilter]);

  useEffect(() => {
    setLoading(true);
    load();
    const interval = setInterval(load, 5000);
    return () => clearInterval(interval);
  }, [load]);

  const totalPages = Math.max(1, Math.ceil(total / PAGE_SIZE));

  const states = ["", "delivered", "sent", "paging", "page_response_received", "failed", "expired"];

  return (
    <Card title={`SMS History (${total})`}>
      <div className="flex items-center gap-2 mb-3">
        <span className="text-xs text-muted">Filter:</span>
        {states.map((s) => (
          <button
            key={s}
            onClick={() => { setStateFilter(s); setPage(0); }}
            className={`text-xs px-2 py-0.5 rounded transition-colors ${
              stateFilter === s
                ? "bg-surface-raised text-primary"
                : "bg-surface-solid text-muted hover:text-primary"
            }`}
          >
            {s || "All"}
          </button>
        ))}
      </div>

      {loading && submissions.length === 0 ? (
        <p className="text-dimmed text-sm">Loading...</p>
      ) : error ? (
        <p className="text-accent-red text-sm">{error}</p>
      ) : submissions.length === 0 ? (
        <p className="text-dimmed text-sm">No SMS submissions.</p>
      ) : (
        <div className="overflow-x-auto">
          <table className="w-full text-sm">
            <thead>
              <tr className="text-muted text-xs">
                <th className="text-left py-1 pr-6">Time</th>
                <th className="text-left py-1 pr-6">From</th>
                <th className="text-left py-1 pr-6">To</th>
                <th className="text-left py-1 pr-6">Type</th>
                <th className="text-left py-1 pr-6">Body</th>
                <th className="text-left py-1">State</th>
              </tr>
            </thead>
            <tbody>
              {submissions.map((sms) => {
                const kind = teleserviceKind(sms.teleserviceId);
                const isOpen = expandedId === sms.smsId;
                const bodyPreview = kind === "wap-push"
                  ? `[binary, ${sms.rawUserDataHex ? sms.rawUserDataHex.length / 2 : 0} bytes]`
                  : (sms.text || "");
                return (
                  <Fragment key={sms.smsId}>
                    <tr
                      className="border-t border-border hover:bg-hover cursor-pointer"
                      onClick={() => setExpandedId(isOpen ? null : sms.smsId)}
                    >
                      <td className="py-1.5 pr-6 text-muted font-mono text-xs whitespace-nowrap">
                        {formatIsoTime(sms.createdAt)}
                      </td>
                      <td className="py-1.5 pr-6 text-muted font-mono text-xs whitespace-nowrap">
                        {sms.originatingNumber || "-"}
                      </td>
                      <td className="py-1.5 pr-6 text-secondary font-mono text-xs whitespace-nowrap">
                        {sms.destinationNumber || "-"}
                      </td>
                      <td className="py-1.5 pr-6">
                        <span className={`text-xs px-2 py-0.5 rounded ${
                          kind === "wap-push"
                            ? "bg-badge-blue-bg text-badge-blue-text"
                            : "bg-surface-raised text-muted"
                        }`}>
                          {teleserviceName(sms.teleserviceId)}
                        </span>
                      </td>
                      <td className={`py-1.5 pr-6 text-xs max-w-[20rem] truncate ${
                        kind === "wap-push" ? "text-dimmed font-mono" : "text-secondary"
                      }`}>
                        {bodyPreview}
                      </td>
                      <td className="py-1.5">
                        <span className={`text-xs px-2 py-0.5 rounded ${smsStateColor(sms.state)}`}>
                          {sms.state}
                        </span>
                        {sms.failureReason && (
                          <span className="text-xs text-accent-red ml-1">{sms.failureReason}</span>
                        )}
                      </td>
                    </tr>
                    {isOpen && (
                      <tr className="bg-surface-raised/30">
                        <td colSpan={6} className="px-3 py-3">
                          <SubmissionDetail sms={sms} />
                        </td>
                      </tr>
                    )}
                  </Fragment>
                );
              })}
            </tbody>
          </table>
        </div>
      )}

      {totalPages > 1 && (
        <div className="flex items-center justify-between mt-3 pt-3 border-t border-border">
          <button
            onClick={() => setPage((p) => Math.max(0, p - 1))}
            disabled={page === 0}
            className="text-xs px-3 py-1 rounded bg-surface-raised text-muted hover:text-primary disabled:text-dimmed disabled:hover:text-dimmed transition-colors"
          >
            Previous
          </button>
          <span className="text-xs text-muted">
            Page {page + 1} of {totalPages}
          </span>
          <button
            onClick={() => setPage((p) => Math.min(totalPages - 1, p + 1))}
            disabled={page >= totalPages - 1}
            className="text-xs px-3 py-1 rounded bg-surface-raised text-muted hover:text-primary disabled:text-dimmed disabled:hover:text-dimmed transition-colors"
          >
            Next
          </button>
        </div>
      )}
    </Card>
  );
}

// ─── Page ───────────────────────────────────────────────────────

export default function SmscPage() {
  const [refreshKey, setRefreshKey] = useState(0);

  return (
    <div className="max-w-7xl mx-auto space-y-4">
      <h1 className="text-lg font-bold">SMSC</h1>
      <SendSmsForm onSent={() => setRefreshKey((k) => k + 1)} />
      <SmsHistoryTable key={refreshKey} />
    </div>
  );
}
