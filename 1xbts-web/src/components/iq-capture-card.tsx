"use client";

import { useCallback, useEffect, useState } from "react";
import { Stat } from "@/components/card";
import { Fold } from "@/components/cell-view";
import { peerHref } from "@/lib/cell";
import { apiFetch } from "@/lib/api-client";
import { ErrorNotice } from "@/components/error-notice";

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

export function IqCaptureCard({ peerId }: { peerId?: string | null }) {
  const [capture, setCapture] = useState<IqCaptureStatus | null>(null);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<unknown>(null);

  const endpoint = peerHref("/api/iq-capture", peerId);

  const refresh = useCallback(async () => {
    try {
      const data = await apiFetch<IqCaptureStatus>(endpoint, { cache: "no-store" }, "load the IQ capture status");
      setCapture(data);
      setError(null);
    } catch (err) {
      setError(err);
    }
  }, [endpoint]);

  const mutate = useCallback(
    async (method: "POST" | "DELETE") => {
      setBusy(true);
      try {
        const data = await apiFetch<IqCaptureStatus>(
          endpoint,
          { method },
          method === "POST" ? "start the IQ capture" : "stop the IQ capture",
        );
        setCapture(data);
        setError(null);
      } catch (err) {
        setError(err);
      } finally {
        setBusy(false);
      }
    },
    [endpoint],
  );

  useEffect(() => {
    refresh();
    const interval = setInterval(refresh, 5000);
    return () => clearInterval(interval);
  }, [refresh]);

  const rateMsps = capture ? (capture.sampleRateHz / 1e6).toFixed(4) : null;
  return (
    <Fold
      title="IQ capture"
      summary={
        <span
          className={`rounded-full px-2 py-0.5 text-[11px] font-medium ${
            capture?.active ? "bg-badge-red-bg text-badge-red-text" : "bg-surface-raised text-muted"
          }`}
        >
          {capture?.active ? `Capturing · ${capture.capturedSeconds.toFixed(1)} s` : "Idle"}
        </span>
      }
      defaultOpen={!!capture?.active}
    >
      <div className="space-y-4">
        <div className="flex flex-wrap items-center gap-2">
          <button
            type="button"
            onClick={() => void mutate("POST")}
            disabled={busy || !!capture?.active}
            className="rounded border border-accent-green/20 bg-accent-green-bg px-3 py-1.5 text-sm text-accent-green disabled:opacity-50"
          >
            Start capture
          </button>
          <button
            type="button"
            onClick={() => void mutate("DELETE")}
            disabled={busy || !capture?.active}
            className="rounded border border-accent-red/20 bg-accent-red-bg px-3 py-1.5 text-sm text-accent-red disabled:opacity-50"
          >
            Stop capture
          </button>
          <button
            type="button"
            onClick={() => void refresh()}
            disabled={busy}
            className="rounded border border-border-input bg-surface-solid px-3 py-1.5 text-sm text-secondary disabled:opacity-50"
          >
            Refresh
          </button>
        </div>

        <ErrorNotice error={error} boxed />

        {capture ? (
          <div className="grid grid-cols-1 gap-6 md:grid-cols-3">
            <div>
              <h4 className="mb-1.5 text-xs font-semibold text-primary">Format</h4>
              <Stat label="Sample rate" value={`${rateMsps} Msps`} mono />
              <Stat label="Chip rate" value={`${(capture.chipRateHz / 1e6).toFixed(4)} Mcps`} mono />
              <Stat label="Directory" value={capture.directory} mono />
            </div>
            <div>
              <h4 className="mb-1.5 text-xs font-semibold text-primary">Current capture</h4>
              <Stat label="Samples" value={capture.capturedSamples.toLocaleString()} mono />
              <Stat label="Duration" value={`${capture.capturedSeconds.toFixed(1)} s`} mono />
              <Stat label="First sample" value={capture.firstSampleSystemTime || "—"} mono />
              <Stat
                label="First chip"
                value={capture.firstAbsoluteChipStart !== undefined ? String(capture.firstAbsoluteChipStart) : "—"}
                mono
              />
              <Stat
                label="Hardware time"
                value={capture.firstHardwareTimeNs !== undefined ? `${capture.firstHardwareTimeNs} ns` : "—"}
                mono
              />
            </div>
            <div>
              <h4 className="mb-1.5 text-xs font-semibold text-primary">Files</h4>
              {capture.wavPath || capture.metadataPath ? (
                <div className="space-y-2 text-xs">
                  <div>
                    <div className="text-muted">WAV</div>
                    <div className="break-all font-mono text-secondary">{capture.wavPath || "—"}</div>
                  </div>
                  <div>
                    <div className="text-muted">Metadata</div>
                    <div className="break-all font-mono text-secondary">{capture.metadataPath || "—"}</div>
                  </div>
                </div>
              ) : (
                <p className="text-sm text-dimmed">No capture yet. The WAV and its metadata file appear here once one runs.</p>
              )}
            </div>
          </div>
        ) : (
          <p className="text-dimmed text-sm">Capture status unavailable</p>
        )}
      </div>
    </Fold>
  );
}
