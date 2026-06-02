"use client";

import { useEffect, useState, useCallback, useRef, use, useMemo, type FormEvent } from "react";
import Link from "next/link";
import { useRouter } from "next/navigation";
import { Card, Stat } from "@/components/card";
import { RecentMessagesCard } from "@/components/recent-messages-card";
import { RecentOtaspCard } from "@/components/recent-otasp-card";
import { TimeSeriesChart, type Series } from "@/components/time-series-chart";
import { esnManufacturer } from "@/lib/esn-manufacturer";
import {
  AccessDetail,
  PagingDetail,
  TrafficDetail,
  formatAccessSummary,
  formatAccessChannel,
  shouldHideAccessEvent,
  formatPagingSummary,
  formatTrafficChannel,
  formatTrafficSummary,
} from "@/components/message-detail";
import {
  fingerprintAccessEvent,
  fingerprintPagingEvent,
  fingerprintTrafficEvent,
} from "@/lib/event-fingerprint";
import { useEventStream } from "@/lib/use-event-stream";
import { type LogEntry, makeLogEntryId, makeSortKey, sortLogEntries, formatTime } from "@/lib/message-log";
import { formatEsn, formatMeid } from "@/lib/format";
import type { AccessEvent, PagingEvent, TrafficEvent } from "@/lib/proto/bsc/v1/service";
import type { SubscriberIdentity } from "@/lib/proto/hlr/v1/service";

interface MobileInfo {
  address: string;
  pageAddress: string;
  state: string;
  mobPRev: number;
  esn?: number;
  imsi?: string;
  meid?: string;
  pgslot?: number;
  slotCycleIndex: number;
  snrDb?: number;
  signalPowerDb?: number;
  demodQualityPct?: number;
  rxPowerDbm?: number;
  rxLevelDbfs?: number;
  trafficPower?: {
    targetEbNtDb: number;
    effectiveTargetEbNtDb: number;
    manualTargetOverrideDb?: number;
    lastPcgSnrDb: number[];
    lastActivePcgMask?: boolean[];
    lastPcbs: number[];
    reversePilotEcIoDb?: number;
    ferPct: number;
    framesTotal: number;
    framesCrcError: number;
    forwardGainOffsetDb: number;
    forwardLastFerPct: number;
    forwardLastPmrmErrors: number;
    forwardLastPmrmFrames: number;
    forwardPmrmCount: number;
    forwardPilotEcIoDb?: number[];
    lastPcgPilotEcNtDb: number[];
    reverseRadioConfig: number;
    powerHistory?: {
      timestampMs: number;
      measuredEbNtDb: number;
      targetEbNtDb: number;
      forwardGainDb: number;
      ferPct: number;
    }[];
  };
  lastHeardMs?: number;
  phoneNumber?: string;
  subscriberId?: string;
  trafficWalshCode?: number;
  trafficServiceOption?: number;
  voiceCallState?: string;
}

interface SmsResult {
  accepted: boolean;
  message: string;
}

interface UpsertSubscriberResponse {
  subscriber?: {
    subscriberId: string;
  };
  error?: string;
}

interface PacketSessionInfo {
  sessionId: string;
  phase: string;
  serviceOption: number;
  peerIp: string;
  ourIp: string;
  tunDevice: string;
  lastActivityAtMs: number;
  mobileAddress: string;
  subscriberId: string;
  phoneNumber: string;
  trafficWalshCode: number;
  captureEnabled: boolean;
}

interface SubscriberDetail {
  identities: SubscriberIdentity[];
  error?: string;
}

const MAX_MESSAGES = 200;

function countActivePcgs(pcgSnrDb: number[], activePcgMask?: boolean[]): number {
  if (activePcgMask && activePcgMask.length === pcgSnrDb.length) {
    return activePcgMask.filter(Boolean).length;
  }
  if (pcgSnrDb.length === 0) return 0;
  const max = Math.max(...pcgSnrDb);
  return pcgSnrDb.filter((v) => v >= max - 10).length;
}

function getGatedCutoffDb(pcgSnrDb: number[], activePcgMask?: boolean[]): number | null {
  if (activePcgMask && activePcgMask.length === pcgSnrDb.length) return null;
  if (pcgSnrDb.length === 0) return null;
  return Math.max(...pcgSnrDb) - 10;
}

function getFrameMetricPcgIndices(
  pcgSnrDb: number[],
  activePcgMask?: boolean[],
): number[] {
  if (pcgSnrDb.length === 0) return [];
  const fallbackCutoff = Math.max(...pcgSnrDb) - 10;
  const candidateIndices =
    activePcgMask && activePcgMask.length === pcgSnrDb.length
      ? activePcgMask.flatMap((active, index) => (active ? [index] : []))
      : pcgSnrDb.flatMap((db, index) => (db >= fallbackCutoff ? [index] : []));
  if (candidateIndices.length === 0) return [];
  const max = candidateIndices
    .map((index) => pcgSnrDb[index])
    .reduce((best, current) => Math.max(best, current), Number.NEGATIVE_INFINITY);
  return candidateIndices.filter((index) => pcgSnrDb[index] === max);
}

function getFrameMetricDb(
  pcgSnrDb: number[],
  activePcgMask?: boolean[],
): number | null {
  const maxIndices = getFrameMetricPcgIndices(pcgSnrDb, activePcgMask);
  if (maxIndices.length === 0) return null;
  return pcgSnrDb[maxIndices[0]];
}

function isActivePcg(
  index: number,
  db: number,
  gatedCutoff: number | null,
  activePcgMask?: boolean[],
): boolean {
  if (activePcgMask && index < activePcgMask.length) {
    return activePcgMask[index];
  }
  if (gatedCutoff == null) return false;
  return db >= gatedCutoff;
}

function formatMobileTitle(mobile: MobileInfo): string {
  if (mobile.phoneNumber) return mobile.phoneNumber;
  if (mobile.esn != null) return `ESN ${formatEsn(mobile.esn)}`;
  if (mobile.meid) return `MEID ${formatMeid(mobile.meid)}`;
  if (mobile.imsi) return `IMSI ${mobile.imsi}`;
  return mobile.address;
}

function defaultSubscriberName(mobile: MobileInfo): string {
  if (mobile.esn != null) return `Mobile ${formatEsn(mobile.esn)}`;
  if (mobile.meid) return `Mobile MEID ${formatMeid(mobile.meid)}`;
  if (mobile.imsi) return `Mobile IMSI ${mobile.imsi}`;
  return "Mobile Station";
}

function addressMatchesMobile(
  pagingAddr: PagingEvent["header"],
  mobile: MobileInfo
): boolean {
  if (!pagingAddr) return false;
  if (pagingAddr.resolvedAddress && pagingAddr.resolvedAddress === mobile.address) return true;
  if (pagingAddr.address && mobile.esn != null && pagingAddr.address.esn === mobile.esn) return true;
  return false;
}

function accessMatchesMobile(event: AccessEvent, mobile: MobileInfo): boolean {
  if (mobile.subscriberId && event.subscriberId === mobile.subscriberId) return true;
  if (event.resolvedAddress && event.resolvedAddress === mobile.address) return true;
  if (mobile.esn != null && event.esn === mobile.esn) return true;
  if (event.address && event.address === mobile.address) return true;
  if (event.trafficWalshCode != null && mobile.trafficWalshCode === event.trafficWalshCode) return true;
  return false;
}

export default function MobileDetailPage({
  params,
}: {
  params: Promise<{ id: string }>;
}) {
  const { id } = use(params);
  const address = decodeURIComponent(id);
  const router = useRouter();

  const [mobile, setMobile] = useState<MobileInfo | null>(null);
  const [loading, setLoading] = useState(true);
  const [messages, setMessages] = useState<LogEntry[]>([]);
  const [expanded, setExpanded] = useState<Set<string>>(new Set());
  const [packetSessions, setPacketSessions] = useState<PacketSessionInfo[]>([]);
  const [subscriberIdentities, setSubscriberIdentities] = useState<SubscriberIdentity[]>([]);
  const [identityTab, setIdentityTab] = useState<"subscriber" | "registration">("registration");

  // SMS form
  const [smsFrom, setSmsFrom] = useState("5551234");
  const [smsText, setSmsText] = useState("");
  const [smsSending, setSmsSending] = useState(false);
  const [smsResult, setSmsResult] = useState<SmsResult | null>(null);
  const [showCreateSubscriber, setShowCreateSubscriber] = useState(false);
  const [createPhoneNumber, setCreatePhoneNumber] = useState("");
  const [createDisplayName, setCreateDisplayName] = useState("");
  const [createError, setCreateError] = useState<string | null>(null);
  const [creatingSubscriber, setCreatingSubscriber] = useState(false);
  const [powerDraft, setPowerDraft] = useState("");
  const [powerOverridePending, setPowerOverridePending] = useState(false);
  const [powerOverrideError, setPowerOverrideError] = useState<string | null>(null);
  const [, setChartTick] = useState(0);
  const powerHistoryRef = useRef<{
    timestamps: number[];
    target: number[];
    measured: number[];
    forwardGain: number[];
    fer: number[];
  }>({ timestamps: [], target: [], measured: [], forwardGain: [], fer: [] });
  const prevWalshRef = useRef<number | undefined>(undefined);
  const lastStatsUpdateRef = useRef<number>(0);

  // Update chart history from a mobile snapshot (called at 500ms intervals).
  const updateChartHistory = useCallback((ms: MobileInfo) => {
    if (ms.trafficWalshCode !== prevWalshRef.current) {
      prevWalshRef.current = ms.trafficWalshCode;
      const h = powerHistoryRef.current;
      h.timestamps.length = 0;
      h.target.length = 0;
      h.measured.length = 0;
      h.forwardGain.length = 0;
      h.fer.length = 0;
    }
    if (ms.trafficPower) {
      const serverHistory = ms.trafficPower.powerHistory ?? [];
      if (serverHistory.length > 0) {
        const h = powerHistoryRef.current;
        h.timestamps = serverHistory.map((s) => Number(s.timestampMs));
        h.target = serverHistory.map((s) => s.targetEbNtDb);
        h.measured = serverHistory.map((s) => s.measuredEbNtDb);
        h.forwardGain = serverHistory.map((s) => s.forwardGainDb);
        h.fer = serverHistory.map((s) => s.ferPct);
      } else {
        const h = powerHistoryRef.current;
        const now = Date.now();
        h.timestamps.push(now);
        h.target.push(ms.trafficPower.effectiveTargetEbNtDb);
        const pilotPcg = ms.trafficPower.lastPcgPilotEcNtDb ?? [];
        const pcg = pilotPcg.length > 0 ? pilotPcg : (ms.trafficPower.lastPcgSnrDb ?? []);
        const valid = pcg.filter((v) => isFinite(v));
        h.measured.push(valid.length > 0 ? valid.reduce((a, b) => a + b, 0) / valid.length : NaN);
        h.forwardGain.push(ms.trafficPower.forwardGainOffsetDb);
        h.fer.push(ms.trafficPower.ferPct);
        const max = 60;
        if (h.timestamps.length > max) {
          const excess = h.timestamps.length - max;
          h.timestamps.splice(0, excess);
          h.target.splice(0, excess);
          h.measured.splice(0, excess);
          h.forwardGain.splice(0, excess);
          h.fer.splice(0, excess);
        }
      }
      setChartTick((t) => t + 1);
    }
  }, []);

  const fetchMobile = useCallback(async () => {
    try {
      const now = Date.now();
      const statsStale = now - lastStatsUpdateRef.current >= 1000;

      const fetches: Promise<Response>[] = [fetch("/api/mobiles")];
      if (statsStale) {
        fetches.push(fetch("/api/packet-sessions"));
      }
      const [mobileRes, packetRes] = await Promise.all(fetches);

      if (mobileRes.ok) {
        const data: MobileInfo[] = await mobileRes.json();
        if (data && !("error" in data)) {
          const ms = data.find((m) => m.address === address);
          if (ms) {
            // Always update chart (fast path)
            updateChartHistory(ms);
            // Only update stats/UI state at 1s intervals
            if (statsStale) {
              setMobile(ms);
              lastStatsUpdateRef.current = now;
            }
          }
        }
      }

      if (statsStale && packetRes?.ok) {
        const packetData = await packetRes.json();
        if (!packetData.error) {
          setPacketSessions(packetData.sessions || []);
        }
      }
    } catch {
      // ignore
    } finally {
      setLoading(false);
    }
  }, [address, updateChartHistory]);

  useEffect(() => {
    fetchMobile();
    const interval = setInterval(fetchMobile, 500);
    return () => clearInterval(interval);
  }, [fetchMobile]);

  useEffect(() => {
    setIdentityTab(mobile?.subscriberId ? "subscriber" : "registration");
  }, [mobile?.subscriberId]);

  useEffect(() => {
    if (!mobile?.subscriberId) {
      setSubscriberIdentities([]);
      return;
    }

    let cancelled = false;
    const fetchSubscriberIdentities = async () => {
      try {
        const res = await fetch(`/api/subscribers/${encodeURIComponent(mobile.subscriberId!)}`);
        const data: SubscriberDetail = await res.json();
        if (!cancelled && res.ok && !data.error) {
          setSubscriberIdentities(data.identities || []);
        }
      } catch {
        if (!cancelled) {
          setSubscriberIdentities([]);
        }
      }
    };

    void fetchSubscriberIdentities();
    return () => {
      cancelled = true;
    };
  }, [mobile?.subscriberId]);

  useEffect(() => {
    const effectiveTarget = mobile?.trafficPower?.effectiveTargetEbNtDb;
    if (effectiveTarget != null) {
      setPowerDraft((current) => (current === "" ? effectiveTarget.toFixed(1) : current));
    } else {
      setPowerDraft("");
    }
  }, [mobile?.trafficPower?.effectiveTargetEbNtDb, mobile?.trafficWalshCode]);

  const toggleExpand = useCallback((id: string) => {
    setExpanded((prev) => {
      const next = new Set(prev);
      if (next.has(id)) next.delete(id);
      else next.add(id);
      return next;
    });
  }, []);

  const addMessage = useCallback((entry: LogEntry) => {
    setMessages((prev) => {
      const existingIndex = prev.findIndex((candidate) => candidate.identity === entry.identity);
      if (existingIndex >= 0) {
        const existing = prev[existingIndex];
        const merged = { ...entry, id: existing.id, seenCount: existing.seenCount + 1 };
        return sortLogEntries([merged, ...prev.slice(0, existingIndex), ...prev.slice(existingIndex + 1)], MAX_MESSAGES);
      }
      return sortLogEntries([{ ...entry, seenCount: 1 }, ...prev], MAX_MESSAGES);
    });
  }, []);

  // Shared SSE via BroadcastChannel, filter to this MS
  useEventStream("paging", useCallback((data: string) => {
    if (!mobile) return;
    const event: PagingEvent = JSON.parse(data);
    if (addressMatchesMobile(event.header, mobile)) {
      const ts = event.timestampUs ?? (Date.now() * 1000);
      addMessage({
        kind: "tx",
        stream: "paging",
        id: makeLogEntryId("tx", ts),
        ts,
        identity: `tx:paging:${event.eventId || fingerprintPagingEvent(event)}`,
        sortKey: makeSortKey(event.eventId, `tx:paging:${event.eventId || fingerprintPagingEvent(event)}`),
        event,
        seenCount: 1,
      });
    }
  }, [addMessage, mobile]));

  useEventStream("traffic", useCallback((data: string) => {
    if (!mobile) return;
    const event: TrafficEvent = JSON.parse(data);
    if (addressMatchesMobile(event.header, mobile)) {
      const ts = event.timestampUs ?? (Date.now() * 1000);
      addMessage({
        kind: "tx",
        stream: "traffic",
        id: makeLogEntryId("tx", ts),
        ts,
        identity: `tx:traffic:${event.eventId || fingerprintTrafficEvent(event)}`,
        sortKey: makeSortKey(event.eventId, `tx:traffic:${event.eventId || fingerprintTrafficEvent(event)}`),
        event,
        seenCount: 1,
      });
    }
  }, [addMessage, mobile]));

  useEventStream("access", useCallback((data: string) => {
    if (!mobile) return;
    const event: AccessEvent = JSON.parse(data);
    if (shouldHideAccessEvent(event)) return;
    if (accessMatchesMobile(event, mobile)) {
      const ts = event.timestampUs ?? (Date.now() * 1000);
      addMessage({
        kind: "rx",
        id: makeLogEntryId("rx", ts),
        ts,
        identity: `rx:access:${event.eventId || fingerprintAccessEvent(event)}`,
        sortKey: makeSortKey(event.eventId, `rx:access:${event.eventId || fingerprintAccessEvent(event)}`),
        event,
        seenCount: 1,
      });
    }
  }, [addMessage, mobile]));

  const sendSms = async () => {
    if (!smsText.trim()) return;
    setSmsSending(true);
    setSmsResult(null);
    try {
      // Prefer subscriber phone number (HLR-resolved). For non-subscriber
      // mobiles fall back to addressing by IMSI directly.
      const destinationNumber = mobile?.phoneNumber || undefined;
      const destinationImsi = destinationNumber ? undefined : mobile?.imsi || undefined;
      const res = await fetch("/api/sms", {
        method: "POST",
        headers: { "Content-Type": "application/json" },
        body: JSON.stringify({
          originatingNumber: smsFrom,
          text: smsText,
          destinationNumber,
          destinationImsi,
        }),
      });
      const data: SmsResult = await res.json();
      setSmsResult(data);
      if (data.accepted) {
        setSmsText("");
        setTimeout(fetchMobile, 1000);
      }
    } catch (err) {
      setSmsResult({
        accepted: false,
        message: err instanceof Error ? err.message : "unknown error",
      });
    } finally {
      setSmsSending(false);
    }
  };

  const openCreateSubscriber = useCallback(() => {
    if (!mobile) return;
    setCreatePhoneNumber("");
    setCreateDisplayName(defaultSubscriberName(mobile));
    setCreateError(null);
    setShowCreateSubscriber(true);
  }, [mobile]);

  const createSubscriberFromMobile = useCallback(async (event: FormEvent) => {
    event.preventDefault();
    if (!mobile) return;

    const phoneNumber = createPhoneNumber.trim();
    const displayName = createDisplayName.trim();
    if (!/^\d+$/.test(phoneNumber)) {
      setCreateError("Phone number must contain at least one digit and only digits");
      return;
    }
    if (!mobile.imsi || (mobile.esn == null && !mobile.meid)) {
      setCreateError("Mobile needs IMSI plus ESN or MEID");
      return;
    }

    setCreatingSubscriber(true);
    setCreateError(null);
    try {
      const res = await fetch("/api/subscribers", {
        method: "POST",
        headers: { "Content-Type": "application/json" },
        body: JSON.stringify({
          phoneNumber,
          displayName: displayName || defaultSubscriberName(mobile),
          status: "active",
          esn: mobile.esn,
          imsi: mobile.imsi,
          meid: mobile.meid,
        }),
      });
      const data: UpsertSubscriberResponse = await res.json();
      if (!res.ok || data.error) {
        throw new Error(data.error || `HTTP ${res.status}`);
      }
      if (data.subscriber?.subscriberId) {
        router.push(`/subscribers/${encodeURIComponent(data.subscriber.subscriberId)}`);
        return;
      }
      await fetchMobile();
      setShowCreateSubscriber(false);
    } catch (err) {
      setCreateError(err instanceof Error ? err.message : "unknown error");
    } finally {
      setCreatingSubscriber(false);
    }
  }, [createDisplayName, createPhoneNumber, fetchMobile, mobile, router]);

  const applyPowerOverride = useCallback(async (payload: { targetDb?: number; clear?: boolean }) => {
    if (mobile?.trafficWalshCode == null) return;

    setPowerOverridePending(true);
    setPowerOverrideError(null);
    try {
      const res = await fetch("/api/channels/power-override", {
        method: "POST",
        headers: { "Content-Type": "application/json" },
        body: JSON.stringify({ walshCode: mobile.trafficWalshCode, ...payload }),
      });
      const json = await res.json();
      if (!res.ok || !json.accepted) {
        throw new Error(json.message || `HTTP ${res.status}`);
      }
      if (json.trafficPower?.effectiveTargetEbNtDb != null) {
        setPowerDraft(json.trafficPower.effectiveTargetEbNtDb.toFixed(1));
      } else if (payload.clear) {
        setPowerDraft("");
      }
      await fetchMobile();
    } catch (err) {
      setPowerOverrideError(err instanceof Error ? err.message : "unknown error");
    } finally {
      setPowerOverridePending(false);
    }
  }, [fetchMobile, mobile?.trafficWalshCode]);

  const pinPowerOverride = useCallback(async () => {
    const targetDb = Number(powerDraft.trim());
    if (!Number.isFinite(targetDb)) {
      setPowerOverrideError("invalid reverse target");
      return;
    }
    await applyPowerOverride({ targetDb });
  }, [applyPowerOverride, powerDraft]);

  const clearPowerOverride = useCallback(async () => {
    await applyPowerOverride({ clear: true });
  }, [applyPowerOverride]);

  const primarySubscriberIdentity = useMemo(
    () =>
      subscriberIdentities.find((identity) => identity.isPrimary) ??
      subscriberIdentities[0],
    [subscriberIdentities]
  );

  if (loading) {
    return (
      <div className="max-w-7xl mx-auto">
        <p className="text-dimmed text-sm">Loading...</p>
      </div>
    );
  }

  if (!mobile) {
    return (
      <div className="max-w-7xl mx-auto space-y-4">
        <Link
          href="/mobiles"
          className="text-sm text-muted hover:text-secondary"
        >
          &larr; Back to Mobiles
        </Link>
        <div className="rounded-lg border border-accent-amber/20 bg-accent-amber-bg p-4 text-accent-amber text-sm">
          Mobile station &quot;{address}&quot; not found.
        </div>
      </div>
    );
  }

  const observedEsnHex = mobile.esn != null ? formatEsn(mobile.esn) : null;
  const observedMeid = mobile.meid ? formatMeid(mobile.meid) : null;
  const provisionedEsnHex =
    primarySubscriberIdentity?.esn != null ? formatEsn(primarySubscriberIdentity.esn) : null;
  const provisionedImsi = primarySubscriberIdentity?.imsi || null;
  const provisionedMeid = primarySubscriberIdentity?.meid ? formatMeid(primarySubscriberIdentity.meid) : null;
  const canCreateSubscriber = !mobile.subscriberId && Boolean(mobile.imsi) && (mobile.esn != null || Boolean(mobile.meid));
  const trafficPower = mobile.trafficPower;
  const isRc3 = (trafficPower?.reverseRadioConfig ?? 0) === 3;
  // Inner-loop metric: pilot SINR (RC3) or Eb/Nt (RC1); falls back to frame snapshot.
  const pilotArr = trafficPower?.lastPcgPilotEcNtDb ?? [];
  const innerLoopPcgDb =
    pilotArr.length > 0
      ? pilotArr
      : trafficPower?.lastPcgSnrDb ?? [];
  // Per-frame data Eb/Nt snapshot (always from decoded frame).
  const frameEbNtDb = trafficPower?.lastPcgSnrDb ?? [];
  const metricLabel = isRc3 ? "Pilot SINR" : "Eb/Nt";
  const reverseTargetDb =
    trafficPower?.effectiveTargetEbNtDb ?? trafficPower?.targetEbNtDb ?? null;
  const reverseFrameMax =
    innerLoopPcgDb.length > 0
      ? getFrameMetricDb(innerLoopPcgDb, trafficPower?.lastActivePcgMask)
      : null;
  const reverseFrameMaxPcgs =
    innerLoopPcgDb.length > 0
      ? getFrameMetricPcgIndices(innerLoopPcgDb, trafficPower?.lastActivePcgMask)
      : [];
  const reverseActivePcgs =
    innerLoopPcgDb.length > 0
      ? countActivePcgs(innerLoopPcgDb, trafficPower?.lastActivePcgMask)
      : null;
  const reverseLifetimeFerPct =
    trafficPower && trafficPower.framesTotal > 0
      ? (100 * trafficPower.framesCrcError) / trafficPower.framesTotal
      : 0;
  const reverseGatedCutoff =
    innerLoopPcgDb.length > 0
      ? getGatedCutoffDb(innerLoopPcgDb, trafficPower?.lastActivePcgMask)
      : null;
  const reversePilotEcIoText =
    trafficPower?.reversePilotEcIoDb != null
      ? `${trafficPower.reversePilotEcIoDb.toFixed(2)} dB`
      : null;
  const forwardPilotEcIoText =
    trafficPower && (trafficPower.forwardPilotEcIoDb ?? []).length > 0
      ? (trafficPower.forwardPilotEcIoDb ?? []).map((db) => `${db.toFixed(1)} dB`).join(", ")
      : trafficPower && trafficPower.forwardPmrmCount > 0
        ? "none reported"
        : "-";

  const mobilePacketSessions = packetSessions.filter((session) => {
    if (session.phase === "closed") return false;
    if (mobile.subscriberId && session.subscriberId === mobile.subscriberId) return true;
    if (session.mobileAddress && session.mobileAddress === mobile.address) return true;
    return false;
  });

  return (
    <div className="max-w-7xl mx-auto space-y-6">
      <div className="flex items-center gap-4">
        <Link
          href="/mobiles"
          className="text-sm text-muted hover:text-secondary"
        >
          &larr; Mobiles
        </Link>
        <div className="min-w-0">
          <h1 className="text-lg font-bold font-mono">{formatMobileTitle(mobile)}</h1>
          <div className="text-xs text-muted font-mono truncate">
            Registered IMSI {mobile.imsi || "Not Available"}
            {observedMeid ? ` / MEID ${observedMeid}` : ""}
          </div>
        </div>
        <span
          className={`text-xs px-2 py-0.5 rounded ${
            mobile.state === "Registered"
              ? "bg-badge-green-bg text-badge-green-text"
              : mobile.state === "Paged"
                ? "bg-badge-yellow-bg text-badge-yellow-text"
                : mobile.state === "TrafficAssigning" || mobile.state === "TrafficActive"
                  ? "bg-badge-purple-bg text-badge-purple-text"
                  : "bg-badge-blue-bg text-badge-blue-text"
          }`}
        >
          {mobile.state}
        </span>
      </div>

      <div className="grid grid-cols-1 md:grid-cols-3 gap-4">
        <Card title="Identity">
          <div className="mb-3 grid grid-cols-2 rounded border border-border bg-surface p-0.5">
            <button
              type="button"
              onClick={() => mobile.subscriberId && setIdentityTab("subscriber")}
              disabled={!mobile.subscriberId}
              className={`rounded px-2 py-1 text-xs transition-colors ${
                identityTab === "subscriber"
                  ? "bg-surface-raised text-secondary"
                  : "text-muted hover:text-secondary disabled:text-dimmed disabled:hover:text-dimmed"
              }`}
            >
              Subscriber
            </button>
            <button
              type="button"
              onClick={() => setIdentityTab("registration")}
              className={`rounded px-2 py-1 text-xs transition-colors ${
                identityTab === "registration"
                  ? "bg-surface-raised text-secondary"
                  : "text-muted hover:text-secondary"
              }`}
            >
              Registration
            </button>
          </div>

          {identityTab === "subscriber" && mobile.subscriberId ? (
            <>
              {mobile.phoneNumber && (
                <Stat label="Phone Number" value={mobile.phoneNumber} mono />
              )}
              <div className="flex justify-between py-0.5">
                <span className="text-muted text-sm">Subscriber</span>
                <Link
                  href={`/subscribers/${encodeURIComponent(mobile.subscriberId)}`}
                  className="text-sm font-mono text-accent-green hover:text-accent-green transition-colors"
                >
                  {mobile.subscriberId.length > 20
                    ? `${mobile.subscriberId.slice(0, 8)}...${mobile.subscriberId.slice(-8)}`
                    : mobile.subscriberId}
                </Link>
              </div>
              <Stat label="Provisioned ESN" value={provisionedEsnHex || "Not Available"} mono />
              <Stat label="Provisioned IMSI" value={provisionedImsi || "Not Available"} mono />
              <Stat label="Provisioned MEID" value={provisionedMeid || "Not Available"} mono />
              {primarySubscriberIdentity?.esn != null && esnManufacturer(primarySubscriberIdentity.esn) && (
                <Stat label="Manufacturer" value={esnManufacturer(primarySubscriberIdentity.esn)!} />
              )}
            </>
          ) : (
            <>
              <Stat label="Observed ESN" value={observedEsnHex || "Not Available"} mono />
              <Stat label="Observed MEID" value={observedMeid || "Not Available"} mono />
              <Stat label="Registered IMSI" value={mobile.imsi || "Not Available"} mono />
              <Stat label="MOB_P_REV" value={String(mobile.mobPRev)} />
              <Stat
                label="PGSLOT"
                value={mobile.pgslot != null ? String(mobile.pgslot) : "-"}
                mono
              />
              <Stat label="SLOT_CYCLE_INDEX" value={String(mobile.slotCycleIndex)} />
              {!mobile.subscriberId && (
                <div className="mt-3 border-t border-border pt-3">
                  {!showCreateSubscriber ? (
                    <button
                      type="button"
                      onClick={openCreateSubscriber}
                      disabled={!canCreateSubscriber}
                      className="text-xs px-3 py-1.5 rounded bg-accent-green-bg text-accent-green border border-accent-green/20 hover:bg-accent-green/15 disabled:opacity-50 disabled:hover:bg-accent-green-bg transition-colors"
                    >
                      Create Subscriber
                    </button>
                  ) : (
                    <form onSubmit={createSubscriberFromMobile} className="space-y-3">
                      <div>
                        <label className="block text-xs text-muted mb-1">Phone Number</label>
                        <input
                          type="text"
                          value={createPhoneNumber}
                          onChange={(e) => setCreatePhoneNumber(e.target.value)}
                          placeholder="5551234567"
                          required
                          className="w-full glass-input font-mono"
                        />
                      </div>
                      <div>
                        <label className="block text-xs text-muted mb-1">Display Name</label>
                        <input
                          type="text"
                          value={createDisplayName}
                          onChange={(e) => setCreateDisplayName(e.target.value)}
                          className="w-full glass-input"
                        />
                      </div>
                      {createError && (
                        <p className="text-accent-red text-xs">{createError}</p>
                      )}
                      <div className="flex items-center gap-2">
                        <button
                          type="submit"
                          disabled={creatingSubscriber}
                          className="text-xs px-3 py-1.5 rounded bg-accent-green-bg text-accent-green border border-accent-green/20 hover:bg-accent-green/15 disabled:opacity-50 transition-colors"
                        >
                          {creatingSubscriber ? "Creating..." : "Create"}
                        </button>
                        <button
                          type="button"
                          onClick={() => {
                            setShowCreateSubscriber(false);
                            setCreateError(null);
                          }}
                          disabled={creatingSubscriber}
                          className="text-xs px-3 py-1.5 rounded bg-surface-raised text-muted border border-border hover:text-secondary disabled:opacity-50 transition-colors"
                        >
                          Cancel
                        </button>
                      </div>
                    </form>
                  )}
                </div>
              )}
            </>
          )}
        </Card>

        <Card title="Signal Quality">
          <Stat
            label="SNR"
            value={mobile.snrDb != null ? `${mobile.snrDb.toFixed(1)} dB` : "-"}
            mono
          />
          <Stat
            label="Rx Level"
            value={
              mobile.rxPowerDbm != null
                ? `${mobile.rxPowerDbm.toFixed(1)} dBm`
                : mobile.rxLevelDbfs != null
                  ? `${mobile.rxLevelDbfs.toFixed(1)} dBFS`
                  : mobile.signalPowerDb != null
                    ? `${mobile.signalPowerDb.toFixed(1)} dB`
                    : "-"
            }
            mono
          />
          <Stat
            label="Demod Quality"
            value={
              mobile.demodQualityPct != null
                ? `${mobile.demodQualityPct.toFixed(0)}%`
                : "-"
            }
            mono
          />
          <Stat
            label="Last Heard"
            value={mobile.lastHeardMs ? formatTime(mobile.lastHeardMs * 1000) : "-"}
            mono
          />
        </Card>

        <Card title="Send SMS">
          <div className="space-y-3">
            <div>
              <label className="block text-xs text-muted mb-1">
                Originating Number
              </label>
              <input
                type="text"
                value={smsFrom}
                onChange={(e) => setSmsFrom(e.target.value)}
                className="w-full glass-input"
              />
            </div>
            <div>
              <label className="block text-xs text-muted mb-1">
                Message
              </label>
              <textarea
                value={smsText}
                onChange={(e) => setSmsText(e.target.value)}
                rows={3}
                className="w-full glass-input resize-none"
                placeholder="Enter SMS text..."
              />
            </div>
            <div className="flex items-center gap-3">
              <button
                onClick={sendSms}
                disabled={smsSending || !smsText.trim()}
                className="px-4 py-1.5 bg-accent-green-bg text-accent-green border border-accent-green/20 hover:bg-accent-green/15 disabled:bg-surface-raised disabled:text-muted text-sm rounded transition-colors"
              >
                {smsSending ? "Sending..." : "Send SMS"}
              </button>
              {smsResult && (
                <span
                  className={`text-xs ${
                    smsResult.accepted ? "text-accent-green" : "text-accent-red"
                  }`}
                >
                  {smsResult.message}
                </span>
              )}
            </div>
          </div>
        </Card>
      </div>

      <RecentMessagesCard phone={mobile.phoneNumber} />

      <RecentOtaspCard esn={mobile.esn} meid={mobile.meid} />

      {mobilePacketSessions.length > 0 && (
        <Card title={`Active Sessions (${mobilePacketSessions.length})`}>
          <div className="space-y-3">
            {mobilePacketSessions.map((session) => (
              <div
                key={session.sessionId}
                className="rounded-lg border border-border bg-surface p-4"
              >
                <div className="flex flex-wrap items-start justify-between gap-3">
                  <div className="space-y-1">
                    <Link
                      href={`/packets/${encodeURIComponent(session.sessionId)}`}
                      className="font-mono text-sm text-accent-green hover:text-accent-green"
                    >
                      {session.sessionId}
                    </Link>
                    <div className="text-xs text-muted">
                      SO{session.serviceOption}
                      {session.trafficWalshCode ? ` · W${session.trafficWalshCode}` : ""}
                      {session.captureEnabled ? " · capture enabled" : ""}
                    </div>
                  </div>
                  <span className="rounded bg-surface-raised px-2 py-0.5 text-xs text-primary">
                    {session.phase}
                  </span>
                </div>
                <div className="mt-3 grid gap-2 sm:grid-cols-2 xl:grid-cols-4 text-sm">
                  <div className="text-muted">
                    Mobile IP
                    <div className="font-mono text-primary">{session.peerIp || "-"}</div>
                  </div>
                  <div className="text-muted">
                    Gateway IP
                    <div className="font-mono text-primary">{session.ourIp || "-"}</div>
                  </div>
                  <div className="text-muted">
                    TUN
                    <div className="font-mono text-primary">{session.tunDevice || "-"}</div>
                  </div>
                  <div className="text-muted">
                    Last Activity
                    <div className="font-mono text-primary">
                      {session.lastActivityAtMs ? formatTime(session.lastActivityAtMs * 1000) : "-"}
                    </div>
                  </div>
                </div>
              </div>
            ))}
          </div>
        </Card>
      )}

      {mobile.trafficWalshCode != null && (
        <Card title="Traffic Channel">
          <div className="space-y-5">
            <div className="rounded-lg border border-border bg-surface p-4">
              <div className="grid gap-3 sm:grid-cols-2 xl:grid-cols-4">
                <div className="rounded-md border border-border bg-surface px-3 py-2">
                  <div className="text-[11px] uppercase tracking-wide text-muted">Walsh Code</div>
                  <div className="mt-1 font-mono text-lg text-primary">W{mobile.trafficWalshCode}</div>
                </div>
                <div className="rounded-md border border-border bg-surface px-3 py-2">
                  <div className="text-[11px] uppercase tracking-wide text-muted">Service Option</div>
                  <div className="mt-1 font-mono text-sm text-primary">
                    {mobile.trafficServiceOption != null
                      ? `SO${mobile.trafficServiceOption}${
                          mobile.trafficServiceOption === 6 ? " (SMS)" :
                          mobile.trafficServiceOption === 3 ? " (EVRC)" :
                          mobile.trafficServiceOption === 17 ? " (EVRC)" :
                          mobile.trafficServiceOption === 68 ? " (EVRC-B)" :
                          mobile.trafficServiceOption === 73 ? " (EVRC-NW)" : ""
                        }`
                      : "Unknown"}
                  </div>
                </div>
                <div className="rounded-md border border-border bg-surface px-3 py-2">
                  <div className="text-[11px] uppercase tracking-wide text-muted">Channel State</div>
                  <div className="mt-1 text-sm font-medium text-primary">{mobile.state}</div>
                </div>
                <div className="rounded-md border border-border bg-surface px-3 py-2">
                  <div className="text-[11px] uppercase tracking-wide text-muted">Voice</div>
                  <div className="mt-1 text-sm font-medium text-primary">
                    {mobile.voiceCallState ?? "No active voice state"}
                  </div>
                </div>
              </div>
            </div>

            {trafficPower ? (
              <>
              {/* Power control time-series chart */}
              {powerHistoryRef.current.timestamps.length >= 2 && (() => {
                const h = powerHistoryRef.current;
                const chartSeries: Series[] = [
                  { key: "target", label: `Target ${metricLabel}`, color: "#f59e0b", values: h.target, dashed: true },
                  { key: "measured", label: `Measured ${metricLabel}`, color: "#22c55e", values: h.measured },
                  { key: "fwd_gain", label: "Fwd Gain", color: "#818cf8", values: h.forwardGain },
                ];
                return (
                  <section className="rounded-lg border border-border bg-surface p-4 mb-4">
                    <div className="mb-3">
                      <div className="text-sm font-medium text-primary">Power Control — Live {isRc3 ? "(RC3)" : "(RC1)"}</div>
                      <div className="text-xs text-muted">
                        {Math.round((h.timestamps[h.timestamps.length - 1] - h.timestamps[0]) / 1000)}s window · 100ms granularity · Target (dashed) vs Measured {metricLabel} · Forward gain offset
                      </div>
                    </div>
                    <div>
                      <TimeSeriesChart
                        width={900}
                        height={300}
                        yLabel="dB"
                        timestamps={h.timestamps}
                        series={chartSeries}
                      />
                    </div>
                    <div className="mt-2 flex justify-center gap-6 text-[11px] font-mono text-muted">
                      <span>Rev FER window <span className={trafficPower.ferPct > 1 ? "text-accent-red" : "text-accent-green"}>{trafficPower.ferPct.toFixed(1)}%</span></span>
                      <span>Frames {trafficPower.framesTotal.toLocaleString()}</span>
                      <span>Fwd FER <span className={trafficPower.forwardLastFerPct > 1 ? "text-accent-red" : "text-accent-green"}>{trafficPower.forwardLastFerPct.toFixed(1)}%</span></span>
                      <span>PMRMs {trafficPower.forwardPmrmCount}</span>
                      {trafficPower.forwardPilotEcIoDb && trafficPower.forwardPilotEcIoDb.length > 0 && (
                        <span>Pilot Ec/Io {trafficPower.forwardPilotEcIoDb[0].toFixed(1)} dB</span>
                      )}
                    </div>
                  </section>
                );
              })()}

              <div className="grid gap-4 xl:grid-cols-2">
                <section className="rounded-lg border border-border bg-surface p-4">
                  <div className="mb-4">
                    <div className="text-sm font-medium text-primary">Reverse Power Control {isRc3 ? "(RC3 — Pilot SINR)" : "(RC1 — Data Eb/Nt)"}</div>
                    <div className="text-xs text-muted">
                      Closed-loop reverse traffic measurements, live per-PCG PCB scheduling, and bad-frame accounting.
                    </div>
                  </div>

                  <div className="grid gap-3 sm:grid-cols-2 xl:grid-cols-3">
                    <div className="rounded-md border border-border bg-surface px-3 py-2">
                      <div className="text-[11px] uppercase tracking-wide text-muted">Effective Target {metricLabel}</div>
                      <div className="mt-1 font-mono text-base text-primary">
                        {trafficPower.effectiveTargetEbNtDb.toFixed(2)} dB
                      </div>
                      <div className="text-[11px] text-muted">
                        auto {trafficPower.targetEbNtDb.toFixed(2)} dB
                      </div>
                    </div>
                    <div className="rounded-md border border-border bg-surface px-3 py-2">
                      <div className="text-[11px] uppercase tracking-wide text-muted">Recent Peak</div>
                      <div className="mt-1 font-mono text-base text-primary">
                        {reverseFrameMax != null ? `${reverseFrameMax.toFixed(2)} dB` : "-"}
                      </div>
                      <div className="text-[11px] text-muted">
                        {reverseFrameMaxPcgs.length === 0
                          ? "latest 20 ms snapshot"
                          : reverseFrameMaxPcgs.length === 1
                            ? `latest 20 ms snapshot, PCG ${reverseFrameMaxPcgs[0]}`
                            : `latest 20 ms snapshot, PCGs ${reverseFrameMaxPcgs.join(", ")}`}
                      </div>
                    </div>
                    {reversePilotEcIoText ? (
                      <div className="rounded-md border border-border bg-surface px-3 py-2">
                        <div className="text-[11px] uppercase tracking-wide text-muted">Pilot Ec/Io (legacy)</div>
                        <div className="mt-1 font-mono text-base text-primary">
                          {reversePilotEcIoText}
                        </div>
                        <div className="text-[11px] text-muted">1 s smoothed reverse traffic finger — diagnostic only</div>
                      </div>
                    ) : null}
                    <div className="rounded-md border border-border bg-surface px-3 py-2">
                      <div className="text-[11px] uppercase tracking-wide text-muted">Active PCGs</div>
                      <div className="mt-1 font-mono text-base text-primary">
                        {reverseActivePcgs != null ? `${reverseActivePcgs}/16` : "-"}
                      </div>
                      <div className="text-[11px] text-muted">
                        {trafficPower.lastActivePcgMask && trafficPower.lastActivePcgMask.length === 16
                          ? "exact reverse transmit mask"
                          : reverseGatedCutoff == null
                          ? "within 10 dB of frame max"
                          : reverseGatedCutoff <= 0
                            ? `cutoff ${reverseGatedCutoff.toFixed(2)} dB, all PCGs count active`
                            : `cutoff ${reverseGatedCutoff.toFixed(2)} dB`}
                      </div>
                    </div>
                    <div className="rounded-md border border-border bg-surface px-3 py-2">
                      <div className="text-[11px] uppercase tracking-wide text-muted">Reverse FER Window</div>
                      <div className="mt-1 font-mono text-base text-primary">
                        {trafficPower.ferPct.toFixed(2)}%
                      </div>
                      <div className="text-[11px] text-muted">outer-loop sliding window</div>
                    </div>
                    <div className="rounded-md border border-border bg-surface px-3 py-2">
                      <div className="text-[11px] uppercase tracking-wide text-muted">Reverse Frames</div>
                      <div className="mt-1 font-mono text-base text-primary">
                        {trafficPower.framesTotal}
                      </div>
                      <div className="text-[11px] text-muted">lifetime FER {reverseLifetimeFerPct.toFixed(2)}%</div>
                    </div>
                    <div className="rounded-md border border-border bg-surface px-3 py-2">
                      <div className="text-[11px] uppercase tracking-wide text-muted">Bad Frames</div>
                      <div className="mt-1 font-mono text-base text-primary">
                        {trafficPower.framesCrcError}
                      </div>
                      <div className="text-[11px] text-muted">lifetime bad FQI / CRC</div>
                    </div>
                  </div>

                  <div className="mt-4 rounded-md border border-border bg-surface px-3 py-3">
                    <div className="flex flex-wrap items-center justify-between gap-3">
                      <div>
                        <div className="text-[11px] uppercase tracking-wide text-muted">Manual Target Override</div>
                        <div className="mt-1 text-sm text-primary">
                          {trafficPower.manualTargetOverrideDb != null
                            ? `Pinned at ${trafficPower.manualTargetOverrideDb.toFixed(2)} dB`
                            : "Automatic outer loop"}
                        </div>
                      </div>
                      <span className={`inline-flex rounded px-2 py-0.5 text-[11px] ${
                        trafficPower.manualTargetOverrideDb != null
                          ? "bg-badge-orange-bg text-badge-orange-text"
                          : "bg-surface-raised text-muted"
                      }`}>
                        {trafficPower.manualTargetOverrideDb != null ? "Pinned" : "Auto"}
                      </span>
                    </div>
                    <div className="mt-3 flex flex-wrap items-center gap-2">
                      <input
                        type="number"
                        step="0.1"
                        min={isRc3 ? "-20" : "0"}
                        max={isRc3 ? "40" : "40"}
                        inputMode="decimal"
                        value={powerDraft}
                        onChange={(e) => setPowerDraft(e.target.value)}
                        disabled={powerOverridePending}
                        className="w-28 rounded border border-border-input bg-body px-2 py-1 text-right font-mono text-sm text-primary disabled:opacity-50"
                      />
                      <button
                        type="button"
                        onClick={pinPowerOverride}
                        disabled={powerOverridePending || mobile.trafficWalshCode == null}
                        className="rounded border border-accent-green/30 px-3 py-1 text-xs text-accent-green hover:bg-accent-green-bg disabled:opacity-50"
                      >
                        {powerOverridePending ? "Applying..." : "Pin"}
                      </button>
                      <button
                        type="button"
                        onClick={clearPowerOverride}
                        disabled={powerOverridePending || trafficPower.manualTargetOverrideDb == null}
                        className="rounded border border-border-input px-3 py-1 text-xs text-secondary hover:bg-surface-raised disabled:opacity-50"
                      >
                        Clear
                      </button>
                    </div>
                    {powerOverrideError && (
                      <div className="mt-2 text-xs text-accent-red">{powerOverrideError}</div>
                    )}
                  </div>

                  <div className="mt-4">
                    <div className="mb-2 flex items-center justify-between gap-3">
                      <div className="text-xs uppercase tracking-wide text-muted">
                        Latest Per-PCG {metricLabel}
                      </div>
                      <div className="text-[11px] text-muted">
                        ● = active, ○ = inactive, ▲ = last scheduled UP, ▼ = last scheduled DOWN
                      </div>
                    </div>
                    <div className="mb-2 text-[11px] text-muted">
                      {isRc3
                        ? "Showing pilot SINR (inner-loop control metric). Frame data Eb/Nt shown below each value."
                        : "Peak highlighting is diagnostic only."}{" "}
                      Reverse closed-loop decisions are scheduled one PCG at a time on the absolute-PCG timeline.
                    </div>
                    <div className="grid gap-2 sm:grid-cols-2 lg:grid-cols-4">
                      {innerLoopPcgDb.map((db, i) => {
                        const pcb = trafficPower.lastPcbs[i];
                        const isFrameMax = reverseFrameMaxPcgs.includes(i);
                        const active = isActivePcg(
                          i,
                          db,
                          reverseGatedCutoff,
                          trafficPower.lastActivePcgMask,
                        );
                        const frameDb = frameEbNtDb[i];
                        const tone = !active
                          ? "border-border-subtle bg-surface text-muted opacity-70"
                          : reverseTargetDb != null && db >= reverseTargetDb
                            ? "border-accent-green/20 bg-accent-green-bg text-accent-green"
                            : reverseTargetDb != null && db >= reverseTargetDb - 3
                              ? "border-accent-amber/20 bg-accent-amber-bg text-accent-amber"
                              : "border-accent-red/20 bg-accent-red-bg text-accent-red";
                        return (
                          <div
                            key={i}
                            className={`rounded-md border px-3 py-2 ${tone} ${
                              isFrameMax ? "ring-1 ring-sky-500/60" : ""
                            }`}
                            title={
                              isFrameMax
                                ? active
                                  ? pcb === 0
                                    ? "Recent-peak PCG, last scheduled UP (PCB=0)"
                                    : "Recent-peak PCG, last scheduled DOWN (PCB=1)"
                                  : "Recent-peak PCG, inactive"
                                : active
                                ? pcb === 0
                                  ? "Last scheduled UP (PCB=0)"
                                  : "Last scheduled DOWN (PCB=1)"
                                : "Inactive PCG"
                            }
                          >
                            <div className="flex items-center justify-between gap-3 text-[11px] uppercase tracking-wide">
                              <div className="flex items-center gap-2">
                                <span
                                  className={active ? "text-accent-green" : "text-muted"}
                                  title={active ? "Active PCG" : "Inactive PCG"}
                                >
                                  {active ? "●" : "○"}
                                </span>
                                <span className="text-muted">PCG {i}</span>
                              </div>
                              <div className="flex items-center gap-2">
                                {isFrameMax && (
                                  <span
                                    className="inline-flex items-center gap-1 rounded-full border border-sky-500/40 bg-sky-500/10 px-1.5 py-0.5 text-[10px] font-semibold text-sky-300"
                                    title="Recent peak in the latest 20 ms measurement snapshot"
                                  >
                                    <svg
                                      viewBox="0 0 20 20"
                                      fill="currentColor"
                                      className="h-3 w-3"
                                      aria-hidden="true"
                                    >
                                      <path d="M10 2.5l2.1 4.26 4.7.68-3.4 3.31.8 4.68L10 13.2l-4.2 2.23.8-4.68-3.4-3.31 4.7-.68L10 2.5z" />
                                    </svg>
                                    PEAK
                                  </span>
                                )}
                                {active ? (
                                  <span className={pcb === 0 ? "text-accent-green" : "text-accent-red"}>
                                    {pcb === 0 ? "▲" : "▼"}
                                  </span>
                                ) : (
                                  <span className="text-dimmed"> </span>
                                )}
                              </div>
                            </div>
                            <div className="mt-1 font-mono text-sm">{db != null && isFinite(db) ? `${db.toFixed(2)} dB` : "---"}</div>
                            {isRc3 && active && frameDb != null && isFinite(frameDb) && (
                              <div className="mt-0.5 font-mono text-[10px] text-muted" title="Per-frame data Eb/Nt from decoded traffic">
                                Eb/Nt {frameDb.toFixed(1)}
                              </div>
                            )}
                          </div>
                        );
                      })}
                    </div>
                  </div>
                </section>

                <section className="rounded-lg border border-border bg-surface p-4">
                  <div className="mb-4">
                    <div className="text-sm font-medium text-primary">Forward Power Control</div>
                    <div className="text-xs text-muted">
                      PMRM-reported forward FER and the current F-FCH gain walk for this channel.
                    </div>
                  </div>

                  <div className="grid gap-3 sm:grid-cols-2">
                    <div className="rounded-md border border-border bg-surface px-3 py-2">
                      <div className="text-[11px] uppercase tracking-wide text-muted">Gain Offset</div>
                      <div className="mt-1 font-mono text-base text-primary">
                        {trafficPower.forwardGainOffsetDb >= 0 ? "+" : ""}
                        {trafficPower.forwardGainOffsetDb.toFixed(2)} dB
                      </div>
                    </div>
                    <div className="rounded-md border border-border bg-surface px-3 py-2">
                      <div className="text-[11px] uppercase tracking-wide text-muted">Forward FER</div>
                      <div className="mt-1 font-mono text-base text-primary">
                        {trafficPower.forwardPmrmCount > 0
                          ? `${trafficPower.forwardLastFerPct.toFixed(2)}%`
                          : "no PMRM yet"}
                      </div>
                    </div>
                    <div className="rounded-md border border-border bg-surface px-3 py-2">
                      <div className="text-[11px] uppercase tracking-wide text-muted">Last PMRM</div>
                      <div className="mt-1 font-mono text-base text-primary">
                        {trafficPower.forwardPmrmCount > 0
                          ? `${trafficPower.forwardLastPmrmErrors} / ${trafficPower.forwardLastPmrmFrames}`
                          : "-"}
                      </div>
                      <div className="text-[11px] text-muted">errors / frames</div>
                    </div>
                    <div className="rounded-md border border-border bg-surface px-3 py-2">
                      <div className="text-[11px] uppercase tracking-wide text-muted">PMRMs</div>
                      <div className="mt-1 font-mono text-base text-primary">
                        {trafficPower.forwardPmrmCount}
                      </div>
                      <div className="text-[11px] text-muted">since allocation</div>
                    </div>
                  </div>

                  <div className="mt-4 rounded-md border border-border bg-surface px-3 py-3">
                    <div className="text-[11px] uppercase tracking-wide text-muted">
                      Active Set Pilots (Ec/Io)
                    </div>
                    <div className="mt-1 font-mono text-sm text-primary">
                      {forwardPilotEcIoText}
                    </div>
                  </div>

                  <p className="mt-4 text-xs leading-5 text-muted">
                    Reverse FER now reflects the same bad-frame accounting the BSC uses for the
                    reverse outer loop. Forward FER comes from PMRM reports sent by the mobile,
                    while the gain offset shows how far the BTS has walked the forward traffic
                    channel from its initial allocation.
                  </p>
                </section>
              </div>
              </>
            ) : (
              <div className="rounded-lg border border-dashed border-border bg-surface px-4 py-3 text-sm text-muted">
                Traffic channel assigned, but no power-control snapshot has arrived yet.
              </div>
            )}
          </div>
        </Card>
      )}

      <Card title={`Message History (${messages.length})`}>
        {messages.length === 0 ? (
          <p className="text-dimmed text-sm">
            No messages for this mobile yet. Messages will appear as they are sent/received.
          </p>
        ) : (
          <div className="space-y-0">
            {messages.map((entry) => {
              const isExpanded = expanded.has(entry.id);
              if (entry.kind === "tx" && entry.stream === "paging") {
                const h = entry.event.header;
                const typeName = h?.msgTypeName ?? "Unknown";
                const detail = formatPagingSummary(entry.event);
                return (
                  <div
                    key={entry.id}
                    className="border-b border-border-subtle py-1.5 px-2 -mx-2 rounded"
                  >
                    <div
                      className="flex items-center gap-2 text-sm cursor-pointer hover:bg-hover rounded"
                      onClick={() => toggleExpand(entry.id)}
                    >
                      <div className="flex-1 min-w-0 flex items-center gap-2">
                        <span className="text-muted font-mono text-xs w-[15rem] shrink-0">{formatTime(entry.ts)}</span>
                        <span className="text-accent-blue font-mono text-xs w-6 shrink-0">TX</span>
                        <span className="text-dimmed text-xs shrink-0">F-PCH</span>
                        <span className="text-primary shrink-0">{typeName}</span>
                        {entry.seenCount > 1 && (
                          <span className="text-muted text-xs shrink-0">x{entry.seenCount}</span>
                        )}
                        {detail && (
                          <span className="text-muted text-xs truncate">{detail}</span>
                        )}
                      </div>
                      <div className="shrink-0 flex items-center gap-1">
                        <span className="text-accent-amber text-xs font-mono w-14 text-right">{h ? `SEQ=${h.msgSeq}` : ""}</span>
                        <span className="text-accent-green text-xs font-mono w-14 text-right">{h?.validAck ? `ACK=${h.ackSeq}` : ""}</span>
                        <span className="text-badge-orange-text text-xs w-4 text-center" title="ACK required">{h?.ackReq ? "\u21A9" : ""}</span>
                        <span className="text-dimmed text-xs">{isExpanded ? "▾" : "▸"}</span>
                      </div>
                    </div>
                    {isExpanded && h && (
                      <div className="mt-1.5 ml-8 pb-1">
                        <div className="text-xs text-muted mb-1">
                          MSG_TAG: 0x{h.msgTag.toString(16).toUpperCase().padStart(2, "0")} | MSG_SEQ: {h.msgSeq} | ACK_SEQ: {h.ackSeq} | ACK_REQ: {h.ackReq ? "1" : "0"} | VALID_ACK: {h.validAck ? "1" : "0"}
                        </div>
                        <PagingDetail event={entry.event} />
                      </div>
                    )}
                  </div>
                );
              } else if (entry.kind === "tx") {
                const h = entry.event.header;
                const typeName = h?.msgTypeName ?? "Unknown";
                const detail = formatTrafficSummary(entry.event);
                return (
                  <div
                    key={entry.id}
                    className="border-b border-border-subtle py-1.5 px-2 -mx-2 rounded"
                  >
                    <div
                      className="flex items-center gap-2 text-sm cursor-pointer hover:bg-hover rounded"
                      onClick={() => toggleExpand(entry.id)}
                    >
                      <div className="flex-1 min-w-0 flex items-center gap-2">
                        <span className="text-muted font-mono text-xs w-[15rem] shrink-0">{formatTime(entry.ts)}</span>
                        <span className="text-accent-cyan font-mono text-xs w-6 shrink-0">TX</span>
                        <span className="text-dimmed text-xs shrink-0">{formatTrafficChannel(entry.event)}</span>
                        <span className="text-primary shrink-0">{typeName}</span>
                        {entry.seenCount > 1 && (
                          <span className="text-muted text-xs shrink-0">x{entry.seenCount}</span>
                        )}
                        {detail && (
                          <span className="text-muted text-xs truncate">{detail}</span>
                        )}
                      </div>
                      <div className="shrink-0 flex items-center gap-1">
                        <span className="text-accent-amber text-xs font-mono w-14 text-right">{h ? `SEQ=${h.msgSeq}` : ""}</span>
                        <span className="text-accent-green text-xs font-mono w-14 text-right">{h?.validAck ? `ACK=${h.ackSeq}` : ""}</span>
                        <span className="text-badge-orange-text text-xs w-4 text-center" title="ACK required">{h?.ackReq ? "\u21A9" : ""}</span>
                        <span className="text-dimmed text-xs">{isExpanded ? "▾" : "▸"}</span>
                      </div>
                    </div>
                    {isExpanded && h && (
                      <div className="mt-1.5 ml-8 pb-1">
                        <div className="text-xs text-muted mb-1">
                          MSG_TAG: 0x{h.msgTag.toString(16).toUpperCase().padStart(2, "0")} | MSG_SEQ: {h.msgSeq} | ACK_SEQ: {h.ackSeq} | ACK_REQ: {h.ackReq ? "1" : "0"} | VALID_ACK: {h.validAck ? "1" : "0"}
                        </div>
                        <TrafficDetail event={entry.event} />
                      </div>
                    )}
                  </div>
                );
              } else {
                const ev = entry.event;
                return (
                  <div
                    key={entry.id}
                    className="border-b border-border-subtle py-1.5 px-2 -mx-2 rounded"
                  >
                    <div
                      className="flex items-center gap-2 text-sm cursor-pointer hover:bg-hover rounded"
                      onClick={() => toggleExpand(entry.id)}
                    >
                      <div className="flex-1 min-w-0 flex items-center gap-2">
                        <span className="text-muted font-mono text-xs w-[15rem] shrink-0">{formatTime(entry.ts)}</span>
                        <span className="text-accent-green font-mono text-xs w-6 shrink-0">RX</span>
                        <span className="text-dimmed text-xs shrink-0">{formatAccessChannel(ev)}</span>
                        <span className="text-primary shrink-0">{ev.msgTypeName}</span>
                        {entry.seenCount > 1 && (
                          <span className="text-muted text-xs shrink-0">x{entry.seenCount}</span>
                        )}
                        <span className="text-muted text-xs truncate">{formatAccessSummary(ev)}</span>
                      </div>
                      <div className="shrink-0 flex items-center gap-1">
                        <span className="text-accent-amber text-xs font-mono w-14 text-right">{ev.msgSeq != null ? `SEQ=${ev.msgSeq}` : ""}</span>
                        <span className="text-accent-green text-xs font-mono w-14 text-right">{ev.validAck && ev.ackSeq != null ? `ACK=${ev.ackSeq}` : ""}</span>
                        <span className="text-badge-orange-text text-xs w-4 text-center" title="ACK required">{ev.ackReq ? "\u21A9" : ""}</span>
                        <span className="text-dimmed text-xs">{isExpanded ? "▾" : "▸"}</span>
                      </div>
                    </div>
                    {isExpanded && (
                      <div className="mt-1.5 ml-8 pb-1">
                        <AccessDetail event={ev} />
                      </div>
                    )}
                  </div>
                );
              }
            })}
          </div>
        )}
      </Card>
    </div>
  );
}
