export function SummaryTile({
  label,
  value,
  unit,
  detail,
  detailClass,
  valueClass,
  children,
}: {
  label: string;
  value: string;
  unit?: string;
  detail?: string;
  detailClass?: string;
  valueClass?: string;
  children?: React.ReactNode;
}) {
  return (
    <div className="glass-card px-4 py-3">
      <div className="text-[11px] uppercase tracking-wide text-muted">{label}</div>
      <div className={`mt-0.5 text-2xl font-semibold ${valueClass ?? "text-primary"}`}>
        {value} {unit && <span className="text-xs font-normal text-muted">{unit}</span>}
      </div>
      {detail && <div className={`mt-0.5 text-xs ${detailClass ?? "text-muted"}`}>{detail}</div>}
      {children}
    </div>
  );
}
