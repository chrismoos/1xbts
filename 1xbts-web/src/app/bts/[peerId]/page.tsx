"use client";

import { use, useCallback, useEffect, useMemo, useState } from "react";
import Link from "next/link";
import { useSearchParams } from "next/navigation";
import { Card, Stat } from "@/components/card";
import { IqCaptureCard } from "@/components/iq-capture-card";
import {
  CarriersCard,
  CodeChannelsCard,
  ConfigurationSection,
  DiagnosticsSection,
  EvdoCarrierSection,
  HealthKpis,
  TransmitterSection,
  type RadioMetrics,
} from "@/components/cell-view";
import { MobilesTable, type MobileInfo } from "@/components/mobiles-table";
import { useAdmissionVerdicts } from "@/components/access-activity";
import { useLive } from "@/lib/live";
import type { GetSessionsResponse, GetUatiAllocationResponse } from "@/lib/proto/an/v1/service";
import {
  attachStateBadgeClass,
  attachStateLabel,
  cellHref,
  cellTitle,
  cellToken,
  peerHref,
  sameCell,
} from "@/lib/cell";
import { radioConfigPairName } from "@/lib/radio-config";
import { useBtsList } from "@/lib/use-bts-list";
import type { BtsConfig, TrafficChannelPower } from "@/lib/proto/bsc/v1/service";
import { CellKind, type ReversePowerControlEntry } from "@/lib/proto/bts_management/v1/service";
import type { FemtoCellStatus } from "@/lib/proto/femto_management/v1/service";
import { apiFetch } from "@/lib/api-client";
import { ErrorNotice } from "@/components/error-notice";

type ServedMobile = MobileInfo;

const CELL_MOBILE_ROWS = 5;

function formatDb(value?: number): string {
  return value != null ? `${value.toFixed(1)} dB` : "-";
}


function FemtoLocationMap({ latitude, longitude }: { latitude: number; longitude: number }) {
  const latitudeDelta = 0.008;
  const longitudeDelta = 0.012;
  const bounds = [
    longitude - longitudeDelta,
    latitude - latitudeDelta,
    longitude + longitudeDelta,
    latitude + latitudeDelta,
  ].join(",");
  const marker = `${latitude},${longitude}`;
  const mapUrl = `https://www.openstreetmap.org/export/embed.html?bbox=${encodeURIComponent(bounds)}&layer=mapnik&marker=${encodeURIComponent(marker)}`;
  const detailUrl = `https://www.openstreetmap.org/?mlat=${encodeURIComponent(latitude)}&mlon=${encodeURIComponent(longitude)}#map=15/${encodeURIComponent(latitude)}/${encodeURIComponent(longitude)}`;

  return (
    <div className="mt-4 overflow-hidden rounded-lg border border-border">
      <iframe
        title="Femtocell GPS location"
        src={mapUrl}
        className="h-64 w-full bg-surface-solid"
        loading="lazy"
      />
      <div className="border-t border-border px-3 py-2 text-right">
        <a
          href={detailUrl}
          target="_blank"
          rel="noreferrer"
          className="text-xs text-accent-green hover:text-accent-green"
        >
          Open larger map &rarr;
        </a>
      </div>
    </div>
  );
}

export default function BtsDetailPage({
  params,
}: {
  params: Promise<{ peerId: string }>;
}) {
  const { peerId: rawPeerId } = use(params);
  const peerId = decodeURIComponent(rawPeerId);
  const baseStation = useSearchParams().get("baseStation");

  const { cells } = useBtsList();
  const summary = useMemo(
    () =>
      cells.find(
        (entry) =>
          entry.peerId === peerId &&
          (!baseStation || entry.baseStation === baseStation),
      ),
    [cells, peerId, baseStation],
  );
  // The broadcast identity is present only once the peer is in service.
  const cell = summary?.cell;
  const cellTok = useMemo(() => cellToken(cell), [cell]);

  const [config, setConfig] = useState<BtsConfig | null>(null);
  const [configError, setConfigError] = useState<unknown>(null);
  const [metrics, setMetrics] = useState<RadioMetrics | null>(null);
  const [powerControls, setPowerControls] = useState<ReversePowerControlEntry[]>([]);
  const liveMobiles = useLive<ServedMobile[]>("/api/mobiles").data;
  const mobiles = useMemo(() => (Array.isArray(liveMobiles) ? liveMobiles : []), [liveMobiles]);
  const { policy } = useAdmissionVerdicts({}, 1, true);
  const evdoScope =
    summary?.evdoEnabled && summary.kind !== CellKind.CELL_KIND_FEMTOCELL ? cellToken(summary.cell) : null;
  const anSessions = useLive<GetSessionsResponse>(evdoScope ? cellHref("/api/an-sessions", evdoScope, baseStation) : null).data;
  const uatiAllocation = useLive<GetUatiAllocationResponse>(
    evdoScope ? cellHref("/api/an-uati-allocation", evdoScope, baseStation) : null,
  ).data;
  const [powerDrafts, setPowerDrafts] = useState<Record<number, string>>({});
  const [mutatingWalsh, setMutatingWalsh] = useState<number | null>(null);
  const [mutationError, setMutationError] = useState<unknown>(null);
  const [femtoStatusError, setFemtoStatusError] = useState<unknown>(null);
  const [femtoStatus, setFemtoStatus] = useState<FemtoCellStatus | null>(null);
  const [femtoCommandStatus, setFemtoCommandStatus] = useState<string | null>(null);
  const [femtoCommandPending, setFemtoCommandPending] = useState<string | null>(null);
  const isFemto = summary?.kind === CellKind.CELL_KIND_FEMTOCELL;

  useEffect(() => {
    if (!isFemto || !baseStation) return;
    const query = new URLSearchParams({ baseStation, peerId });
    const load = () =>
      void apiFetch<typeof femtoStatus>(`/api/femtocells/status?${query}`, { cache: "no-store" }, "load the femtocell status")
        .then((status) => {
          setFemtoStatus(status);
          setFemtoStatusError(null);
        })
        .catch(setFemtoStatusError);
    load();
    const interval = setInterval(load, 5000);
    return () => clearInterval(interval);
  }, [isFemto, baseStation, peerId]);

  const femtoAction = useCallback(async (action: "enable" | "disable" | "sync" | "reboot") => {
    if (!baseStation) return;
    setMutationError(null);
    setFemtoCommandStatus(null);
    setFemtoCommandPending(action);
    try {
      const result = await apiFetch<{ message?: string }>(
        "/api/femtocells/action",
        {
          method: "POST",
          headers: { "Content-Type": "application/json" },
          body: JSON.stringify({ baseStation, peerId, action }),
        },
        action === "sync" ? "start a provisioning sync" : `${action} the femtocell`,
      );
      setFemtoCommandStatus(result.message || "Command sent");
    } catch (error) {
      setMutationError(error);
    } finally {
      setFemtoCommandPending(null);
    }
  }, [baseStation, peerId]);

  useEffect(() => {
    if (!summary || isFemto) return;
    let cancelled = false;
    apiFetch<BtsConfig>(peerHref("/api/bts-config", peerId, baseStation), { cache: "no-store" }, "load the BTS config")
      .then((data) => {
        if (cancelled) return;
        setConfig(data);
        setConfigError(null);
      })
      .catch((err) => {
        if (!cancelled) {
          setConfigError(err);
          setConfig(null);
        }
      });
    return () => {
      cancelled = true;
    };
  }, [peerId, baseStation, summary, isFemto]);

  useEffect(() => {
    if (!summary || isFemto) return;
    const es = new EventSource(peerHref("/api/radio-metrics", peerId, baseStation));
    es.onmessage = (event) => {
      const data = JSON.parse(event.data);
      if (!data.error) setMetrics(data);
    };
    return () => es.close();
  }, [peerId, baseStation, summary, isFemto]);

  const loadPowerControls = useCallback(async () => {
    if (!summary || isFemto) return;
    try {
      const res = await fetch(peerHref("/api/bts/reverse-power-controls", peerId, baseStation), {
        cache: "no-store",
      });
      const data = await res.json();
      if (data.error) throw new Error(data.error);
      setPowerControls(data.entries ?? []);
    } catch {
      setPowerControls([]);
    }
  }, [peerId, baseStation, summary, isFemto]);

  useEffect(() => {
    void loadPowerControls();
    const interval = setInterval(() => void loadPowerControls(), 3000);
    return () => clearInterval(interval);
  }, [loadPowerControls]);

  const servedMobiles = useMemo(
    () => mobiles.filter((mobile) => sameCell(mobile.servingCell, cell)),
    [mobiles, cell],
  );

  const mobileByWalsh = useMemo(() => {
    const map = new Map<number, ServedMobile>();
    for (const mobile of servedMobiles) {
      if (mobile.trafficWalshCode != null) map.set(mobile.trafficWalshCode, mobile);
    }
    return map;
  }, [servedMobiles]);

  const applyOverride = useCallback(
    async (walshCode: number, payload: { targetDb?: number; clear?: boolean }) => {
      setMutatingWalsh(walshCode);
      setMutationError(null);
      try {
        const json = await apiFetch<{
          accepted?: boolean;
          message?: string;
          trafficPower?: { effectiveTargetEbNtDb?: number };
        }>(
          cellHref("/api/bts/power-override", cellTok, baseStation),
          {
            method: "POST",
            headers: { "Content-Type": "application/json" },
            body: JSON.stringify({ walshCode, ...payload }),
          },
          "change the reverse target",
        );
        if (!json.accepted) {
          throw new Error(json.message || "The BTS did not apply the change.");
        }
        setPowerDrafts((prev) => {
          const next = { ...prev };
          if (json.trafficPower?.effectiveTargetEbNtDb != null) {
            next[walshCode] = json.trafficPower.effectiveTargetEbNtDb.toFixed(1);
          } else if (payload.clear) {
            delete next[walshCode];
          }
          return next;
        });
        await loadPowerControls();
      } catch (err) {
        setMutationError(err);
      } finally {
        setMutatingWalsh((current) => (current === walshCode ? null : current));
      }
    },
    [loadPowerControls, cellTok, baseStation],
  );

  const sessions = anSessions?.sessions;
  const rejected = servedMobiles.filter((mobile) => mobile.state === "RegistrationRejected").length;
  const calls = servedMobiles.filter((mobile) => mobile.trafficWalshCode != null).length;

  return (
    <div className="page-container space-y-4">
      <div className="flex flex-wrap items-center gap-3">
        <Link href="/bts" className="text-sm text-muted hover:text-secondary">
          &larr; Cells
        </Link>
        <h1 className="text-lg font-bold">
          {summary?.name || (cell ? cellTitle(cell) : peerId)}
        </h1>
        {summary && (
          <span
            className={`text-xs px-2 py-0.5 rounded ${attachStateBadgeClass(summary.state)}`}
          >
            {attachStateLabel(summary.state)}
          </span>
        )}
        {summary?.evdoEnabled && (
          <span className="rounded-full bg-accent-purple-bg px-2 py-0.5 text-[11px] font-medium text-accent-purple">EV-DO on</span>
        )}
        {cell && summary && (
          <span className="font-mono text-xs text-muted">
            PN {summary.pilotPn} · SID {summary.sid} · NID {summary.nid}
          </span>
        )}
        {summary?.statusDetail && (
          <span className="text-xs text-muted">{summary.statusDetail}</span>
        )}
      </div>

      {summary && !cell && summary.managementEndpoint && (
        <Card title="Peer">
          <div className="grid grid-cols-1 md:grid-cols-3 gap-x-6">
            <Stat label="Peer ID" value={peerId} mono />
            <Stat label="Management Endpoint" value={summary.managementEndpoint} mono />
          </div>
        </Card>
      )}

      {!isFemto && cell && (
        <HealthKpis mobiles={servedMobiles.length} rejected={rejected} calls={calls} metrics={metrics} />
      )}

      {isFemto && femtoStatus && (
        <Card title="Femtocell">
          <div className="grid grid-cols-1 md:grid-cols-3 gap-x-6">
            <Stat label="FEID" value={femtoStatus.feid} mono />
            <Stat label="Profile" value={femtoStatus.profile} />
            <Stat label="Version" value={summary?.softwareVersion || "-"} mono />
            <Stat label="Inner Address" value={femtoStatus.innerPeerAddress || "-"} mono />
            <Stat label="SIP" value={femtoStatus.sipRegistered ? "Registered" : "Not registered"} />
            <Stat label="ACS" value={femtoStatus.acsProvisioned ? "Provisioned" : "Pending"} />
            <Stat label="GPS lock" value={femtoStatus.gpsStatus || "Unknown"} />
            <Stat
              label="Radio timing synchronization"
              value={femtoStatus.timingStatus || "Unknown"}
            />
            <Stat
              label="GPS coordinates"
              value={
                femtoStatus.gpsLatitude != null && femtoStatus.gpsLongitude != null
                  ? `${femtoStatus.gpsLatitude.toFixed(6)}, ${femtoStatus.gpsLongitude.toFixed(6)}`
                  : "Unavailable"
              }
              mono
            />
            <Stat label="Radio" value={femtoStatus.radioOperationalState || "-"} />
          </div>
          {femtoStatus.gpsLatitude != null && femtoStatus.gpsLongitude != null && (
            <FemtoLocationMap
              latitude={femtoStatus.gpsLatitude}
              longitude={femtoStatus.gpsLongitude}
            />
          )}
          <ErrorNotice error={femtoStatusError} className="mt-3" />
          <ErrorNotice error={mutationError} className="mt-3" />
          {femtoCommandStatus && (
            <p className="mt-3 rounded border border-accent-green/30 bg-accent-green/10 px-3 py-2 text-sm text-accent-green">
              {femtoCommandStatus}
            </p>
          )}
          <div className="flex flex-wrap gap-2 mt-4">
            {(["sync", "enable", "disable", "reboot"] as const).map((action) => (
              <button
                key={action}
                className="rounded border border-border px-3 py-1.5 text-xs text-secondary hover:border-accent-green hover:text-accent-green"
                disabled={femtoCommandPending !== null}
                onClick={() => void femtoAction(action)}
              >
                {femtoCommandPending === action
                  ? "Sending..."
                  : action === "sync"
                    ? "Force provisioning sync"
                    : `${action[0].toUpperCase()}${action.slice(1)} femtocell`}
              </button>
            ))}
          </div>
        </Card>
      )}

      <Card
        title={`Mobiles on this cell (${servedMobiles.length})`}
        action={<span className="text-[11px] text-muted">by last heard</span>}
      >
        {servedMobiles.length === 0 ? (
          <p className="text-dimmed text-sm">No mobiles on this cell.</p>
        ) : (
          <MobilesTable
            mobiles={[...servedMobiles]
              .sort((a, b) => (b.lastHeardMs ?? 0) - (a.lastHeardMs ?? 0))
              .slice(0, CELL_MOBILE_ROWS)}
            cells={cells}
            closed={policy?.accessMode === "closed"}
            hideCell
          />
        )}
        <div className="mt-2 flex items-center justify-between border-t border-border pt-2">
          <Link href="/mobiles" className="text-xs text-accent-green hover:text-accent-green">
            All mobiles &rarr;
          </Link>
          {servedMobiles.length > CELL_MOBILE_ROWS && (
            <span className="text-[11px] text-dimmed">
              Showing {CELL_MOBILE_ROWS} of {servedMobiles.length}
            </span>
          )}
        </div>
      </Card>

      {!isFemto && powerControls.length > 0 && (
        <Card title={`Reverse power control (${powerControls.length})`}>
          <ErrorNotice error={mutationError} className="mb-2" />
          <div className="overflow-x-auto">
            <table className="data-table">
              <thead>
                <tr>
                  <th>Walsh</th>
                  <th>Mobile</th>
                  <th>Target</th>
                  <th>Override</th>
                  <th className="text-right">FER</th>
                  <th className="text-right">Frames</th>
                </tr>
              </thead>
              <tbody>
                {powerControls.map((entry) => (
                  <PowerControlRow
                    key={entry.walshCode}
                    walshCode={entry.walshCode}
                    power={entry.power}
                    mobile={mobileByWalsh.get(entry.walshCode)}
                    draft={powerDrafts[entry.walshCode]}
                    mutating={mutatingWalsh === entry.walshCode}
                    onDraftChange={(value) =>
                      setPowerDrafts((prev) => ({ ...prev, [entry.walshCode]: value }))
                    }
                    onPin={(targetDb) => void applyOverride(entry.walshCode, { targetDb })}
                    onClear={() => void applyOverride(entry.walshCode, { clear: true })}
                    onInvalid={() =>
                      setMutationError(`The reverse target for W${entry.walshCode} is not a number.`)
                    }
                  />
                ))}
              </tbody>
            </table>
          </div>
        </Card>
      )}

      <ErrorNotice error={configError} boxed />

      {!isFemto && config && (
        <>
          <div className="grid grid-cols-1 gap-4 lg:grid-cols-2">
            <CarriersCard config={config} />
            <CodeChannelsCard config={config} />
          </div>
          <div className="space-y-2.5">
            <ConfigurationSection config={config} softwareVersion={summary?.softwareVersion} />
            <TransmitterSection config={config} />
            <EvdoCarrierSection
              evdo={config.evdo}
              sessions={sessions ? sessions.length : null}
              connected={sessions ? sessions.filter((session) => session.connected).length : null}
              uati={uatiAllocation?.capacity ? { inUse: uatiAllocation.inUse, capacity: uatiAllocation.capacity } : null}
              sessionsHref="/evdo"
            />
            <DiagnosticsSection metrics={metrics} />
            <IqCaptureCard peerId={peerId} />
          </div>
        </>
      )}
    </div>
  );
}

function PowerControlRow({
  walshCode,
  power,
  mobile,
  draft,
  mutating,
  onDraftChange,
  onPin,
  onClear,
  onInvalid,
}: {
  walshCode: number;
  power?: TrafficChannelPower;
  mobile?: ServedMobile;
  draft?: string;
  mutating: boolean;
  onDraftChange: (value: string) => void;
  onPin: (targetDb: number) => void;
  onClear: () => void;
  onInvalid: () => void;
}) {
  const isRc3 = power?.reverseRadioConfig === 3;
  const metricLabel = isRc3 ? "Pilot SINR" : "Eb/Nt";
  const draftValue = draft ?? power?.effectiveTargetEbNtDb.toFixed(1) ?? "";
  const radioConfigs = radioConfigPairName(
    power?.forwardRadioConfig,
    power?.reverseRadioConfig,
  );

  return (
    <tr>
      <td className="font-mono text-primary">
        <div>W{walshCode}</div>
        {radioConfigs && <div className="text-[11px] text-muted">{radioConfigs}</div>}
      </td>
      <td>
        {mobile ? (
          <Link
            href={`/mobiles/${encodeURIComponent(mobile.address)}`}
            className="text-accent-green hover:text-accent-green transition-colors"
          >
            {mobile.phoneNumber || mobile.subscriberDisplayName || mobile.address}
          </Link>
        ) : (
          <span className="text-dimmed">-</span>
        )}
      </td>
      <td className="text-secondary">
        {power ? (
          <div className="leading-5">
            <div className="text-[10px] uppercase tracking-wide text-muted">
              {metricLabel}
            </div>
            <div className="font-mono text-primary">
              eff {formatDb(power.effectiveTargetEbNtDb)}
            </div>
            <div className="font-mono text-muted">
              auto {formatDb(power.targetEbNtDb)}
            </div>
            <span
              className={`inline-flex px-1.5 py-0.5 rounded text-[10px] ${
                power.manualTargetOverrideDb != null
                  ? "bg-badge-orange-bg text-badge-orange-text"
                  : "bg-surface-raised text-muted"
              }`}
            >
              {power.manualTargetOverrideDb != null ? "Pinned" : "Auto"}
            </span>
          </div>
        ) : (
          <span className="text-dimmed">-</span>
        )}
      </td>
      <td>
        {power ? (
          <div className="flex items-center gap-2">
            <input
              type="number"
              step="0.1"
              min={isRc3 ? "-20" : "0"}
              max={isRc3 ? "40" : "20"}
              inputMode="decimal"
              value={draftValue}
              onChange={(event) => onDraftChange(event.target.value)}
              disabled={mutating}
              className="w-20 rounded border border-border-input bg-surface-solid px-2 py-1 text-right font-mono text-xs text-primary disabled:opacity-50"
            />
            <button
              type="button"
              onClick={() => {
                const targetDb = Number(draftValue.trim());
                if (!Number.isFinite(targetDb)) {
                  onInvalid();
                  return;
                }
                onPin(targetDb);
              }}
              disabled={mutating}
              className="rounded border border-accent-green/30 px-2 py-1 text-[11px] text-accent-green hover:bg-accent-green-bg disabled:opacity-50"
            >
              Pin
            </button>
            <button
              type="button"
              onClick={onClear}
              disabled={mutating || power.manualTargetOverrideDb == null}
              className="rounded border border-border-input px-2 py-1 text-[11px] text-secondary hover:bg-surface-raised disabled:opacity-50"
            >
              Clear
            </button>
          </div>
        ) : (
          <span className="text-dimmed">-</span>
        )}
      </td>
      <td className="text-right font-mono text-secondary">
        {power ? `${power.ferPct.toFixed(1)}%` : "-"}
      </td>
      <td className="text-right font-mono text-muted">
        {power ? `${power.framesCrcError}/${power.framesTotal}` : "-"}
      </td>
    </tr>
  );
}
