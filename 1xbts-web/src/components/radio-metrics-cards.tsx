import { Card, Stat } from "@/components/card";

export interface TxMetrics {
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

export interface RxMetrics {
  rtRatio: number | null;
  reads: number | null;
  samples: number | null;
  captureUs: number | null;
  pipelineUs: number | null;
  totalUs: number | null;
  totalMaxUs: number | null;
  deficitMs?: number | null;
}

export interface RadioMetrics {
  tx?: TxMetrics;
  rx?: RxMetrics;
}

function formatFixed(value: number | null | undefined, digits: number, suffix = ""): string {
  return typeof value === "number" ? `${value.toFixed(digits)}${suffix}` : "Unavailable";
}

function formatInteger(value: number | null | undefined, suffix = ""): string {
  return typeof value === "number" ? `${value}${suffix}` : "Unavailable";
}

// TX/RX metric cards for one cell. Pure (no hooks), so a server component can
// render it.
export function RadioMetricsCards({ metrics }: { metrics: RadioMetrics | null }) {
  return (
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
  );
}
