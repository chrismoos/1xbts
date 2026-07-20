"use client";

import { useCallback, useEffect, useState } from "react";
import { Card, Stat } from "@/components/card";

interface TxMetrics {
  rtRatio: number | null;
  chipCursor: number | null;
  blocksTransmitted: number | null;
  genAvgUs: number | null;
  genMaxUs: number | null;
  txAvgUs: number | null;
  txMaxUs: number | null;
  synthPilotUs: number | null;
  synthSyncUs: number | null;
  synthPagingUs: number | null;
  synthSpreadUs: number | null;
  syncFragmentsSent: number | null;
  pagingFragmentsSent: number | null;
}

interface RxMetrics {
  rtRatio: number | null;
  reads: number | null;
  samples: number | null;
  captureUs: number | null;
  pipelineUs: number | null;
  totalUs: number | null;
  totalMaxUs: number | null;
  deficitMs?: number | null;
}

interface RadioMetrics {
  tx?: TxMetrics;
  rx?: RxMetrics;
}

interface IqCaptureStatus {
  active: boolean;
  directory: string;
  wavPath?: string;
  metadataPath?: string;
  firstAbsoluteChipStart?: number;
  firstSampleSystemTime?: string;
  firstHardwareTimeNs?: number;
  capturedSamples: number;
  capturedSeconds: number;
  sampleRateHz: number;
  chipRateHz: number;
}

function formatFixed(value: number | null | undefined, digits: number, suffix = ""): string {
  return typeof value === "number" ? `${value.toFixed(digits)}${suffix}` : "Unavailable";
}

function formatInteger(value: number | null | undefined, suffix = ""): string {
  return typeof value === "number" ? `${value}${suffix}` : "Unavailable";
}

export default function RadioPage() {
  const [metrics, setMetrics] = useState<RadioMetrics | null>(null);
  const [capture, setCapture] = useState<IqCaptureStatus | null>(null);
  const [captureBusy, setCaptureBusy] = useState(false);
  const [captureError, setCaptureError] = useState<string | null>(null);

  const refreshCapture = useCallback(async () => {
    try {
      const res = await fetch("/api/iq-capture", { cache: "no-store" });
      const data = await res.json();
      if (!res.ok || data.error) {
        throw new Error(data.error || `HTTP ${res.status}`);
      }
      setCapture(data);
      setCaptureError(null);
    } catch (err) {
      setCaptureError(err instanceof Error ? err.message : "unknown error");
    }
  }, []);

  const mutateCapture = useCallback(
    async (method: "POST" | "DELETE") => {
      setCaptureBusy(true);
      try {
        const res = await fetch("/api/iq-capture", { method });
        const data = await res.json();
        if (!res.ok || data.error) {
          throw new Error(data.error || `HTTP ${res.status}`);
        }
        setCapture(data);
        setCaptureError(null);
      } catch (err) {
        setCaptureError(err instanceof Error ? err.message : "unknown error");
      } finally {
        setCaptureBusy(false);
      }
    },
    []
  );

  useEffect(() => {
    const es = new EventSource("/api/radio-metrics");
    es.onmessage = (e) => {
      const data = JSON.parse(e.data);
      if (!data.error) setMetrics(data);
    };
    return () => es.close();
  }, []);

  useEffect(() => {
    refreshCapture();
    const interval = setInterval(refreshCapture, 5000);
    return () => clearInterval(interval);
  }, [refreshCapture]);

  return (
    <div className="max-w-7xl mx-auto space-y-6">
      <h1 className="text-lg font-bold">Radio & RF Metrics</h1>

      <Card title="IQ Capture">
        <div className="space-y-4">
          <div className="flex flex-wrap items-center gap-3">
            <span
              className={`text-xs px-2 py-0.5 rounded ${
                capture?.active
                  ? "bg-badge-red-bg text-badge-red-text"
                  : "bg-surface-raised text-secondary"
              }`}
            >
              {capture?.active ? "Capturing" : "Idle"}
            </span>
            <button
              type="button"
              onClick={() => void mutateCapture("POST")}
              disabled={captureBusy || !!capture?.active}
              className="rounded border border-accent-green/20 bg-accent-green-bg px-3 py-1.5 text-sm text-accent-green disabled:opacity-50"
            >
              Start Capture
            </button>
            <button
              type="button"
              onClick={() => void mutateCapture("DELETE")}
              disabled={captureBusy || !capture?.active}
              className="rounded border border-accent-red/20 bg-accent-red-bg px-3 py-1.5 text-sm text-accent-red disabled:opacity-50"
            >
              Stop Capture
            </button>
            <button
              type="button"
              onClick={() => void refreshCapture()}
              disabled={captureBusy}
              className="rounded border border-border-input bg-surface-solid px-3 py-1.5 text-sm text-secondary disabled:opacity-50"
            >
              Refresh
            </button>
          </div>

          {captureError && (
            <div className="rounded border border-accent-amber/20 bg-accent-amber-bg px-3 py-2 text-sm text-accent-amber">
              {captureError}
            </div>
          )}

          {capture ? (
            <div className="grid grid-cols-1 md:grid-cols-2 gap-4">
              <div className="space-y-2 text-sm">
                <Stat label="Directory" value={capture.directory} />
                <Stat label="Sample Rate" value={`${capture.sampleRateHz} Hz`} />
                <Stat label="Chip Rate" value={`${capture.chipRateHz} Hz`} />
                <Stat label="Captured Samples" value={String(capture.capturedSamples)} />
                <Stat label="Captured Seconds" value={capture.capturedSeconds.toFixed(3)} />
              </div>
              <div className="space-y-2 text-sm">
                <Stat
                  label="First Sample Time"
                  value={capture.firstSampleSystemTime || "Unavailable"}
                />
                <Stat
                  label="First Chip"
                  value={
                    capture.firstAbsoluteChipStart !== undefined
                      ? String(capture.firstAbsoluteChipStart)
                      : "Unavailable"
                  }
                />
                <Stat
                  label="Hardware Time"
                  value={
                    capture.firstHardwareTimeNs !== undefined
                      ? `${capture.firstHardwareTimeNs} ns`
                      : "Unavailable"
                  }
                />
              </div>
              <div className="md:col-span-2 space-y-2 text-sm">
                <div>
                  <div className="text-muted text-xs uppercase tracking-wide">WAV Path</div>
                  <div className="text-secondary break-all font-mono text-xs">
                    {capture.wavPath || "No capture file yet"}
                  </div>
                </div>
                <div>
                  <div className="text-muted text-xs uppercase tracking-wide">Metadata Path</div>
                  <div className="text-secondary break-all font-mono text-xs">
                    {capture.metadataPath || "No metadata file yet"}
                  </div>
                </div>
              </div>
            </div>
          ) : (
            <p className="text-dimmed text-sm">Capture status unavailable</p>
          )}
        </div>
      </Card>

      <div className="grid grid-cols-1 md:grid-cols-2 gap-4">
        <Card title="TX Performance">
          {metrics?.tx ? (
            <>
              <Stat label="RT Ratio" value={formatFixed(metrics.tx.rtRatio, 1, "x")} />
              <Stat label="Blocks Transmitted" value={formatInteger(metrics.tx.blocksTransmitted)} />
              <Stat label="Gen Avg" value={formatInteger(metrics.tx.genAvgUs, " us")} />
              <Stat label="Gen Max" value={formatInteger(metrics.tx.genMaxUs, " us")} />
              <Stat label="TX Avg" value={formatInteger(metrics.tx.txAvgUs, " us")} />
              <Stat label="TX Max" value={formatInteger(metrics.tx.txMaxUs, " us")} />
            </>
          ) : (
            <p className="text-dimmed text-sm">Unavailable</p>
          )}
        </Card>

        <Card title="TX Synthesis Breakdown">
          {metrics?.tx ? (
            <>
              <Stat label="Pilot" value={formatInteger(metrics.tx.synthPilotUs, " us")} />
              <Stat label="Sync" value={formatInteger(metrics.tx.synthSyncUs, " us")} />
              <Stat label="Paging" value={formatInteger(metrics.tx.synthPagingUs, " us")} />
              <Stat label="Spreading" value={formatInteger(metrics.tx.synthSpreadUs, " us")} />
            </>
          ) : (
            <p className="text-dimmed text-sm">Unavailable</p>
          )}
        </Card>

        <Card title="RX Pipeline">
          {metrics?.rx ? (
            <>
              <Stat label="RT Ratio" value={formatFixed(metrics.rx.rtRatio, 2, "x")} />
              <Stat label="Reads/s" value={formatInteger(metrics.rx.reads)} />
              <Stat label="Samples/s" value={formatInteger(metrics.rx.samples)} />
              <Stat label="Capture" value={formatInteger(metrics.rx.captureUs, " us")} />
              <Stat label="Pipeline" value={formatInteger(metrics.rx.pipelineUs, " us")} />
              <Stat label="Total" value={formatInteger(metrics.rx.totalUs, " us")} />
              <Stat label="Total Max" value={formatInteger(metrics.rx.totalMaxUs, " us")} />
              {metrics.rx.deficitMs != null && (
                <Stat label="Deficit" value={formatFixed(metrics.rx.deficitMs, 1, " ms")} />
              )}
            </>
          ) : (
            <p className="text-dimmed text-sm">No RX active</p>
          )}
        </Card>
      </div>
    </div>
  );
}
