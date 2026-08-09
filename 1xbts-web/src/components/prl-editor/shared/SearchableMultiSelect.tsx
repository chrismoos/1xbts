import { KeyboardEvent, useEffect, useId, useMemo, useRef, useState } from "react";
import { SearchableOption } from "./SearchableSelect";

export function SearchableMultiSelect({
  values,
  options,
  onChange,
  placeholder,
  ariaLabel,
}: {
  values: string[];
  options: SearchableOption[];
  onChange: (values: string[]) => void;
  placeholder: string;
  ariaLabel: string;
}) {
  const [open, setOpen] = useState(false);
  const [query, setQuery] = useState("");
  const searchRef = useRef<HTMLInputElement>(null);
  const listboxId = useId();
  const selected = useMemo(() => new Set(values), [values]);
  const filtered = useMemo(() => {
    const terms = query.trim().toLowerCase().split(/\s+/).filter(Boolean);
    if (terms.length === 0) return options;
    return options.filter((option) => {
      const text = `${option.label} ${option.searchText ?? ""}`.toLowerCase();
      return terms.every((term) => text.includes(term));
    });
  }, [options, query]);

  useEffect(() => {
    if (open) searchRef.current?.focus();
  }, [open]);

  const toggle = (value: string) => {
    onChange(
      selected.has(value)
        ? values.filter((current) => current !== value)
        : [...values, value],
    );
  };

  const selectedLabel =
    values.length === 0
      ? placeholder
      : values.length === 1
        ? (options.find((option) => option.value === values[0])?.label ??
          values[0])
        : `${values.length} selected`;

  const onSearchKeyDown = (event: KeyboardEvent<HTMLInputElement>) => {
    if (event.key === "Escape") {
      event.preventDefault();
      setOpen(false);
      setQuery("");
    }
  };

  return (
    <div
      className="relative"
      onBlur={(event) => {
        if (!event.currentTarget.contains(event.relatedTarget)) {
          setOpen(false);
          setQuery("");
        }
      }}
    >
      <button
        type="button"
        aria-label={ariaLabel}
        aria-expanded={open}
        aria-controls={listboxId}
        aria-haspopup="listbox"
        onClick={() => setOpen((current) => !current)}
        onKeyDown={(event) => {
          if (event.key === "ArrowDown") {
            event.preventDefault();
            setOpen(true);
          }
        }}
        className="flex w-full items-center justify-between gap-2 rounded border border-border-input bg-input px-2 py-1 text-left text-xs"
      >
        <span className={values.length === 0 ? "truncate text-muted" : "truncate"}>
          {selectedLabel}
        </span>
        <span className="shrink-0 text-dimmed" aria-hidden="true">
          {open ? "▴" : "▾"}
        </span>
      </button>

      {open && (
        <div className="absolute z-40 mt-1 w-full min-w-64 overflow-hidden rounded border border-border-input bg-surface-solid shadow-xl">
          <div className="border-b border-border p-2">
            <input
              ref={searchRef}
              type="search"
              value={query}
              onChange={(event) => setQuery(event.target.value)}
              onKeyDown={onSearchKeyDown}
              placeholder="Filter choices…"
              aria-label={`Search ${ariaLabel}`}
              className="w-full rounded border border-border-input bg-input px-2 py-1 text-xs outline-none focus:border-accent-indigo"
            />
          </div>
          <div
            id={listboxId}
            role="listbox"
            aria-multiselectable="true"
            className="max-h-64 overflow-auto py-1"
          >
            {filtered.length === 0 ? (
              <div className="px-3 py-3 text-xs text-dimmed">No matches</div>
            ) : (
              filtered.map((option) => {
                const checked = selected.has(option.value);
                return (
                  <button
                    key={option.value}
                    type="button"
                    role="option"
                    aria-selected={checked}
                    onMouseDown={(event) => event.preventDefault()}
                    onClick={() => toggle(option.value)}
                    className="flex w-full items-start gap-2 px-3 py-2 text-left text-xs hover:bg-surface-raised"
                  >
                    <span
                      aria-hidden="true"
                      className={`mt-px flex h-3.5 w-3.5 shrink-0 items-center justify-center rounded-sm border text-[9px] ${
                        checked
                          ? "border-accent-blue bg-accent-blue/20 text-accent-blue"
                          : "border-border-input bg-input"
                      }`}
                    >
                      {checked ? "✓" : ""}
                    </span>
                    <span className="min-w-0 truncate font-mono">
                      {option.label}
                    </span>
                  </button>
                );
              })
            )}
          </div>
          <div className="flex items-center justify-between border-t border-border px-2 py-1.5">
            <button
              type="button"
              onMouseDown={(event) => event.preventDefault()}
              onClick={() => onChange([])}
              disabled={values.length === 0}
              className="text-[11px] text-accent-blue hover:underline disabled:opacity-30"
            >
              Clear
            </button>
            <button
              type="button"
              onClick={() => {
                setOpen(false);
                setQuery("");
              }}
              className="rounded border border-border px-2 py-1 text-[11px] text-muted hover:text-primary"
            >
              Done
            </button>
          </div>
        </div>
      )}
    </div>
  );
}
