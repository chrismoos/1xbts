"use client";

import { useCallback, useEffect, useState } from "react";
import { Card, Stat } from "@/components/card";
import { peerHref } from "@/lib/cell";

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
  const [error, setError] = useState<string | null>(null);

  const endpoint = peerHref("/api/iq-capture", peerId);

  const refresh = useCallback(async () => {
    try {
      const res = await fetch(endpoint, { cache: "no-store" });
      const data = await res.json();
      if (!res.ok || data.error) {
        throw new Error(data.error || `HTTP ${res.status}`);
      }
      setCapture(data);
      setError(null);
    } catch (err) {
      setError(err instanceof Error ? err.message : "unknown error");
    }
  }, [endpoint]);

  const mutate = useCallback(
    async (method: "POST" | "DELETE") => {
      setBusy(true);
      try {
        const res = await fetch(endpoint, { method });
        const data = await res.json();
        if (!res.ok || data.error) {
          throw new Error(data.error || `HTTP ${res.status}`);
        }
        setCapture(data);
        setError(null);
      } catch (err) {
        setError(err instanceof Error ? err.message : "unknown error");
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

  return (
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
            onClick={() => void mutate("POST")}
            disabled={busy || !!capture?.active}
            className="rounded border border-accent-green/20 bg-accent-green-bg px-3 py-1.5 text-sm text-accent-green disabled:opacity-50"
          >
            Start Capture
          </button>
          <button
            type="button"
            onClick={() => void mutate("DELETE")}
            disabled={busy || !capture?.active}
            className="rounded border border-accent-red/20 bg-accent-red-bg px-3 py-1.5 text-sm text-accent-red disabled:opacity-50"
          >
            Stop Capture
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

        {error && (
          <div className="rounded border border-accent-amber/20 bg-accent-amber-bg px-3 py-2 text-sm text-accent-amber">
            {error}
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
  );
}
