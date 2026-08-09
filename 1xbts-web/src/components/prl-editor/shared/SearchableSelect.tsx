import { KeyboardEvent, useId, useMemo, useState } from "react";

export interface SearchableOption {
  value: string;
  label: string;
  searchText?: string;
}

export function SearchableSelect({
  value,
  options,
  onChange,
  placeholder = "Search…",
  ariaLabel,
  className = "",
  invalid = false,
}: {
  value: string;
  options: SearchableOption[];
  onChange: (value: string) => void;
  placeholder?: string;
  ariaLabel: string;
  className?: string;
  invalid?: boolean;
}) {
  const [open, setOpen] = useState(false);
  const [query, setQuery] = useState("");
  const [activeIndex, setActiveIndex] = useState(0);
  const listboxId = useId();
  const selected = options.find((option) => option.value === value);
  const filtered = useMemo(() => {
    const terms = query.trim().toLowerCase().split(/\s+/).filter(Boolean);
    if (terms.length === 0) return options;
    return options.filter((option) => {
      const text = `${option.label} ${option.searchText ?? ""}`.toLowerCase();
      return terms.every((term) => text.includes(term));
    });
  }, [options, query]);

  const choose = (option: SearchableOption) => {
    onChange(option.value);
    setQuery("");
    setOpen(false);
  };

  const onKeyDown = (event: KeyboardEvent<HTMLInputElement>) => {
    if (event.key === "ArrowDown") {
      event.preventDefault();
      setOpen(true);
      setActiveIndex((current) =>
        open ? Math.min(current + 1, Math.max(0, filtered.length - 1)) : 0,
      );
    } else if (event.key === "ArrowUp") {
      event.preventDefault();
      setOpen(true);
      setActiveIndex((current) =>
        open ? Math.max(0, current - 1) : Math.max(0, filtered.length - 1),
      );
    } else if (event.key === "Enter" && open && filtered[activeIndex]) {
      event.preventDefault();
      choose(filtered[activeIndex]);
    } else if (event.key === "Escape") {
      setOpen(false);
      setQuery("");
    }
  };

  return (
    <div
      className={`relative ${className}`}
      onBlur={(event) => {
        if (!event.currentTarget.contains(event.relatedTarget)) {
          setOpen(false);
          setQuery("");
        }
      }}
    >
      <input
        type="text"
        role="combobox"
        aria-label={ariaLabel}
        aria-expanded={open}
        aria-controls={listboxId}
        aria-autocomplete="list"
        className={`w-full bg-input border rounded px-2 py-1 text-xs font-mono ${
          invalid ? "border-accent-red" : "border-border"
        }`}
        value={open ? query : (selected?.label ?? "")}
        placeholder={placeholder}
        onFocus={() => {
          setOpen(true);
          setQuery("");
          setActiveIndex(0);
        }}
        onChange={(event) => {
          setQuery(event.target.value);
          setOpen(true);
          setActiveIndex(0);
        }}
        onKeyDown={onKeyDown}
      />
      {open && (
        <div
          id={listboxId}
          role="listbox"
          className="absolute z-40 mt-1 max-h-64 w-full min-w-64 overflow-auto rounded border border-border-input bg-surface-solid shadow-xl"
        >
          {filtered.length === 0 ? (
            <div className="px-2 py-2 text-xs text-dimmed">No matches</div>
          ) : (
            filtered.map((option, index) => (
              <button
                key={option.value}
                type="button"
                role="option"
                aria-selected={option.value === value}
                tabIndex={-1}
                className={`block w-full px-2 py-1.5 text-left text-xs font-mono ${
                  index === activeIndex
                    ? "bg-accent-blue/20 text-primary"
                    : "hover:bg-surface-raised"
                }`}
                onMouseDown={(event) => event.preventDefault()}
                onClick={() => choose(option)}
              >
                {option.label}
              </button>
            ))
          )}
        </div>
      )}
    </div>
  );
}
