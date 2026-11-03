export function Card({
  title,
  children,
  className,
  action,
}: {
  title: string;
  children: React.ReactNode;
  className?: string;
  action?: React.ReactNode;
}) {
  return (
    <div className={`glass-card p-4 ${className ?? ""}`}>
      <h2 className="glass-card-title -mx-4 -mt-4 mb-3 flex items-center justify-between gap-3">
        {title}
        {action && <span className="normal-case tracking-normal font-normal">{action}</span>}
      </h2>
      {children}
    </div>
  );
}

export function Stat({
  label,
  value,
  mono,
  title,
}: {
  label: string;
  value: string;
  mono?: boolean;
  title?: string;
}) {
  return (
    <div className="flex justify-between py-0.5">
      <span className="text-muted text-sm">{label}</span>
      <span
        className={`text-secondary text-sm ${mono ? "font-mono" : "font-medium"}`}
        title={title}
      >
        {value}
      </span>
    </div>
  );
}
