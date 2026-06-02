import { PrlOption } from "@/lib/prl-options";

export function EnumSelect<T extends number | string>({
  label,
  value,
  options,
  onChange,
  disabled,
  className,
}: {
  label?: string;
  value: T;
  options: PrlOption<T>[];
  onChange: (next: T) => void;
  disabled?: boolean;
  className?: string;
}) {
  return (
    <label className={`block ${className ?? ""}`}>
      {label && <span className="text-muted text-[11px]">{label}</span>}
      <select
        className="block w-full mt-0.5 bg-bg border border-border rounded px-2 py-1 text-xs disabled:opacity-50"
        value={String(value)}
        disabled={disabled}
        onChange={(e) => {
          const raw = e.target.value;
          // Coerce numeric proto enums (which serialize as numbers in
          // ts-proto's runtime) back to number; otherwise keep as string.
          const coerced =
            typeof options[0]?.value === "number"
              ? (Number(raw) as unknown as T)
              : (raw as unknown as T);
          onChange(coerced);
        }}
      >
        {options.map((opt) => (
          <option key={String(opt.value)} value={String(opt.value)}>
            {opt.label}
          </option>
        ))}
      </select>
    </label>
  );
}
