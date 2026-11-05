"use client";

import { useEffect, useState } from "react";
import { Card } from "@/components/card";
import { formatTimeMs as formatTime } from "@/lib/format";
import { smsStateColor } from "@/lib/sms-state";
import { teleserviceKind, teleserviceName } from "@/lib/teleservice";
import { apiFetch } from "@/lib/api-client";
import { ErrorNotice } from "@/components/error-notice";

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
  const [error, setError] = useState<unknown>(null);

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
        const data = await apiFetch<{ submissions?: typeof rows }>(
          `/api/sms-history?${params}`,
          undefined,
          "load recent messages",
        );
        if (!alive) return;
        setRows(data.submissions || []);
        setError(null);
      } catch (err) {
        if (!alive) return;
        setError(err);
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
        <ErrorNotice error={error} />
      ) : rows.length === 0 ? (
        <p className="text-dimmed text-sm">No messages.</p>
      ) : (
        <div className="overflow-x-auto">
          <table className="data-table">
            <thead>
              <tr>
                <th>Time</th>
                <th>Direction</th>
                <th>Peer</th>
                <th>Type</th>
                <th>Body</th>
                <th>State</th>
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
                  <tr key={sms.smsId}>
                    <td className="text-muted font-mono whitespace-nowrap">
                      {formatIsoTime(sms.createdAt)}
                    </td>
                    <td>
                      <span className={`px-2 py-0.5 rounded ${
                        isMt
                          ? "bg-badge-blue-bg text-badge-blue-text"
                          : "bg-badge-yellow-bg text-badge-yellow-text"
                      }`}>
                        {isMt ? "Incoming" : "Outgoing"}
                      </span>
                    </td>
                    <td className="text-secondary font-mono whitespace-nowrap">{peer || "-"}</td>
                    <td className="text-muted whitespace-nowrap">
                      {teleserviceName(sms.teleserviceId)}
                    </td>
                    <td className={`max-w-[20rem] truncate ${
                      kind === "wap-push" ? "text-dimmed font-mono" : "text-secondary"
                    }`}>
                      {body}
                    </td>
                    <td>
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
