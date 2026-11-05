"use client";

import { useCallback, useMemo, useState } from "react";
import Link from "next/link";
import { EvdoChip, EvdoLinkChip, EvdoSnrBar, EvdoVerdictChip } from "@/components/evdo";
import { reasonLabel } from "@/lib/admission";
import { cellForColorCode, cellLabel, peerIdForCell } from "@/lib/cell";
import {
  a10ForEvdo,
  drcRateLabel,
  evdoIdentity,
  parseA10Sessions,
  evdoLinkState,
  evdoVerdict,
  formatAge,
  packetSessionForEvdo,
  type EvdoIdentity,
  type EvdoLinkState,
  type EvdoPacketSession,
} from "@/lib/evdo";
import { formatHrpdFullUati, isHrpdTelemetryTrafficEvent, uatiHex, uatiHexDigits } from "@/lib/hrpd-correlation";
import { useLive } from "@/lib/live";
import { useMobileDirectory, type MobileDirectoryEntry } from "@/lib/mobile-directory";
import { GetSessionsResponse, type Session } from "@/lib/proto/an/v1/service";
import { HrpdTrafficEvent } from "@/lib/proto/events/v1/an";
import { useBtsList } from "@/lib/use-bts-list";
import { useEventStream, type EventStreamOptions } from "@/lib/use-event-stream";

const REPLAY_EVENTS: EventStreamOptions = { replay: true };
const UATI_24_MASK = 0x00ff_ffff;
const SNR_DB_PER_TENTH = 10;

const AUTH_RESULT_LABELS: Record<string, string> = {
  accepted: "AN-AAA accepted",
  rejected: "AN-AAA rejected",
  aaa_error: "AN-AAA error",
  aaa_timeout: "AN-AAA timeout",
  chap_refused: "CHAP refused",
  no_response: "No AN-AAA response",
  peer_terminated: "Closed by handset",
};

const LINK_ORDER: Record<EvdoLinkState, number> = { connected: 0, dormant: 1, idle: 2 };

export interface EvdoSessionRow {
  session: Session;
  packet?: EvdoPacketSession;
  identity: EvdoIdentity;
  link: EvdoLinkState;
  refused: boolean;
  snrDb?: number;
}

function isOneXMobile(mobile?: MobileDirectoryEntry): mobile is MobileDirectoryEntry {
  return Boolean(mobile && !mobile.address.startsWith("evdo-"));
}

export function useEvdoSessionRows(sessionsKey: string | null) {
  const [snrByUati, setSnrByUati] = useState<Map<number, number>>(new Map());
  const mobiles = useMobileDirectory();
  const liveSessions = useLive<unknown>(sessionsKey);
  const livePackets = useLive<{ sessions?: EvdoPacketSession[] }>(sessionsKey ? "/api/packet-sessions" : null);
  const liveA10 = useLive<unknown>(sessionsKey ? "/api/pcf-a10-sessions" : null);
  const a10s = useMemo(() => parseA10Sessions(liveA10.data), [liveA10.data]);
  const raw = liveSessions.data as { error?: string } | undefined;
  const error = liveSessions.errorBody ?? liveSessions.error ?? raw?.error ?? null;
  const sessions = useMemo(
    () => (raw && !raw.error ? (GetSessionsResponse.fromJSON(raw).sessions ?? []) : []),
    [raw],
  );
  const packets = useMemo(() => livePackets.data?.sessions ?? [], [livePackets.data]);

  useEventStream(
    "hrpd-traffic",
    useCallback((data: string) => {
      const event = HrpdTrafficEvent.fromJSON(JSON.parse(data));
      if (!isHrpdTelemetryTrafficEvent(event) || event.reversePilotSnrDbTenths === 0) return;
      setSnrByUati((prev) => new Map(prev).set(event.uati & UATI_24_MASK, event.reversePilotSnrDbTenths / SNR_DB_PER_TENTH));
    }, []),
    REPLAY_EVENTS,
  );

  const rows: EvdoSessionRow[] = useMemo(
    () =>
      sessions
        .map((session) => {
          const packet = packetSessionForEvdo(session, packets, a10s);
          return {
            session,
            packet,
            identity: evdoIdentity(session, mobiles, packet, a10ForEvdo(session, a10s)),
            link: evdoLinkState(session.connected, packet?.phase),
            refused: evdoVerdict(session) === "refused",
            snrDb: session.connected ? snrByUati.get(session.uati & UATI_24_MASK) : undefined,
          };
        })
        .sort(
          (left, right) =>
            LINK_ORDER[left.link] - LINK_ORDER[right.link] ||
            (right.session.lastActivityAtMs ?? 0) - (left.session.lastActivityAtMs ?? 0),
        ),
    [sessions, packets, a10s, mobiles, snrByUati],
  );

  return { rows, sessions, error, loading: liveSessions.loading };
}

function SessionIdentity({ row, compact }: { row: EvdoSessionRow; compact?: boolean }) {
  const { identity } = row;
  const mobile = identity.mobile;
  const name = mobile?.subscriberDisplayName || mobile?.phoneNumber;
  const ids = compact
    ? []
    : [
        identity.hardwareId ? `${identity.hardwareId.label} ${identity.hardwareId.value}` : null,
        identity.imsi ? `IMSI ${identity.imsi}` : null,
      ].filter((value): value is string => value != null);
  const heading =
    name && mobile ? (
      <Link href={`/mobiles/${encodeURIComponent(mobile.address)}`} className="font-semibold text-primary hover:underline">
        {name}
      </Link>
    ) : identity.hardwareId ? (
      <span className="font-semibold text-accent-amber">Unknown device</span>
    ) : (
      <span className="font-semibold text-muted">Identifying…</span>
    );
  const detail =
    name && mobile?.phoneNumber && name !== mobile.phoneNumber
      ? mobile.phoneNumber
      : !name && identity.hardwareId
        ? compact
          ? `${identity.hardwareId.label} ${identity.hardwareId.value}`
          : "Hardware ID not provisioned"
        : !identity.hardwareId
          ? "No Hardware ID yet"
          : null;
  return (
    <div>
      {heading}
      {detail && <div className="text-xs text-muted">{detail}</div>}
      {ids.map((id) => (
        <div key={id} className="font-mono text-[11px] text-dimmed">
          {id}
        </div>
      ))}
    </div>
  );
}

function SessionRowView({
  row,
  cells,
  compact,
}: {
  row: EvdoSessionRow;
  cells: ReturnType<typeof useBtsList>["cells"];
  compact?: boolean;
}) {
  const { session, packet, identity } = row;
  const mobile = identity.mobile;
  const rate = session.connected ? drcRateLabel(session.drcIndex) : undefined;
  const admission = session.admission;
  const authResult = admission?.accessAuthResult ? AUTH_RESULT_LABELS[admission.accessAuthResult] : undefined;
  const sessionHref = `/evdo/${uatiHexDigits(session.uati)}`;
  const linkCell = (
    <td>
      <Link href={sessionHref}>
        <EvdoLinkChip state={row.link} />
      </Link>
      {!compact && session.connected && session.macIndex != null && (
        <div className="mt-1 text-xs text-muted">MAC {session.macIndex}</div>
      )}
      {compact && row.refused && <div className="mt-1"><EvdoVerdictChip verdict="refused" /></div>}
    </td>
  );
  const rateCell = (
    <td>
      <div className="font-mono text-xs whitespace-nowrap">{rate ?? "—"}</div>
      {session.connected && <EvdoSnrBar snrDb={row.snrDb} width={compact ? 48 : 70} />}
    </td>
  );
  if (compact) {
    return (
      <tr className="align-top">
        <td>
          <SessionIdentity row={row} compact />
        </td>
        {linkCell}
        {rateCell}
      </tr>
    );
  }
  const servingCell = cellForColorCode(cells, session.colorCode);
  const servingPeerId = peerIdForCell(cells, servingCell);
  return (
    <tr className="align-top">
      <td>
        <SessionIdentity row={row} />
      </td>
      {linkCell}
      {rateCell}
      <td>
        {packet ? (
          <Link href={`/packets/${encodeURIComponent(packet.sessionId)}`} className="hover:underline">
            <EvdoChip
              label={`A10 ${packet.phase}`}
              className={packet.phase === "active" ? "text-accent-green bg-accent-green-bg" : "text-accent-amber bg-accent-amber-bg"}
            />
            {packet.trafficWalshCode ? (
              <div className="mt-0.5 font-mono text-[11px] text-dimmed">key {packet.trafficWalshCode}</div>
            ) : null}
          </Link>
        ) : (
          <span className="text-xs text-dimmed">—</span>
        )}
      </td>
      <td>
        {isOneXMobile(mobile) ? (
          <Link href={`/mobiles/${encodeURIComponent(mobile.address)}`}>
            <EvdoChip
              label={`1x · ${mobile.trafficWalshCode != null ? `W${mobile.trafficWalshCode}` : (mobile.state ?? "registered")}`}
              className="text-accent-blue bg-accent-blue-bg"
            />
          </Link>
        ) : (
          <span className="text-xs text-dimmed">—</span>
        )}
      </td>
      <td>
        <EvdoVerdictChip verdict={evdoVerdict(session)} />
        {row.refused && admission?.reason && <div className="mt-0.5 text-xs text-muted">{reasonLabel(admission.reason)}</div>}
        {authResult && <div className="mt-0.5 text-xs text-muted">{authResult}</div>}
      </td>
      <td>
        <Link href={sessionHref} className="font-mono text-sm text-accent-blue hover:underline" title={formatHrpdFullUati(session.fullUati)}>
          {uatiHex(session.uati)}
        </Link>
        <div className="text-xs text-dimmed">
          color {session.colorCode}
          {servingPeerId ? (
            <>
              {" · "}
              <Link href={`/bts/${encodeURIComponent(servingPeerId)}`} className="text-accent-cyan hover:underline">
                {cellLabel(servingCell)}
              </Link>
            </>
          ) : null}
        </div>
      </td>
      <td className="text-muted">{formatAge(session.lastActivityAtMs)}</td>
    </tr>
  );
}

const FULL_COLUMNS = ["Subscriber", "Link", "Forward rate · SNR", "Packet data", "Also on 1x", "Access", "UATI", "Seen"];
const COMPACT_COLUMNS = ["Subscriber", "Link", "Rate · SNR"];

export function EvdoSessionTable({
  rows,
  compact,
  emptyLabel = "No sessions",
}: {
  rows: EvdoSessionRow[];
  compact?: boolean;
  emptyLabel?: string;
}) {
  const { cells } = useBtsList();
  const columns = compact ? COMPACT_COLUMNS : FULL_COLUMNS;
  return (
    <table className="data-table">
      <thead>
        <tr>
          {columns.map((column) => (
            <th key={column} >
              {column}
            </th>
          ))}
        </tr>
      </thead>
      <tbody>
        {rows.length === 0 && (
          <tr>
            <td className="text-muted" colSpan={columns.length}>
              {emptyLabel}
            </td>
          </tr>
        )}
        {rows.map((row) => (
          <SessionRowView key={row.session.uati} row={row} cells={cells} compact={compact} />
        ))}
      </tbody>
    </table>
  );
}
