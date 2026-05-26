"use client";

import { useEffect, useState } from "react";
import { Card } from "@/components/card";
import { formatTimeMs as formatTime } from "@/lib/format";
import { smsStateColor } from "@/lib/sms-state";
import { teleserviceKind, teleserviceName } from "@/lib/teleservice";

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

function formatIsoTime(value?: string): string {
  if (!value) return "-";
  const ts = Date.parse(value);
  return Number.isFinite(ts) ? formatTime(ts) : "-";
}

export function RecentMessagesCard({ phone, limit = 10 }: { phone?: string; limit?: number }) {
  const [rows, setRows] = useState<SmsRow[]>([]);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    if (!phone) {
      setRows([]);
      setLoading(false);
      return;
    }
    let alive = true;
    const load = async () => {
      try {
        const params = new URLSearchParams({ phone, limit: String(limit) });
        const res = await fetch(`/api/sms-history?${params}`);
        if (!res.ok) throw new Error(`HTTP ${res.status}`);
        const data = await res.json();
        if (!alive) return;
        if (data.error) throw new Error(data.error);
        setRows(data.submissions || []);
        setError(null);
      } catch (err) {
        if (!alive) return;
        setError(err instanceof Error ? err.message : "unknown");
      } finally {
        if (alive) setLoading(false);
      }
    };
    load();
    const interval = setInterval(load, 10000);
    return () => {
      alive = false;
      clearInterval(interval);
    };
  }, [phone, limit]);

  if (!phone) {
    return (
      <Card title="Recent Messages">
        <p className="text-dimmed text-sm">
          No phone number on this subscriber; SMS history is per phone.
        </p>
      </Card>
    );
  }

  return (
    <Card title="Recent Messages">
      {loading && rows.length === 0 ? (
        <p className="text-dimmed text-sm">Loading...</p>
      ) : error ? (
        <p className="text-accent-red text-sm">{error}</p>
      ) : rows.length === 0 ? (
        <p className="text-dimmed text-sm">No messages.</p>
      ) : (
        <div className="overflow-x-auto">
          <table className="w-full text-sm">
            <thead>
              <tr className="text-muted text-xs">
                <th className="text-left py-1 pr-6">Time</th>
                <th className="text-left py-1 pr-6">Direction</th>
                <th className="text-left py-1 pr-6">Peer</th>
                <th className="text-left py-1 pr-6">Type</th>
                <th className="text-left py-1 pr-6">Body</th>
                <th className="text-left py-1">State</th>
              </tr>
            </thead>
            <tbody>
              {rows.map((sms) => {
                const isMt = sms.destinationNumber === phone;
                const peer = isMt ? sms.originatingNumber : sms.destinationNumber;
                const kind = teleserviceKind(sms.teleserviceId);
                const body = kind === "wap-push"
                  ? `[binary, ${sms.rawUserDataHex ? sms.rawUserDataHex.length / 2 : 0} bytes]`
                  : sms.text;
                return (
                  <tr key={sms.smsId} className="border-t border-border hover:bg-hover">
                    <td className="py-1.5 pr-6 text-muted font-mono text-xs whitespace-nowrap">
                      {formatIsoTime(sms.createdAt)}
                    </td>
                    <td className="py-1.5 pr-6 text-xs">
                      <span className={`px-2 py-0.5 rounded ${
                        isMt
                          ? "bg-badge-blue-bg text-badge-blue-text"
                          : "bg-badge-yellow-bg text-badge-yellow-text"
                      }`}>
                        {isMt ? "Incoming" : "Outgoing"}
                      </span>
                    </td>
                    <td className="py-1.5 pr-6 text-secondary font-mono text-xs whitespace-nowrap">{peer || "-"}</td>
                    <td className="py-1.5 pr-6 text-xs text-muted whitespace-nowrap">
                      {teleserviceName(sms.teleserviceId)}
                    </td>
                    <td className={`py-1.5 pr-6 text-xs max-w-[20rem] truncate ${
                      kind === "wap-push" ? "text-dimmed font-mono" : "text-secondary"
                    }`}>
                      {body}
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
                );
              })}
            </tbody>
          </table>
        </div>
      )}
    </Card>
  );
}
