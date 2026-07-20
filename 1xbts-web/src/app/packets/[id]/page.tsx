"use client";

import Link from "next/link";
import { use, useCallback, useEffect, useState } from "react";
import { Card, Stat } from "@/components/card";
import {
  mobileForPacketSession,
  mobileLabel,
  useMobileDirectory,
} from "@/lib/mobile-directory";

interface PacketSessionInfo {
  sessionId: string;
  phase: string;
  serviceOption: number;
  peerIp: string;
  ourIp: string;
  tunDevice: string;
  uplinkFrames: number;
  downlinkFrames: number;
  uplinkBytes: number;
  downlinkBytes: number;
  createdAtMs: number;
  lastPhaseChangeAtMs: number;
  lastUplinkAtMs: number;
  lastDownlinkAtMs: number;
  lastActivityAtMs: number;
  lastUplinkRateBps: number;
  lastDownlinkRateBps: number;
  mobileAddress: string;
  subscriberId: string;
  phoneNumber: string;
  imsi: string;
  meid: string;
  hrpdMnId: string;
  hrpdMnIdSource: string;
  subscriberImsi: string;
  esn: number;
  trafficWalshCode: number;
  rlpState: string;
  lcpState: string;
  ipcpState: string;
  captureEnabled: boolean;
  accessTechnology: string;
}

interface PacketTraceEvent {
  timestampMs: number;
  layer: string;
  direction: string;
  summary: string;
  detail: string;
  payloadHex: string;
}

interface PacketSessionDetail {
  summary?: PacketSessionInfo;
  lastRxControl: string;
  lastTxControl: string;
  lastRxControlRepeats: number;
  lastTxControlRepeats: number;
  recentPppEvents: PacketTraceEvent[];
  captureEvents: PacketTraceEvent[];
}

type PacketSessionResponse = {
  session?: PacketSessionDetail;
  error?: string;
};

function formatTimestamp(ms?: number): string {
  if (!ms) return "-";
  const d = new Date(ms);
  if (Number.isNaN(d.getTime())) return "-";
  return d.toLocaleString();
}

function formatAge(ms?: number): string {
  if (!ms) return "-";
  const delta = Math.max(0, Date.now() - ms);
  const seconds = Math.floor(delta / 1000);
  if (seconds < 60) return `${seconds}s`;
  const minutes = Math.floor(seconds / 60);
  const rem = seconds % 60;
  if (minutes < 60) return `${minutes}m ${rem}s`;
  return `${Math.floor(minutes / 60)}h ${minutes % 60}m`;
}

function formatBytes(bytes: number): string {
  if (bytes === 0) return "0 B";
  if (bytes < 1024) return `${bytes} B`;
  if (bytes < 1024 * 1024) return `${(bytes / 1024).toFixed(1)} KB`;
  return `${(bytes / (1024 * 1024)).toFixed(1)} MB`;
}

function formatPhase(phase: string): string {
  switch (phase) {
    case "rlp_sync":
      return "RLP Sync";
    case "lcp":
      return "LCP";
    case "ipcp":
      return "IPCP";
    case "active":
      return "Active";
    case "closed":
      return "Closed";
    default:
      return phase;
  }
}

function formatStateLabel(value: string): string {
  if (!value) return "-";
  return value
    .split("_")
    .map((part) => part.charAt(0).toUpperCase() + part.slice(1))
    .join(" ");
}

function serviceOptionName(so: number): string {
  switch (so) {
    case 7:
      return "SO 7";
    case 33:
      return "SO 33";
    default:
      return `SO ${so}`;
  }
}

function TabButton({
  active,
  label,
  onClick,
}: {
  active: boolean;
  label: string;
  onClick: () => void;
}) {
  return (
    <button
      onClick={onClick}
      className={`rounded px-3 py-1.5 text-sm transition-colors ${
        active
          ? "bg-accent-green-bg text-accent-green border border-accent-green/20"
          : "bg-surface-raised text-muted hover:text-primary"
      }`}
    >
      {label}
    </button>
  );
}

function TraceList({
  events,
  emptyText,
}: {
  events: PacketTraceEvent[];
  emptyText: string;
}) {
  if (events.length === 0) {
    return <p className="text-sm text-muted">{emptyText}</p>;
  }

  return (
    <div className="space-y-2">
      {events.map((event, index) => (
        <div
          key={`${event.timestampMs}-${event.layer}-${event.direction}-${index}`}
          className="rounded border border-border bg-surface p-3"
        >
          <div className="flex flex-wrap items-center gap-2 text-xs">
            <span className="font-mono text-muted">{formatTimestamp(event.timestampMs)}</span>
            <span className="rounded bg-surface-raised px-2 py-0.5 text-secondary">
              {event.layer}
            </span>
            <span className="rounded bg-surface-raised px-2 py-0.5 text-accent-cyan">
              {event.direction}
            </span>
            <span className="text-primary">{event.summary}</span>
          </div>
          {event.detail && (
            <div className="mt-2 text-xs text-muted">{event.detail}</div>
          )}
          {event.payloadHex && (
            <pre className="mt-2 overflow-x-auto whitespace-pre-wrap break-all rounded bg-black/30 p-2 text-[11px] text-muted">
              {event.payloadHex}
            </pre>
          )}
        </div>
      ))}
    </div>
  );
}

export default function PacketSessionDetailPage({
  params,
}: {
  params: Promise<{ id: string }>;
}) {
  const { id } = use(params);
  const [detail, setDetail] = useState<PacketSessionDetail | null>(null);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState<string | null>(null);
  const [captureBusy, setCaptureBusy] = useState(false);
  const [activeTab, setActiveTab] = useState<"overview" | "rlp" | "ppp" | "capture">("overview");
  const mobiles = useMobileDirectory();

  const fetchDetail = useCallback(async () => {
    try {
      const res = await fetch(`/api/packet-sessions/${encodeURIComponent(id)}`, {
        cache: "no-store",
      });
      const data: PacketSessionResponse = await res.json();
      if (!res.ok || data.error) {
        throw new Error(data.error || `HTTP ${res.status}`);
      }
      setDetail(data.session || null);
      setError(null);
    } catch (err) {
      setError(err instanceof Error ? err.message : "unknown error");
    } finally {
      setLoading(false);
    }
  }, [id]);

  useEffect(() => {
    fetchDetail();
    const interval = setInterval(fetchDetail, 1500);
    return () => clearInterval(interval);
  }, [fetchDetail]);

  const setCapture = useCallback(
    async (enabled: boolean) => {
      setCaptureBusy(true);
      try {
        const res = await fetch(
          `/api/packet-sessions/${encodeURIComponent(id)}/capture`,
          { method: enabled ? "POST" : "DELETE" }
        );
        const data: PacketSessionResponse = await res.json();
        if (!res.ok || data.error) {
          throw new Error(data.error || `HTTP ${res.status}`);
        }
        setDetail(data.session || null);
        setError(null);
      } catch (err) {
        setError(err instanceof Error ? err.message : "unknown error");
      } finally {
        setCaptureBusy(false);
      }
    },
    [id]
  );

  const summary = detail?.summary;
  const mobile = summary ? mobileForPacketSession(summary, mobiles) : undefined;
  const mobileLinkLabel = mobile ? mobileLabel(mobile) : undefined;
  const isHrpd = summary?.accessTechnology === "HRPD";
  const subscriberImsi = summary?.subscriberImsi || summary?.imsi || "";

  if (loading) {
    return (
      <div className="max-w-7xl mx-auto">
        <p className="text-sm text-dimmed">Loading...</p>
      </div>
    );
  }

  if (error || !summary) {
    return (
      <div className="max-w-7xl mx-auto space-y-4">
        <Link href="/packets" className="text-sm text-muted hover:text-primary">
          &larr; Packets
        </Link>
        <div className="rounded-lg border border-accent-red/20 bg-accent-red-bg p-4 text-sm text-accent-red">
          {error || "Session not found"}
        </div>
      </div>
    );
  }

  return (
    <div className="max-w-7xl mx-auto space-y-4">
      <div className="flex flex-wrap items-center gap-4">
        <Link href="/packets" className="text-sm text-muted hover:text-primary">
          &larr; Packets
        </Link>
        {mobile && mobileLinkLabel ? (
          <Link
            href={`/mobiles/${encodeURIComponent(mobile.address)}`}
            className="text-sm text-accent-cyan hover:text-accent-cyan"
          >
            MS {mobileLinkLabel.value}
          </Link>
        ) : summary.mobileAddress ? (
          <span className="text-sm text-muted">
            MS {summary.phoneNumber || summary.mobileAddress}
          </span>
        ) : (
          null
        )}
      </div>

      <div className="flex flex-wrap items-start justify-between gap-4">
        <div>
          <h1 className="font-mono text-lg font-bold text-primary">{summary.sessionId}</h1>
          <div className="mt-1 text-sm text-muted">
            {summary.accessTechnology || "1x"} · {serviceOptionName(summary.serviceOption)} · {formatPhase(summary.phase)} · age {formatAge(summary.createdAtMs)}
          </div>
        </div>
        <button
          onClick={() => void setCapture(!summary.captureEnabled)}
          disabled={captureBusy}
          className={`rounded px-4 py-2 text-sm transition-colors ${
            summary.captureEnabled
              ? "bg-accent-red-bg text-accent-red border border-accent-red/20 hover:bg-accent-red/15"
              : "bg-accent-cyan hover:bg-accent-cyan/80 text-primary"
          } disabled:bg-surface-raised disabled:text-muted`}
        >
          {captureBusy
            ? "Updating..."
            : summary.captureEnabled
              ? "Stop Capture"
              : "Start Capture"}
        </button>
      </div>

      <div className="grid grid-cols-1 md:grid-cols-4 gap-4">
        <Card title="Activity">
          <Stat label="Last Activity" value={formatTimestamp(summary.lastActivityAtMs)} mono />
          <Stat label="Last UL" value={formatTimestamp(summary.lastUplinkAtMs)} mono />
          <Stat label="Last DL" value={formatTimestamp(summary.lastDownlinkAtMs)} mono />
          <Stat label="Phase Changed" value={formatTimestamp(summary.lastPhaseChangeAtMs)} mono />
        </Card>
        <Card title="Bearer">
          <Stat
            label={summary.accessTechnology === "HRPD" ? "A10 Key" : "Walsh"}
            value={
              summary.trafficWalshCode
                ? summary.accessTechnology === "HRPD"
                  ? String(summary.trafficWalshCode)
                  : `W${summary.trafficWalshCode}`
                : "-"
            }
            mono
          />
          <Stat label="Mobile IP" value={summary.peerIp || "-"} mono />
          <Stat label="Gateway IP" value={summary.ourIp || "-"} mono />
          <Stat label="TUN" value={summary.tunDevice || "-"} mono />
        </Card>
        <Card title="Traffic">
          <Stat label="UL Bytes" value={formatBytes(summary.uplinkBytes)} mono />
          <Stat label="DL Bytes" value={formatBytes(summary.downlinkBytes)} mono />
          <Stat label="UL Frames" value={summary.uplinkFrames.toLocaleString()} mono />
          <Stat label="DL Frames" value={summary.downlinkFrames.toLocaleString()} mono />
        </Card>
        <Card title="Subscriber">
          {mobile && mobileLinkLabel && (
            <div className="flex justify-between py-0.5">
              <span className="text-muted text-sm">Mobile</span>
              <Link
                href={`/mobiles/${encodeURIComponent(mobile.address)}`}
                className="text-secondary text-sm font-mono hover:text-accent-cyan"
              >
                {mobileLinkLabel.value}
              </Link>
            </div>
          )}
          <Stat label="Address" value={summary.mobileAddress || "-"} mono />
          <Stat label="Phone" value={summary.phoneNumber || "-"} mono />
          <Stat label="Subscriber IMSI" value={subscriberImsi || "-"} mono />
          <Stat label="Subscriber" value={summary.subscriberId || "-"} mono />
          <Stat label="ESN" value={summary.esn ? `0x${summary.esn.toString(16).toUpperCase().padStart(8, "0")}` : "-"} mono />
          <Stat label="MEID" value={summary.meid || "-"} mono />
          <Stat label="HRPD MN ID" value={summary.hrpdMnId || "-"} mono />
          <Stat label="MN ID Source" value={summary.hrpdMnIdSource || "-"} mono />
        </Card>
      </div>

      <div className="flex flex-wrap gap-2">
        <TabButton active={activeTab === "overview"} label="Overview" onClick={() => setActiveTab("overview")} />
        <TabButton active={activeTab === "rlp"} label={isHrpd ? "Bearer" : "RLP"} onClick={() => setActiveTab("rlp")} />
        <TabButton active={activeTab === "ppp"} label="PPP" onClick={() => setActiveTab("ppp")} />
        <TabButton active={activeTab === "capture"} label="Capture" onClick={() => setActiveTab("capture")} />
      </div>

      {activeTab === "overview" && (
        <div className="grid grid-cols-1 lg:grid-cols-2 gap-4">
          <Card title="State">
            <Stat label="Access" value={summary.accessTechnology || "1x"} />
            <Stat label="Phase" value={formatPhase(summary.phase)} />
            <Stat label={isHrpd ? "A10" : "RLP"} value={formatStateLabel(summary.rlpState)} />
            <Stat label="LCP" value={formatStateLabel(summary.lcpState)} />
            <Stat label="IPCP" value={formatStateLabel(summary.ipcpState)} />
          </Card>
          <Card title="Rates">
            <Stat label="Last UL Rate" value={`${summary.lastUplinkRateBps || 0} bps`} mono />
            <Stat label="Last DL Rate" value={`${summary.lastDownlinkRateBps || 0} bps`} mono />
            <Stat label="Created" value={formatTimestamp(summary.createdAtMs)} mono />
            <Stat label="Age" value={formatAge(summary.createdAtMs)} mono />
          </Card>
        </div>
      )}

      {activeTab === "rlp" && (
        <div className="grid grid-cols-1 lg:grid-cols-2 gap-4">
          <Card title={isHrpd ? "Bearer State" : "Handshake"}>
            <Stat label={isHrpd ? "A10 State" : "RLP State"} value={formatStateLabel(summary.rlpState)} />
            <Stat label="Last RX Control" value={detail.lastRxControl || "-"} mono />
            <Stat label="Last TX Control" value={detail.lastTxControl || "-"} mono />
            <Stat label="RX Repeats" value={String(detail.lastRxControlRepeats || 0)} mono />
            <Stat label="TX Repeats" value={String(detail.lastTxControlRepeats || 0)} mono />
          </Card>
          <Card title="Rates">
            <Stat label="Last UL Rate" value={`${summary.lastUplinkRateBps || 0} bps`} mono />
            <Stat label="Last DL Rate" value={`${summary.lastDownlinkRateBps || 0} bps`} mono />
            <Stat label="UL Frames" value={summary.uplinkFrames.toLocaleString()} mono />
            <Stat label="DL Frames" value={summary.downlinkFrames.toLocaleString()} mono />
          </Card>
        </div>
      )}

      {activeTab === "ppp" && (
        <Card title="PPP Negotiation">
          <div className="mb-4 grid grid-cols-1 md:grid-cols-3 gap-4">
            <div className="rounded border border-border bg-surface p-3">
              <div className="text-xs uppercase tracking-wide text-muted">LCP</div>
              <div className="mt-1 text-sm text-primary">{formatStateLabel(summary.lcpState)}</div>
            </div>
            <div className="rounded border border-border bg-surface p-3">
              <div className="text-xs uppercase tracking-wide text-muted">IPCP</div>
              <div className="mt-1 text-sm text-primary">{formatStateLabel(summary.ipcpState)}</div>
            </div>
            <div className="rounded border border-border bg-surface p-3">
              <div className="text-xs uppercase tracking-wide text-muted">Negotiated IPs</div>
              <div className="mt-1 text-sm font-mono text-primary">
                {summary.peerIp || "-"} / {summary.ourIp || "-"}
              </div>
            </div>
          </div>
          <TraceList
            events={detail.recentPppEvents || []}
            emptyText="No recent PPP control-plane activity recorded."
          />
        </Card>
      )}

      {activeTab === "capture" && (
        <Card title="IP Capture">
          <div className="mb-4 flex items-center justify-between gap-4">
            <p className="text-sm text-muted">
              Capture records mobile IP frames that traverse this session while capture is enabled.
            </p>
            <span
              className={`rounded px-2 py-1 text-xs ${
                summary.captureEnabled
                  ? "bg-accent-cyan/10 text-accent-cyan border border-accent-cyan/20"
                  : "bg-surface-raised text-muted"
              }`}
            >
              {summary.captureEnabled ? "Recording" : "Idle"}
            </span>
          </div>
          <TraceList
            events={detail.captureEvents || []}
            emptyText={
              summary.captureEnabled
                ? "Capture is running but no IP frames have been recorded yet."
                : "Capture is disabled."
            }
          />
        </Card>
      )}
    </div>
  );
}
