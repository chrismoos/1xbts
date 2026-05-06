export function Card({
  title,
  children,
  className,
}: {
  title: string;
  children: React.ReactNode;
  className?: string;
}) {
  return (
    <div className={`glass-card p-4 ${className ?? ""}`}>
      <h2 className="glass-card-title -mx-4 -mt-4 mb-3">
        {title}
      </h2>
      {children}
    </div>
  );
}

export function Stat({
  label,
  value,
  mono,
}: {
  label: string;
  value: string;
  mono?: boolean;
}) {
  return (
    <div className="flex justify-between py-0.5">
      <span className="text-muted text-sm">{label}</span>
      <span
        className={`text-secondary text-sm ${mono ? "font-mono" : "font-medium"}`}
      >
        {value}
      </span>
    </div>
  );
}
