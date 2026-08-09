import { ReactNode } from "react";

export function TableFilters({
  title,
  shown,
  total,
  active,
  onClear,
  filters = [],
  quickFilters = [],
  children,
}: {
  title: string;
  shown: number;
  total: number;
  active: boolean;
  onClear: () => void;
  filters?: {
    label: string;
    value: string;
    onRemove: () => void;
  }[];
  quickFilters?: {
    label: string;
    active: boolean;
    onClick: () => void;
  }[];
  children?: ReactNode;
}) {
  return (
    <aside className="glass-card relative z-20 self-start overflow-visible xl:sticky xl:top-0">
      <div className="glass-card-title flex items-center justify-between gap-2">
        <span>{title}</span>
        <span className="font-mono normal-case tracking-normal text-dimmed">
          {shown}/{total}
        </span>
      </div>
      <div className="p-3 space-y-4">
        {quickFilters.length > 0 && (
          <div>
            <p className="mb-1.5 text-[10px] font-semibold uppercase tracking-wider text-dimmed">
              Quick filters
            </p>
            <div className="flex flex-wrap gap-1.5">
              {quickFilters.map((filter) => (
                <button
                  key={filter.label}
                  type="button"
                  onClick={filter.onClick}
                  className={`rounded border px-2 py-1 text-[11px] ${
                    filter.active
                      ? "border-accent-blue/40 bg-accent-blue/15 text-accent-blue"
                      : "border-border text-muted hover:border-border-input hover:text-primary"
                  }`}
                >
                  {filter.label}
                </button>
              ))}
            </div>
          </div>
        )}

        <div>
          <p className="mb-2 text-[10px] font-semibold uppercase tracking-wider text-dimmed">
            Exact field filters
          </p>
          <div className="space-y-2 text-xs">{children}</div>
        </div>

        {filters.length > 0 && (
          <div>
            <p className="mb-1.5 text-[10px] font-semibold uppercase tracking-wider text-dimmed">
              Active filters
            </p>
            <div className="flex flex-wrap gap-1.5">
              {filters.map((filter, index) => (
                <button
                  key={`${filter.label}:${filter.value}:${index}`}
                  type="button"
                  onClick={filter.onRemove}
                  title={`Remove ${filter.label} filter`}
                  className="max-w-full truncate rounded-full border border-accent-blue/30 bg-accent-blue/10 px-2 py-1 text-[10px] text-accent-blue hover:border-accent-blue/60"
                >
                  {filter.label}: {filter.value} ×
                </button>
              ))}
            </div>
          </div>
        )}

        <div className="flex items-center justify-between gap-2 border-t border-border/50 pt-3 text-[11px] text-dimmed">
          <span>
            Showing {shown} of {total}
          </span>
          {active && (
            <button
              type="button"
              onClick={onClear}
              className="text-accent-blue hover:underline"
            >
              Clear all
            </button>
          )}
        </div>
      </div>
    </aside>
  );
}

export const FILTER_SELECT_CLASS =
  "w-full bg-input border border-border-input rounded px-2 py-1 text-xs";
export const FILTER_INPUT_CLASS =
  "w-full bg-input border border-border-input rounded px-2 py-1 text-xs font-mono";

export function FilterField({
  label,
  className = "",
  children,
}: {
  label: string;
  className?: string;
  children: ReactNode;
}) {
  return (
    <div className={`block ${className} !w-full !min-w-0`}>
      <span className="block text-[10px] uppercase tracking-wider text-dimmed mb-0.5">
        {label}
      </span>
      {children}
    </div>
  );
}

export type SortDirection = "asc" | "desc";

export function compareSortValues(
  left: string | number,
  right: string | number,
  direction: SortDirection,
): number {
  const result =
    typeof left === "number" && typeof right === "number"
      ? left - right
      : String(left).localeCompare(String(right), undefined, {
          numeric: true,
          sensitivity: "base",
        });
  return direction === "asc" ? result : -result;
}

export function SortableColumnHeader({
  label,
  active,
  direction,
  onClick,
  className = "",
}: {
  label: string;
  active: boolean;
  direction: SortDirection;
  onClick: () => void;
  className?: string;
}) {
  return (
    <button
      type="button"
      onClick={onClick}
      className={`flex items-center gap-1 hover:text-primary ${className}`}
      title={`Sort by ${label}`}
    >
      <span>{label}</span>
      <span aria-hidden="true">{active ? (direction === "asc" ? "▲" : "▼") : "↕"}</span>
    </button>
  );
}
