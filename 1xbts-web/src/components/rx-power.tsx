export function RxPower({
  dbm,
  dbfs,
  className,
}: {
  dbm?: number;
  dbfs?: number;
  className?: string;
}) {
  if (dbm != null) {
    return (
      <span className={className} title={dbfs != null ? `${dbfs.toFixed(1)} dBFS` : undefined}>
        {dbm.toFixed(1)} <span className="text-muted text-[11px]">dBm</span>
      </span>
    );
  }
  if (dbfs != null) {
    return (
      <span className={className} title="Set rx_reference_dbm to show dBm">
        {dbfs.toFixed(1)} <span className="text-muted text-[11px]">dBFS</span>
      </span>
    );
  }
  return <span className={className}>-</span>;
}
