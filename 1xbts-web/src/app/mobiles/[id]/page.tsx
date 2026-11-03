"use client";

import { VoicePrivacyIndicator } from "@/components/voice-privacy-indicator";
import { useEffect, useLayoutEffect, useState, useCallback, useRef, use, useMemo, type FormEvent, type ReactNode } from "react";
import Link from "next/link";
import { useRouter } from "next/navigation";
import { Card, Stat } from "@/components/card";
import { useAuthenticationVerdicts } from "@/components/authentication-activity";
import { AccessStatus } from "@/components/access-activity";
import { MobileActivity } from "@/components/mobile-activity";
import { Pager } from "@/components/pager";
import {
  MESSAGE_CATEGORY_LABEL,
  type MessageCategory,
  MessageListHeader,
  MessageRow,
  messageView,
} from "@/components/message-row";
import { SendSmsDialog } from "@/components/mobile/send-sms-dialog";
import { Toast, useToast } from "@/components/toast";
import { SmsList } from "@/components/mobile/sms-list";
import { StartOtasp } from "@/components/start-otasp";
import { RecentOtaspCard } from "@/components/recent-otasp-card";
import { TimeSeriesChart } from "@/components/time-series-chart";
import { esnManufacturer } from "@/lib/esn-manufacturer";
import { shouldHideAccessEvent } from "@/components/message-detail";
import {
  fingerprintAccessEvent,
  fingerprintPagingEvent,
  fingerprintTrafficEvent,
} from "@/lib/event-fingerprint";
import { liveSocket, useLive } from "@/lib/live";
import { useEventStream, type EventStreamOptions } from "@/lib/use-event-stream";
import { hrpdTimestampNsToUs } from "@/lib/hrpd-correlation";
import { type LogEntry, makeLogEntryId, makeSortKey, sortLogEntries, formatTime } from "@/lib/message-log";
import { a1TimestampUs, type A1LogEvent } from "@/lib/a1-event";
import { cellLabel, cellToken, peerIdForCell } from "@/lib/cell";
import { useBtsList } from "@/lib/use-bts-list";
import { formatEsn, formatMeid } from "@/lib/format";
import { isSyntheticPacketMobileAddress } from "@/lib/mobile-directory";
import { radioConfigName, radioConfigPairName } from "@/lib/radio-config";
import { serviceOptionName } from "@/lib/service-option";
import type { AccessEvent, PagingEvent, TrafficEvent } from "@/lib/proto/bsc/v1/service";
import type {
  HrpdAccessEvent,
  HrpdSessionEvent,
  HrpdTrafficEvent,
} from "@/lib/proto/events/v1/an";
import type { SubscriberIdentity } from "@/lib/proto/hlr/v1/service";
import type { AccessSignal } from "@/lib/proto/bsc/v1/service";
import {
  Bars,
  RxPowerCard,
  SignalQualityCard,
  accessSignalBars,
  barsTier,
} from "@/components/signal-quality";
import { CALL_KIND_BADGE, CALL_KIND_LABEL, callKind, callStateLabel } from "@/lib/call-kind";
import { measuredReverseDb, recentMedianDb, reverseMetricLabel, reverseQualityBars } from "@/lib/reverse-quality";

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
  accessSignal?: AccessSignal;
  recentAccessEbNtDb?: number[];
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
    forwardRadioConfig: number;
    reverseRadioConfig: number;
    measuredInnerLoop1sDb?: number;
    measuredInnerLoop1sTimestampMs?: number;
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
  voicePrivacy?: string;
  servingCell?: { cell: number; sector: number };
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
  imsi: string;
  meid: string;
  mnId: string;
  mnIdSource: string;
  subscriberImsi: string;
  esn: number;
  trafficWalshCode: number;
  captureEnabled: boolean;
  accessTechnology: string;
}

interface SubscriberDetail {
  subscriber?: { displayName?: string };
  identities: SubscriberIdentity[];
  error?: string;
}

const MAX_MESSAGES = 200;
const NO_PACKET_SESSIONS: PacketSessionInfo[] = [];
const MESSAGE_PAGE_SIZES = [25, 50, 100] as const;
// C.S0005: a slot cycle is 16 x 2^SLOT_CYCLE_INDEX slots of 80 ms.
const SLOT_CYCLE_BASE_SECONDS = 1.28;

type MobileTab = "overview" | "call" | "messages" | "sms" | "otasp" | "activity" | "data";
const RECENT_MESSAGE_ROWS = 7;

function countActivePcgs(pcgSnrDb: number[], activePcgMask?: boolean[]): number {
  if (activePcgMask && activePcgMask.length === pcgSnrDb.length) {
    return activePcgMask.filter((active, index) => active && Number.isFinite(pcgSnrDb[index])).length;
  }
  const finite = pcgSnrDb.filter((value) => Number.isFinite(value));
  if (finite.length === 0) return 0;
  const max = Math.max(...finite);
  return finite.filter((value) => value >= max - 10).length;
}

function getGatedCutoffDb(pcgSnrDb: number[], activePcgMask?: boolean[]): number | null {
  if (activePcgMask && activePcgMask.length === pcgSnrDb.length) return null;
  const finite = pcgSnrDb.filter((value) => Number.isFinite(value));
  if (finite.length === 0) return null;
  return Math.max(...finite) - 10;
}

function getFrameMetricPcgIndices(
  pcgSnrDb: number[],
  activePcgMask?: boolean[],
): number[] {
  if (pcgSnrDb.length === 0) return [];
  const finite = pcgSnrDb.filter((value) => Number.isFinite(value));
  if (finite.length === 0) return [];
  const fallbackCutoff = Math.max(...finite) - 10;
  const candidateIndices =
    activePcgMask && activePcgMask.length === pcgSnrDb.length
      ? activePcgMask.flatMap((active, index) =>
          active && Number.isFinite(pcgSnrDb[index]) ? [index] : [],
        )
      : pcgSnrDb.flatMap((db, index) =>
          Number.isFinite(db) && db >= fallbackCutoff ? [index] : [],
        );
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
  if (!Number.isFinite(db)) return false;
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

function subscriberTitle(subscriberId: string, displayName: string): string {
  return displayName || `Subscriber ${subscriberId.slice(-4)}`;
}

function firstVisibleRow(container: HTMLElement | null): { id: string; top: number } | null {
  if (!container || container.getBoundingClientRect().top >= 0) return null;
  for (const row of container.querySelectorAll<HTMLElement>("[data-entry-id]")) {
    const rect = row.getBoundingClientRect();
    if (rect.bottom > 0 && row.dataset.entryId) return { id: row.dataset.entryId, top: rect.top };
  }
  return null;
}

function IdentityRow({
  label,
  observed,
  provisioned,
}: {
  label: string;
  observed: string | null;
  provisioned: string | null;
}) {
  if (observed && provisioned && observed.toUpperCase() !== provisioned.toUpperCase()) {
    return (
      <div className="flex justify-between gap-3 py-1 px-2 -mx-2 my-0.5 rounded bg-accent-amber-bg">
        <span className="text-muted text-sm">{label}</span>
        <span className="text-sm font-mono text-right">
          <span className="text-secondary">{observed}</span>{" "}
          <span className="text-accent-amber">≠ {provisioned} provisioned</span>
        </span>
      </div>
    );
  }
  return (
    <div className="flex justify-between gap-3 py-0.5">
      <span className="text-muted text-sm">{label}</span>
      <span className="text-secondary text-sm font-mono text-right">
        {observed ?? provisioned ?? "Not available"}
        {observed && provisioned && (
          <span className="text-accent-green ml-1.5" title="Matches provisioning">✓</span>
        )}
        {!observed && provisioned && <span className="text-muted text-xs ml-1.5">provisioned, not seen</span>}
      </span>
    </div>
  );
}

function Tile({ label, sub, children }: { label: string; sub?: string; children: ReactNode }) {
  return (
    <div className="rounded-lg bg-surface-raised px-3 py-2">
      <div className="text-[10px] uppercase tracking-wide text-muted">{label}</div>
      <div className="mt-0.5 text-lg font-semibold text-primary">{children}</div>
      {sub && <div className="text-[11px] text-muted">{sub}</div>}
    </div>
  );
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

function hrpdTimestampUs(event: HrpdSessionEvent | HrpdAccessEvent | HrpdTrafficEvent): number {
  return hrpdTimestampNsToUs(event.timestampNs);
}

function hrpdMatchesMobile(
  event: HrpdSessionEvent | HrpdAccessEvent | HrpdTrafficEvent,
  mobile: MobileInfo,
): boolean {
  const enriched = event as typeof event & {
    identity?: { imsi?: string; esn?: number; meid?: string };
    subscriber?: { subscriberId?: string };
  };
  if (mobile.subscriberId && enriched.subscriber?.subscriberId === mobile.subscriberId) return true;
  if (mobile.imsi && enriched.identity?.imsi === mobile.imsi) return true;
  if (mobile.esn != null && enriched.identity?.esn === mobile.esn) return true;
  if (mobile.meid && enriched.identity?.meid?.toLowerCase() === mobile.meid.toLowerCase()) return true;
  return false;
}

// An A1 message matches this mobile by the bus-resolved subscriber or its Mobile
// Identity IEs — there's no air-interface address or Walsh code to fall back on.
function a1MatchesMobile(event: A1LogEvent, mobile: MobileInfo): boolean {
  if (mobile.subscriberId && event.subscriber?.subscriberId === mobile.subscriberId) return true;
  const imsi = event.identity?.imsi || event.mobileIdentity?.imsi;
  const esn = event.identity?.esn || event.mobileIdentity?.esn;
  const meid = event.identity?.meid || event.mobileIdentity?.meid;
  if (mobile.imsi && imsi === mobile.imsi) return true;
  if (mobile.esn != null && esn === mobile.esn) return true;
  if (mobile.meid && meid?.toLowerCase() === mobile.meid.toLowerCase()) return true;
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

  const liveMobiles = useLive<MobileInfo[]>("/api/mobiles");
  const livePackets = useLive<{ sessions?: PacketSessionInfo[] }>("/api/packet-sessions");
  const mobile = useMemo(
    () => (Array.isArray(liveMobiles.data) ? liveMobiles.data.find((m) => m.address === address) ?? null : null),
    [liveMobiles.data, address],
  );
  const authImsi = mobile?.imsi;
  const authImsis = useMemo(() => (authImsi ? [authImsi] : []), [authImsi]);
  const { events: verdicts } = useAuthenticationVerdicts(authImsis, mobile?.esn ?? undefined);
  const loading = liveMobiles.loading;
  const { cells } = useBtsList();
  const [messages, setMessages] = useState<LogEntry[]>([]);
  const replayEvents: EventStreamOptions = { replay: true, enabled: mobile != null };
  const [expanded, setExpanded] = useState<Set<string>>(new Set());
  const packetSessions = livePackets.data?.sessions ?? NO_PACKET_SESSIONS;
  const [subscriberName, setSubscriberName] = useState("");
  const [subscriberIdentities, setSubscriberIdentities] = useState<SubscriberIdentity[]>([]);
  const [tab, setTab] = useState<MobileTab>("overview");
  const [msgPage, setMsgPage] = useState(0);
  const [msgPageSize, setMsgPageSize] = useState<number>(MESSAGE_PAGE_SIZES[0]);
  const [msgPaused, setMsgPaused] = useState(false);
  const [shownMessages, setShownMessages] = useState<LogEntry[]>([]);
  const [pinned, setPinned] = useState<LogEntry[]>([]);
  const messageListRef = useRef<HTMLDivElement>(null);
  const scrollAnchorRef = useRef<{ id: string; top: number } | null>(null);
  const [msgFilter, setMsgFilter] = useState<MessageCategory | "all">("all");
  const [msgQuery, setMsgQuery] = useState("");
  const [focusEntryId, setFocusEntryId] = useState<string | null>(null);
  const [smsOpen, setSmsOpen] = useState(false);
  const [smsRefresh, setSmsRefresh] = useState(0);
  const [toast, showToast] = useToast();
  const [callPending, setCallPending] = useState(false);
  const [callResult, setCallResult] = useState<SmsResult | null>(null);

  // SMS form
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
      const oneSecondMean = ms.trafficPower.measuredInnerLoop1sDb;
      const oneSecondTimestamp = ms.trafficPower.measuredInnerLoop1sTimestampMs;
      if (
        Number.isFinite(oneSecondMean) &&
        oneSecondTimestamp != null &&
        oneSecondTimestamp > 0
      ) {
        const h = powerHistoryRef.current;
        const latestTimestamp = h.timestamps[h.timestamps.length - 1];
        if (latestTimestamp == null || oneSecondTimestamp > latestTimestamp) {
          h.timestamps.push(oneSecondTimestamp);
          h.target.push(ms.trafficPower.effectiveTargetEbNtDb);
          h.measured.push(oneSecondMean as number);
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
      } else if (serverHistory.length > 0) {
        const h = powerHistoryRef.current;
        h.timestamps = serverHistory.map((s) => Number(s.timestampMs));
        h.target = serverHistory.map((s) => s.targetEbNtDb);
        h.measured = serverHistory.map((s) => s.measuredEbNtDb);
        h.forwardGain = serverHistory.map((s) => s.forwardGainDb);
        h.fer = serverHistory.map((s) => s.ferPct);
      } else if (ms.trafficPower.reverseRadioConfig !== 3) {
        const h = powerHistoryRef.current;
        const now = Date.now();
        h.timestamps.push(now);
        h.target.push(ms.trafficPower.effectiveTargetEbNtDb);
        const pilotPcg = ms.trafficPower.lastPcgPilotEcNtDb ?? [];
        const pcg = pilotPcg.length > 0 ? pilotPcg : (ms.trafficPower.lastPcgSnrDb ?? []);
        const valid = pcg.filter((value) => Number.isFinite(value));
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

  useEffect(() => {
    if (mobile) updateChartHistory(mobile);
  }, [mobile, updateChartHistory]);

  const refreshMobile = useCallback(() => liveSocket().refresh("/api/mobiles"), []);

  useEffect(() => {
    if (!mobile?.subscriberId) {
      setSubscriberIdentities([]);
      setSubscriberName("");
      return;
    }

    setSubscriberName("");
    let cancelled = false;
    const fetchSubscriberIdentities = async () => {
      try {
        const res = await fetch(`/api/subscribers/${encodeURIComponent(mobile.subscriberId!)}`);
        const data: SubscriberDetail = await res.json();
        if (!cancelled) {
          setSubscriberIdentities(res.ok && !data.error ? data.identities || [] : []);
          setSubscriberName(res.ok && !data.error ? data.subscriber?.displayName || "" : "");
        }
      } catch {
        if (!cancelled) {
          setSubscriberIdentities([]);
          setSubscriberName("");
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
  }, [addMessage, mobile]), replayEvents);

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
  }, [addMessage, mobile]), replayEvents);

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
  }, [addMessage, mobile]), replayEvents);

  useEventStream("hrpd-session", useCallback((data: string) => {
    if (!mobile) return;
    const event: HrpdSessionEvent = JSON.parse(data);
    if (!hrpdMatchesMobile(event, mobile)) return;
    const ts = hrpdTimestampUs(event);
    const sequence = (event as HrpdSessionEvent & { sequence?: string | number }).sequence ?? ts;
    addMessage({
      kind: "hrpd",
      stream: "session",
      id: makeLogEntryId("hrpd", ts),
      ts,
      identity: `hrpd:session:${event.uati}:${event.reason}:${sequence}`,
      sortKey: `hrpd:session:${event.uati}:${sequence}`,
      event,
      seenCount: 1,
    });
  }, [addMessage, mobile]), replayEvents);

  useEventStream("hrpd-access", useCallback((data: string) => {
    if (!mobile) return;
    const event: HrpdAccessEvent = JSON.parse(data);
    if (!hrpdMatchesMobile(event, mobile)) return;
    const ts = hrpdTimestampUs(event);
    const sequence = (event as HrpdAccessEvent & { sequence?: string | number }).sequence ?? ts;
    addMessage({
      kind: "hrpd",
      stream: "access",
      id: makeLogEntryId("hrpd", ts),
      ts,
      identity: `hrpd:access:${event.uati}:${event.accessSignature}:${event.reason}:${sequence}`,
      sortKey: `hrpd:access:${event.uati}:${sequence}`,
      event,
      seenCount: 1,
    });
  }, [addMessage, mobile]), replayEvents);

  useEventStream("hrpd-traffic", useCallback((data: string) => {
    if (!mobile) return;
    const event: HrpdTrafficEvent = JSON.parse(data);
    if (!hrpdMatchesMobile(event, mobile)) return;
    const ts = hrpdTimestampUs(event);
    const sequence = (event as HrpdTrafficEvent & { sequence?: string | number }).sequence ?? ts;
    addMessage({
      kind: "hrpd",
      stream: "traffic",
      id: makeLogEntryId("hrpd", ts),
      ts,
      identity: `hrpd:traffic:${event.uati}:${event.reason}:${event.macIndex}:${event.direction}:${sequence}`,
      sortKey: `hrpd:traffic:${event.uati}:${sequence}`,
      event,
      seenCount: 1,
    });
  }, [addMessage, mobile]), replayEvents);

  useEventStream("a1", useCallback((data: string) => {
    if (!mobile) return;
    const event: A1LogEvent = JSON.parse(data);
    if (!a1MatchesMobile(event, mobile)) return;
    const ts = a1TimestampUs(event);
    const identity = `a1:${event.eventId || `${event.messageTypeValue}:${event.sduHex}:${ts}`}`;
    addMessage({
      kind: "a1",
      id: makeLogEntryId("a1", ts),
      ts,
      identity,
      sortKey: makeSortKey(event.eventId, identity),
      event,
      seenCount: 1,
    });
  }, [addMessage, mobile]), replayEvents);

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
      refreshMobile();
      setShowCreateSubscriber(false);
    } catch (err) {
      setCreateError(err instanceof Error ? err.message : "unknown error");
    } finally {
      setCreatingSubscriber(false);
    }
  }, [createDisplayName, createPhoneNumber, refreshMobile, mobile, router]);

  const applyPowerOverride = useCallback(async (payload: { targetDb?: number; clear?: boolean }) => {
    if (mobile?.trafficWalshCode == null) return;
    // A mobile not bound to a cell (unregistered, or synthesized from an HRPD
    // session) leaves the cell unnamed, so the BSC resolves it as an unaddressed request.
    const cell = cellToken(mobile.servingCell);

    setPowerOverridePending(true);
    setPowerOverrideError(null);
    try {
      const res = await fetch("/api/channels/power-override", {
        method: "POST",
        headers: { "Content-Type": "application/json" },
        body: JSON.stringify({
          walshCode: mobile.trafficWalshCode,
          ...(cell ? { cell } : {}),
          ...payload,
        }),
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
      refreshMobile();
    } catch (err) {
      setPowerOverrideError(err instanceof Error ? err.message : "unknown error");
    } finally {
      setPowerOverridePending(false);
    }
  }, [refreshMobile, mobile?.trafficWalshCode, mobile?.servingCell]);

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

  const messageOpen = shownMessages.some((entry) => expanded.has(entry.id));
  const holdMessages = msgPaused || messageOpen || msgPage > 0;

  useEffect(() => {
    if (holdMessages) return;
    scrollAnchorRef.current = firstVisibleRow(messageListRef.current);
    setShownMessages(messages);
  }, [messages, holdMessages]);

  useLayoutEffect(() => {
    const anchor = scrollAnchorRef.current;
    scrollAnchorRef.current = null;
    if (!anchor || !messageListRef.current) return;
    const row = messageListRef.current.querySelector(`[data-entry-id="${CSS.escape(anchor.id)}"]`);
    if (row) window.scrollBy(0, row.getBoundingClientRect().top - anchor.top);
  }, [shownMessages]);

  const heldMessageCount = useMemo(() => {
    const shownIds = new Set(shownMessages.map((entry) => entry.id));
    return messages.filter((entry) => !shownIds.has(entry.id)).length;
  }, [messages, shownMessages]);

  const showHeldMessages = useCallback(() => {
    setMsgPage(0);
    setShownMessages(messages);
  }, [messages]);

  const togglePin = useCallback((entry: LogEntry) => {
    setPinned((prev) =>
      prev.some((p) => p.id === entry.id) ? prev.filter((p) => p.id !== entry.id) : [entry, ...prev],
    );
  }, []);

  useEffect(() => {
    if (!focusEntryId || tab !== "messages") return;
    const row = messageListRef.current?.querySelector(`[data-entry-id="${CSS.escape(focusEntryId)}"]`);
    row?.scrollIntoView({ block: "center" });
    setFocusEntryId(null);
  }, [focusEntryId, tab, shownMessages]);

  const placeCall = useCallback(async () => {
    if (!mobile?.subscriberId) return;
    setCallPending(true);
    setCallResult(null);
    try {
      const res = await fetch("/api/calls", {
        method: "POST",
        headers: { "Content-Type": "application/json" },
        body: JSON.stringify({ subscriberId: mobile.subscriberId }),
      });
      const data = await res.json();
      setCallResult({
        accepted: Boolean(data.accepted),
        message: data.message || (data.accepted ? "Calling" : `HTTP ${res.status}`),
      });
    } catch (err) {
      setCallResult({ accepted: false, message: err instanceof Error ? err.message : "unknown error" });
    } finally {
      setCallPending(false);
    }
  }, [mobile?.subscriberId]);

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

  const servingPeerId = peerIdForCell(cells, mobile.servingCell);
  const observedEsnHex = mobile.esn != null ? formatEsn(mobile.esn) : null;
  const observedMeid = mobile.meid ? formatMeid(mobile.meid) : null;
  const provisionedEsnHex =
    primarySubscriberIdentity?.esn != null ? formatEsn(primarySubscriberIdentity.esn) : null;
  const provisionedImsi = primarySubscriberIdentity?.imsi || null;
  const provisionedMeid = primarySubscriberIdentity?.meid ? formatMeid(primarySubscriberIdentity.meid) : null;
  const canCreateSubscriber = !mobile.subscriberId && Boolean(mobile.imsi) && (mobile.esn != null || Boolean(mobile.meid));
  const trafficPower = mobile.trafficPower;
  const forwardRcName = radioConfigName(trafficPower?.forwardRadioConfig);
  const reverseRcName = radioConfigName(trafficPower?.reverseRadioConfig);
  const radioConfigPair = radioConfigPairName(
    trafficPower?.forwardRadioConfig,
    trafficPower?.reverseRadioConfig,
  );
  const isRc3 = trafficPower?.reverseRadioConfig === 3;
  // Inner-loop metric: pilot SINR (RC3) or Eb/Nt (RC1/RC2). Falls back to frame snapshot.
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
    if (mobile.imsi && (session.subscriberImsi === mobile.imsi || session.imsi === mobile.imsi)) {
      return true;
    }
    if (mobile.esn != null && session.esn === mobile.esn) return true;
    if (mobile.meid && session.meid === mobile.meid) return true;
    if (
      session.mobileAddress &&
      !isSyntheticPacketMobileAddress(session.mobileAddress) &&
      session.mobileAddress === mobile.address
    ) {
      return true;
    }
    return false;
  });

  const pageTitle = mobile.subscriberId
    ? subscriberTitle(mobile.subscriberId, subscriberName)
    : formatMobileTitle(mobile);
  const onCall = mobile.trafficWalshCode != null;
  const activeTab: MobileTab = tab === "call" && !onCall ? "overview" : tab;
  const deviceEsn = mobile.esn ?? primarySubscriberIdentity?.esn;
  const manufacturer = deviceEsn != null ? (esnManufacturer(deviceEsn) ?? null) : null;
  const provisioned = Boolean(mobile.subscriberId);
  const identityPairs: [string, string | null, string | null][] = [
    ["IMSI", mobile.imsi || null, provisioned ? provisionedImsi : null],
    ["MEID", observedMeid, provisioned ? provisionedMeid : null],
    ["ESN", observedEsnHex, provisioned ? provisionedEsnHex : null],
  ];
  const mismatched = identityPairs
    .filter(([, seen, prov]) => seen && prov && seen.toUpperCase() !== prov.toUpperCase())
    .map(([label]) => label);
  const pagingLabel = `slot ${mobile.pgslot ?? "-"} · every ${(
    SLOT_CYCLE_BASE_SECONDS * 2 ** mobile.slotCycleIndex
  ).toFixed(2)} s`;
  const kind = callKind(mobile.trafficServiceOption);
  const reverseMeasuredDb = trafficPower
    ? (recentMedianDb(powerHistoryRef.current.measured) ?? measuredReverseDb(trafficPower))
    : undefined;
  const reverseBars = reverseQualityBars(trafficPower?.reverseRadioConfig, reverseMeasuredDb);
  const headerBars = trafficPower ? reverseBars : accessSignalBars(mobile.accessSignal);
  const headerSignalValue =
    trafficPower && reverseMeasuredDb != null
      ? `${reverseMeasuredDb.toFixed(1)} dB ${reverseMetricLabel(trafficPower.reverseRadioConfig)} reverse`
      : mobile.accessSignal
        ? `${mobile.accessSignal.ebNtDb.toFixed(1)} dB Eb/Nt access`
        : "No measurement";
  const messageCtx = {
    mobile: { imsi: mobile.imsi ?? undefined, esn: mobile.esn ?? undefined },
    verdicts,
    cells,
  };
  const query = msgQuery.trim().toLowerCase();
  const shownViews = shownMessages.map((entry) => ({ entry, view: messageView(entry, messageCtx) }));
  const categories = [...new Set(shownViews.map(({ view }) => view.category))];
  const filteredViews = shownViews.filter(
    ({ view }) =>
      (msgFilter === "all" || view.category === msgFilter) &&
      (!query || `${view.type} ${view.summary} ${view.channel}`.toLowerCase().includes(query)),
  );
  const filteredPages = Math.max(1, Math.ceil(filteredViews.length / msgPageSize));
  const filteredPage = Math.min(msgPage, filteredPages - 1);
  const pageViews = filteredViews.slice(filteredPage * msgPageSize, (filteredPage + 1) * msgPageSize);
  const liveLabel = msgPaused
    ? "Paused"
    : messageOpen
      ? "Paused while a message is open"
      : filteredPage > 0
        ? `Paused on page ${filteredPage + 1}`
        : "Live";
  const tabs: { id: MobileTab; label: string; badge?: string }[] = [
    { id: "overview", label: "Overview" },
    ...(onCall ? [{ id: "call" as const, label: "Call", badge: "live" }] : []),
    { id: "messages", label: "Messages", badge: String(messages.length) },
    { id: "sms", label: "SMS" },
    { id: "otasp", label: "OTASP" },
    { id: "activity", label: "Activity" },
    { id: "data", label: "Data", badge: String(mobilePacketSessions.length) },
  ];

  const openMessage = (id: string) => {
    setTab("messages");
    setMsgFilter("all");
    setMsgQuery("");
    setMsgPage(0);
    setShownMessages(messages);
    setExpanded((prev) => new Set(prev).add(id));
    setFocusEntryId(id);
  };

  const pinControl = (entry: LogEntry) => (
    <button
      type="button"
      onClick={(e) => {
        e.stopPropagation();
        togglePin(entry);
      }}
      className="rounded-md border border-border-input px-2 py-0.5 text-[11px] text-secondary hover:bg-hover"
    >
      {pinned.some((p) => p.id === entry.id) ? "Unpin" : "Pin"}
    </button>
  );

  const reverseTileClass =
    reverseMeasuredDb != null && reverseTargetDb != null && reverseMeasuredDb < reverseTargetDb - 1
      ? "text-accent-red"
      : "text-primary";

  return (
    <div className="max-w-7xl mx-auto">
      <Link href="/mobiles" className="text-sm text-muted hover:text-secondary">
        &larr; Mobiles
      </Link>
      <div className="mt-3 flex flex-wrap items-center gap-x-6 gap-y-3">
        <div className="min-w-0 flex-1">
          <div className="flex flex-wrap items-center gap-2">
            <h1 className="text-xl font-semibold">{pageTitle}</h1>
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
            {mobile.voiceCallState && (
              <span className="inline-flex items-center gap-1.5 text-xs px-2 py-0.5 rounded bg-surface-raised text-secondary">
                {callStateLabel(mobile.voiceCallState)}
                <VoicePrivacyIndicator status={mobile.voicePrivacy} />
              </span>
            )}
            {mismatched.length > 0 && (
              <span className="text-xs px-2 py-0.5 rounded bg-accent-amber-bg text-accent-amber">
                {mismatched.join(", ")} mismatch
              </span>
            )}
          </div>
          <div className="mt-1 text-xs text-muted font-mono truncate">
            {[
              mobile.phoneNumber && mobile.phoneNumber !== pageTitle ? mobile.phoneNumber : null,
              mobile.imsi ? `IMSI ${mobile.imsi}` : null,
              observedMeid ? `MEID ${observedMeid}` : null,
              observedEsnHex ? `ESN ${observedEsnHex}` : null,
            ]
              .filter(Boolean)
              .join(" · ")}
          </div>
          <div className="text-xs text-muted">
            {[
              manufacturer,
              `P_REV ${mobile.mobPRev}`,
              mobile.servingCell ? cellLabel(mobile.servingCell) : null,
              mobile.lastHeardMs ? `last heard ${formatTime(mobile.lastHeardMs * 1000)}` : null,
            ]
              .filter(Boolean)
              .join(" · ")}
          </div>
        </div>
        <div className="flex items-center gap-3">
          <Bars bars={headerBars} size="lg" />
          <div>
            <div className={`text-sm font-semibold ${barsTier(headerBars)?.text ?? "text-muted"}`}>
              {barsTier(headerBars)?.label ?? "Unknown"}
            </div>
            <div className="text-xs text-muted font-mono">{headerSignalValue}</div>
          </div>
        </div>
        <div className="flex flex-wrap items-start gap-2">
          {mobile.subscriberId && (
            <button
              type="button"
              onClick={placeCall}
              disabled={callPending || onCall}
              className="text-xs px-3 py-1.5 rounded bg-accent-green-bg text-accent-green border border-accent-green/20 hover:bg-accent-green/15 disabled:opacity-50 transition-colors"
            >
              {callPending ? "Calling..." : "Call"}
            </button>
          )}
          <button
            type="button"
            onClick={() => setSmsOpen(true)}
            className="text-xs px-3 py-1.5 rounded bg-surface-raised text-secondary border border-border hover:bg-hover transition-colors"
          >
            Send SMS
          </button>
          {mobile.subscriberId && (
            <StartOtasp subscriberId={mobile.subscriberId} imsi={mobile.imsi || undefined} compact />
          )}
        </div>
      </div>
      {callResult && (
        <p className={`mt-2 text-xs ${callResult.accepted ? "text-accent-green" : "text-accent-red"}`}>
          {callResult.message}
        </p>
      )}

      <div className="mt-4 mb-4 flex gap-1 overflow-x-auto border-b border-border">
        {tabs.map((t) => (
          <button
            key={t.id}
            type="button"
            onClick={() => setTab(t.id)}
            className={`-mb-px whitespace-nowrap border-b-2 px-3.5 py-2 text-sm transition-colors ${
              activeTab === t.id
                ? "border-accent-green text-primary"
                : "border-transparent text-muted hover:text-secondary"
            }`}
          >
            {t.label}
            {t.badge && (
              <span
                className={`ml-1.5 rounded-full bg-surface-raised px-1.5 py-px text-[10px] ${
                  t.id === "call" ? "text-accent-green" : "text-muted"
                }`}
              >
                {t.badge}
              </span>
            )}
          </button>
        ))}
      </div>

      {activeTab === "overview" && (
        <div className="grid grid-cols-1 md:grid-cols-2 xl:grid-cols-3 grid-flow-row-dense gap-4">
          {onCall && (
            <Card title="Current Call" action={<span className="text-[11px] text-accent-green">● live</span>}>
              <div className="mb-2 flex flex-wrap items-center gap-2">
                <span className={`text-xs px-2 py-0.5 rounded ${CALL_KIND_BADGE[kind]}`}>{CALL_KIND_LABEL[kind]}</span>
                {mobile.trafficServiceOption != null && (
                  <span className="text-xs text-muted">{serviceOptionName(mobile.trafficServiceOption)}</span>
                )}
              </div>
              <Stat
                label="Channel"
                value={`W${mobile.trafficWalshCode}${radioConfigPair ? ` · ${radioConfigPair}` : ""}`}
                mono
              />
              {trafficPower ? (
                <>
                  <div className="flex justify-between py-0.5">
                    <span className="text-muted text-sm">Reverse</span>
                    <span className="inline-flex items-center gap-2 text-secondary text-sm font-mono">
                      <Bars bars={reverseBars} />
                      {reverseMeasuredDb != null ? `${reverseMeasuredDb.toFixed(1)} dB ${metricLabel}` : "-"}
                    </span>
                  </div>
                  <Stat
                    label="Reverse FER"
                    value={`${trafficPower.ferPct.toFixed(1)}% · ${reverseLifetimeFerPct.toFixed(2)}% lifetime`}
                    mono
                  />
                  <Stat
                    label="Forward FER"
                    value={`${
                      trafficPower.forwardPmrmCount > 0 ? `${trafficPower.forwardLastFerPct.toFixed(1)}%` : "no PMRM yet"
                    } · gain ${trafficPower.forwardGainOffsetDb >= 0 ? "+" : ""}${trafficPower.forwardGainOffsetDb.toFixed(1)} dB`}
                    mono
                  />
                </>
              ) : (
                <p className="text-xs text-muted mt-1">No power-control snapshot yet.</p>
              )}
              <button
                type="button"
                onClick={() => setTab("call")}
                className="mt-3 text-xs px-2.5 py-1 rounded border border-border-input text-secondary hover:bg-surface-raised"
              >
                Call details →
              </button>
            </Card>
          )}

          <SignalQualityCard signal={mobile.accessSignal} recentEbNtDb={mobile.recentAccessEbNtDb} />

          <Card title="Identity">
            <AccessStatus
              imsi={mobile.imsi || undefined}
              esn={mobile.esn ?? undefined}
              meid={mobile.meid || undefined}
              subscriberId={mobile.subscriberId}
            />
            <div className="flex justify-between py-0.5">
              <span className="text-muted text-sm">Subscriber</span>
              {mobile.subscriberId ? (
                <Link
                  href={`/subscribers/${encodeURIComponent(mobile.subscriberId)}`}
                  className="text-sm text-accent-green hover:text-accent-green transition-colors"
                >
                  {pageTitle} →
                </Link>
              ) : (
                <span className="text-sm text-muted">Not provisioned</span>
              )}
            </div>
            {mobile.phoneNumber && <Stat label="Phone" value={mobile.phoneNumber} mono />}
            {identityPairs.map(([label, seen, prov]) => (
              <IdentityRow key={label} label={label} observed={seen} provisioned={prov} />
            ))}
            <div className="my-2 border-t border-border" />
            <Stat label="Device" value={[manufacturer, `P_REV ${mobile.mobPRev}`].filter(Boolean).join(" · ")} />
            <Stat
              label="Paging"
              value={pagingLabel}
              title={`PGSLOT ${mobile.pgslot ?? "-"} · SLOT_CYCLE_INDEX ${mobile.slotCycleIndex}`}
            />
            <div className="flex justify-between py-0.5">
              <span className="text-muted text-sm">Serving cell</span>
              {servingPeerId ? (
                <Link
                  href={`/bts/${encodeURIComponent(servingPeerId)}`}
                  className="text-sm font-mono text-accent-green hover:text-accent-green transition-colors"
                >
                  {cellLabel(mobile.servingCell)}
                </Link>
              ) : (
                <span className="text-secondary text-sm font-mono">-</span>
              )}
            </div>
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
          </Card>

          <Card
            title="Recent Messages"
            className="md:col-span-2"
            action={
              <button
                type="button"
                onClick={() => setTab("messages")}
                className="text-xs px-2 py-0.5 rounded border border-border-input text-secondary hover:bg-surface-raised"
              >
                All {messages.length} →
              </button>
            }
          >
            {messages.length === 0 ? (
              <p className="text-dimmed text-sm">No messages since this page opened.</p>
            ) : (
              <div>
                {messages.slice(0, RECENT_MESSAGE_ROWS).map((entry) => (
                  <MessageRow
                    key={entry.id}
                    entry={entry}
                    view={messageView(entry, messageCtx)}
                    open={false}
                    onToggle={() => openMessage(entry.id)}
                  />
                ))}
              </div>
            )}
          </Card>

          <RxPowerCard signal={mobile.accessSignal} />

          {mobilePacketSessions.length > 0 && (
            <Card title="Data Session" action={<span className="text-xs text-muted">{mobilePacketSessions[0].phase}</span>}>
              <Stat label="Mobile IP" value={mobilePacketSessions[0].peerIp || "-"} mono />
              <Stat label="Service option" value={`SO ${mobilePacketSessions[0].serviceOption}`} mono />
              <Stat
                label="Last activity"
                value={
                  mobilePacketSessions[0].lastActivityAtMs
                    ? new Date(mobilePacketSessions[0].lastActivityAtMs).toLocaleTimeString()
                    : "-"
                }
                mono
              />
              <button
                type="button"
                onClick={() => setTab("data")}
                className="mt-3 text-xs px-2.5 py-1 rounded border border-border-input text-secondary hover:bg-surface-raised"
              >
                {mobilePacketSessions.length > 1 ? `All ${mobilePacketSessions.length} sessions →` : "Session details →"}
              </button>
            </Card>
          )}
        </div>
      )}

      {activeTab === "call" && (
        <div className="space-y-4">
          <Card
            title={`${CALL_KIND_LABEL[kind]} call · W${mobile.trafficWalshCode}${radioConfigPair ? ` · ${radioConfigPair}` : ""}`}
            action={
              <span className="inline-flex items-center gap-2 text-xs text-secondary">
                <span className="text-accent-green">●</span>
                {callStateLabel(mobile.voiceCallState) ?? mobile.state}
                <VoicePrivacyIndicator status={mobile.voicePrivacy} />
              </span>
            }
          >
            {trafficPower ? (
              <>
                <div className="grid grid-cols-2 sm:grid-cols-3 xl:grid-cols-6 gap-3">
                  <Tile label={`Reverse ${metricLabel}`}>
                    <span className="inline-flex items-center gap-2">
                      <Bars bars={reverseBars} />
                      <span className={reverseTileClass}>{reverseMeasuredDb != null ? reverseMeasuredDb.toFixed(1) : "-"}</span>
                      <span className="text-xs font-normal text-muted">dB</span>
                    </span>
                  </Tile>
                  <Tile label="Target" sub={trafficPower.manualTargetOverrideDb != null ? "pinned" : "auto"}>
                    {trafficPower.effectiveTargetEbNtDb.toFixed(1)} <span className="text-xs font-normal text-muted">dB</span>
                  </Tile>
                  <Tile label="Reverse FER" sub="sliding window">
                    <span className={trafficPower.ferPct > 1 ? "text-accent-amber" : "text-accent-green"}>{trafficPower.ferPct.toFixed(1)}%</span>
                  </Tile>
                  <Tile
                    label="Lifetime FER"
                    sub={`${trafficPower.framesCrcError} / ${trafficPower.framesTotal.toLocaleString()} frames`}
                  >
                    <span className={reverseLifetimeFerPct > 1 ? "text-accent-amber" : "text-accent-green"}>
                      {reverseLifetimeFerPct.toFixed(2)}%
                    </span>
                  </Tile>
                  <Tile label="Forward FER" sub={trafficPower.forwardPmrmCount > 0 ? "last PMRM" : "no PMRM yet"}>
                    {trafficPower.forwardPmrmCount > 0 ? `${trafficPower.forwardLastFerPct.toFixed(1)}%` : "-"}
                  </Tile>
                  <Tile label="Forward gain" sub="from initial allocation">
                    {trafficPower.forwardGainOffsetDb >= 0 ? "+" : ""}
                    {trafficPower.forwardGainOffsetDb.toFixed(1)} <span className="text-xs font-normal text-muted">dB</span>
                  </Tile>
                </div>

                <div className="mt-4">
                  <div className="mb-1 text-xs text-muted">
                    Power control · target and measured {metricLabel} · forward gain offset
                  </div>
                  {powerHistoryRef.current.timestamps.length >= 2 ? (
                    <TimeSeriesChart
                      width={900}
                      height={260}
                      yLabel="dB"
                      timestamps={powerHistoryRef.current.timestamps}
                      series={[
                        { key: "target", label: `Target ${metricLabel}`, color: "#f59e0b", values: powerHistoryRef.current.target },
                        { key: "measured", label: `Measured ${metricLabel}`, color: "#22c55e", values: powerHistoryRef.current.measured },
                        { key: "fwd_gain", label: "Fwd gain", color: "#818cf8", values: powerHistoryRef.current.forwardGain },
                      ]}
                    />
                  ) : (
                    <p className="py-10 text-center text-xs text-muted">Collecting history.</p>
                  )}
                </div>

                <div className="mt-4 grid gap-4 md:grid-cols-3">
                  <div>
                    <div className="mb-1 text-[11px] uppercase tracking-wide text-muted">Forward link {forwardRcName ? `· ${forwardRcName}` : ""}</div>
                    <Stat label="PMRMs" value={String(trafficPower.forwardPmrmCount)} mono />
                    <Stat
                      label="Last PMRM"
                      value={
                        trafficPower.forwardPmrmCount > 0
                          ? `${trafficPower.forwardLastPmrmErrors} errors / ${trafficPower.forwardLastPmrmFrames} frames`
                          : "-"
                      }
                      mono
                    />
                    <Stat label="Pilot Ec/Io (mobile)" value={forwardPilotEcIoText} mono />
                  </div>
                  <div>
                    <div className="mb-1 text-[11px] uppercase tracking-wide text-muted">Call</div>
                    <Stat
                      label="Service option"
                      value={mobile.trafficServiceOption != null ? serviceOptionName(mobile.trafficServiceOption) : "Unknown"}
                    />
                    <Stat label="Channel state" value={mobile.state} />
                    <Stat label="Reverse RC" value={reverseRcName ?? "-"} />
                  </div>
                  <div>
                    <div className="mb-1 flex items-center justify-between text-[11px] uppercase tracking-wide text-muted">
                      <span>Reverse target override</span>
                      <span
                        className={`normal-case tracking-normal rounded px-1.5 py-0.5 ${
                          trafficPower.manualTargetOverrideDb != null
                            ? "bg-badge-orange-bg text-badge-orange-text"
                            : "bg-surface-raised text-muted"
                        }`}
                      >
                        {trafficPower.manualTargetOverrideDb != null ? "Pinned" : "Auto"}
                      </span>
                    </div>
                    <div className="flex flex-wrap items-center gap-2">
                      <input
                        type="number"
                        step="0.1"
                        min={isRc3 ? "-20" : "0"}
                        max="40"
                        inputMode="decimal"
                        value={powerDraft}
                        onChange={(e) => setPowerDraft(e.target.value)}
                        disabled={powerOverridePending}
                        className="w-24 rounded border border-border-input bg-body px-2 py-1 text-right font-mono text-sm text-primary disabled:opacity-50"
                      />
                      <span className="text-xs text-muted">dB</span>
                      <button
                        type="button"
                        onClick={pinPowerOverride}
                        disabled={powerOverridePending}
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
                    <p className="mt-1.5 text-[11px] text-muted">
                      {trafficPower.manualTargetOverrideDb != null
                        ? `Pinned at ${trafficPower.manualTargetOverrideDb.toFixed(1)} dB. Clear to resume the outer loop.`
                        : "The outer loop sets the target from reverse FER."}
                    </p>
                    {powerOverrideError && <p className="mt-1 text-xs text-accent-red">{powerOverrideError}</p>}
                  </div>
                </div>
              </>
            ) : (
              <p className="text-sm text-muted">Traffic channel assigned, but no power-control snapshot has arrived yet.</p>
            )}
          </Card>

          {trafficPower && (
            <Card title="Reverse Loop Detail">
              <div className="grid grid-cols-2 sm:grid-cols-4 gap-3">
                <Tile label="Recent peak" sub={reverseFrameMaxPcgs.length ? `PCG ${reverseFrameMaxPcgs.join(", ")}` : "latest 20 ms"}>
                  {reverseFrameMax != null ? reverseFrameMax.toFixed(1) : "-"} <span className="text-xs font-normal text-muted">dB</span>
                </Tile>
                <Tile label="Active PCGs" sub={trafficPower.lastActivePcgMask?.length === 16 ? "exact transmit mask" : "within 10 dB of peak"}>
                  {reverseActivePcgs != null ? `${reverseActivePcgs}/16` : "-"}
                </Tile>
                <Tile label="Bad frames" sub="lifetime FQI / CRC">
                  {trafficPower.framesCrcError}
                </Tile>
                <Tile label="Pilot Ec/Io" sub="reverse finger, diagnostic">
                  {reversePilotEcIoText ?? "-"}
                </Tile>
              </div>
              <details className="mt-4">
                <summary className="cursor-pointer select-none text-xs uppercase tracking-wide text-muted">Per-PCG detail</summary>
                <div className="mt-3">
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
                        <div className="mt-1 font-mono text-sm">{db != null && Number.isFinite(db) ? `${db.toFixed(2)} dB` : "---"}</div>
                        {isRc3 && active && frameDb != null && Number.isFinite(frameDb) && (
                          <div className="mt-0.5 font-mono text-[10px] text-muted" title="Per-frame data Eb/Nt from decoded traffic">
                            Eb/Nt {frameDb.toFixed(1)}
                          </div>
                        )}
                      </div>
                    );
                  })}
                </div>
                </div>
              </details>
            </Card>
          )}
        </div>
      )}

      {activeTab === "messages" && (
        <Card
          title={`Message History (${messages.length})`}
          action={
            <button
              type="button"
              onClick={() => setMsgPaused((p) => !p)}
              className="inline-flex items-center gap-1.5 rounded-full border border-border px-2.5 py-0.5 text-[11px] text-muted hover:text-secondary"
              title={msgPaused ? "Resume live updates" : "Pause live updates"}
            >
              <span className={`h-1.5 w-1.5 rounded-full ${holdMessages ? "bg-accent-amber" : "bg-accent-green"}`} />
              {liveLabel}
            </button>
          }
        >
          <div className="mb-3 flex flex-wrap items-center gap-2">
            <div className="inline-flex rounded border border-border bg-surface p-0.5">
              {(["all", ...categories] as const).map((c) => (
                <button
                  key={c}
                  type="button"
                  onClick={() => {
                    setMsgFilter(c);
                    setMsgPage(0);
                  }}
                  className={`rounded px-2.5 py-1 text-xs transition-colors ${
                    msgFilter === c ? "bg-surface-raised text-secondary" : "text-muted hover:text-secondary"
                  }`}
                >
                  {c === "all" ? "All" : MESSAGE_CATEGORY_LABEL[c]}
                </button>
              ))}
            </div>
            <input
              type="search"
              value={msgQuery}
              onChange={(e) => {
                setMsgQuery(e.target.value);
                setMsgPage(0);
              }}
              placeholder="Search type or summary"
              className="min-w-48 flex-1 glass-input text-sm"
            />
          </div>
          {pinned.length > 0 && (
            <div className="mb-3 rounded-lg border border-border bg-surface-raised px-3 pb-1">
              <div className="flex items-center justify-between py-1.5 text-[11px] text-muted">
                <span>
                  <span className="text-secondary font-medium">Pinned</span> · stays here across pages, filters and new messages
                </span>
                <button type="button" onClick={() => setPinned([])} className="hover:text-secondary">
                  Clear
                </button>
              </div>
              {pinned.map((entry) => (
                <MessageRow key={entry.id} entry={entry} view={messageView(entry, messageCtx)} open actions={pinControl(entry)} />
              ))}
            </div>
          )}
          {heldMessageCount > 0 && (
            <button
              type="button"
              onClick={showHeldMessages}
              className="mx-auto mb-2 flex rounded-full bg-accent-blue-bg px-4 py-1 text-xs text-accent-blue shadow-sm hover:opacity-90"
            >
              ↑ {heldMessageCount} new message{heldMessageCount > 1 ? "s" : ""} · show
            </button>
          )}
          {shownMessages.length === 0 ? (
            <p className="text-dimmed text-sm">No messages since this page opened. New ones appear as they are sent and received.</p>
          ) : filteredViews.length === 0 ? (
            <p className="text-dimmed text-sm">No messages match.</p>
          ) : (
            <>
              <MessageListHeader />
              <div ref={messageListRef}>
                {pageViews.map(({ entry, view }) => {
                  const open = expanded.has(entry.id);
                  return (
                    <MessageRow
                      key={entry.id}
                      entry={entry}
                      view={view}
                      open={open}
                      onToggle={() => toggleExpand(entry.id)}
                      actions={pinControl(entry)}
                    />
                  );
                })}
              </div>
              <Pager
                page={filteredPage}
                pageSize={msgPageSize}
                total={filteredViews.length}
                pageSizes={MESSAGE_PAGE_SIZES}
                onPage={setMsgPage}
                onPageSize={(size) => {
                  setMsgPageSize(size);
                  setMsgPage(0);
                }}
              />
            </>
          )}
        </Card>
      )}

      {activeTab === "sms" && (
        <Card
          title="SMS"
          action={
            <button
              type="button"
              onClick={() => setSmsOpen(true)}
              className="text-xs px-2.5 py-1 rounded bg-accent-green-bg text-accent-green border border-accent-green/20 hover:bg-accent-green/15"
            >
              Send SMS
            </button>
          }
        >
          <SmsList phone={mobile.phoneNumber} refreshKey={smsRefresh} />
        </Card>
      )}

      {activeTab === "otasp" && (
        <RecentOtaspCard title="OTASP Sessions" esn={mobile.esn} meid={mobile.meid} />
      )}

      {activeTab === "activity" && (
        <MobileActivity
          filter={{
            imsis: mobile.imsi ? [mobile.imsi] : [],
            esn: mobile.esn ?? undefined,
            meid: mobile.meid || undefined,
          }}
        />
      )}

      {activeTab === "data" && (
        <div className="space-y-4">
          {mobilePacketSessions.length === 0 ? (
            <Card title="Data Sessions">
              <p className="text-dimmed text-sm">No active data session.</p>
            </Card>
          ) : (
            mobilePacketSessions.map((session) => (
              <Card
                key={session.sessionId}
                title={`${session.accessTechnology || "1x"} data session · SO ${session.serviceOption}`}
                action={<span className="rounded bg-surface-raised px-2 py-0.5 text-xs text-secondary">{session.phase}</span>}
              >
                <div className="grid grid-cols-2 sm:grid-cols-4 gap-3">
                  <Tile label="Mobile IP">
                    <span className="font-mono text-base">{session.peerIp || "-"}</span>
                  </Tile>
                  <Tile label="Gateway IP">
                    <span className="font-mono text-base">{session.ourIp || "-"}</span>
                  </Tile>
                  <Tile label="Last activity">
                    <span className="text-base">
                      {session.lastActivityAtMs ? new Date(session.lastActivityAtMs).toLocaleTimeString() : "-"}
                    </span>
                  </Tile>
                  <Tile label="Channel">
                    <span className="font-mono text-base">
                      {session.trafficWalshCode
                        ? session.accessTechnology === "HRPD"
                          ? `key ${session.trafficWalshCode}`
                          : `W${session.trafficWalshCode}`
                        : "-"}
                    </span>
                  </Tile>
                </div>
                <div className="mt-4 grid gap-4 md:grid-cols-3">
                  <div>
                    <div className="mb-1 text-[11px] uppercase tracking-wide text-muted">Session</div>
                    <div className="flex justify-between py-0.5">
                      <span className="text-muted text-sm">Session ID</span>
                      <Link
                        href={`/packets/${encodeURIComponent(session.sessionId)}`}
                        className="max-w-[60%] truncate font-mono text-sm text-accent-green hover:text-accent-green"
                      >
                        {session.sessionId}
                      </Link>
                    </div>
                    <Stat label="TUN" value={session.tunDevice || "-"} mono />
                  </div>
                  <div>
                    <div className="mb-1 text-[11px] uppercase tracking-wide text-muted">Identity</div>
                    <Stat label="MN ID" value={session.mnId || "-"} mono />
                    <Stat label="Source" value={session.mnIdSource || "-"} />
                  </div>
                  <div>
                    <div className="mb-1 text-[11px] uppercase tracking-wide text-muted">Capture</div>
                    <Stat label="Packet capture" value={session.captureEnabled ? "On" : "Off"} />
                    <Link
                      href={`/packets/${encodeURIComponent(session.sessionId)}`}
                      className="mt-2 inline-block text-xs px-2.5 py-1 rounded border border-border-input text-secondary hover:bg-surface-raised"
                    >
                      Open session →
                    </Link>
                  </div>
                </div>
              </Card>
            ))
          )}
        </div>
      )}

      <SendSmsDialog
        open={smsOpen}
        onClose={() => setSmsOpen(false)}
        phoneNumber={mobile.phoneNumber}
        imsi={mobile.imsi || undefined}
        onSent={(to) => {
          setSmsRefresh((k) => k + 1);
          showToast(`Message sent to ${to}`);
        }}
      />
      <Toast toast={toast} />
    </div>
  );
}
