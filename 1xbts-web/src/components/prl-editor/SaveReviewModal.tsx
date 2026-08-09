import { useEffect, useRef } from "react";
import {
  type ReviewFieldChange,
  type ReviewRecordChange,
  type SaveReview,
} from "./save-review";

export function SaveReviewModal({
  review,
  saving,
  onCancel,
  onConfirm,
}: {
  review: SaveReview;
  saving: boolean;
  onCancel: () => void;
  onConfirm: () => void;
}) {
  const confirmRef = useRef<HTMLButtonElement>(null);

  useEffect(() => {
    confirmRef.current?.focus();
    const onKeyDown = (event: KeyboardEvent) => {
      if (event.key === "Escape" && !saving) onCancel();
    };
    window.addEventListener("keydown", onKeyDown);
    return () => window.removeEventListener("keydown", onKeyDown);
  }, [onCancel, saving]);

  return (
    <div
      className="fixed inset-0 z-[100] flex items-center justify-center bg-black/65 p-4 backdrop-blur-sm"
      onMouseDown={(event) => {
        if (event.target === event.currentTarget && !saving) onCancel();
      }}
    >
      <div
        role="dialog"
        aria-modal="true"
        aria-labelledby="save-review-title"
        className="flex max-h-[85vh] w-full max-w-4xl flex-col overflow-hidden rounded-lg border border-border bg-surface-solid shadow-2xl"
      >
        <div className="flex items-start justify-between gap-4 border-b border-border px-4 py-3">
          <div>
            <h2 id="save-review-title" className="text-sm font-semibold text-primary">
              Review PRL changes
            </h2>
            <p className="mt-0.5 text-[11px] text-muted">
              Verify these changes before updating the stored PRL.
            </p>
          </div>
          <div className="flex flex-wrap justify-end gap-1.5">
            <CountBadge label="Added" count={review.counts.added} tone="green" />
            <CountBadge label="Deleted" count={review.counts.deleted} tone="red" />
            <CountBadge label="Changed" count={review.counts.changed} tone="blue" />
            <CountBadge label="Moved" count={review.counts.moved} tone="orange" />
          </div>
        </div>

        <div className="min-h-0 flex-1 space-y-4 overflow-y-auto px-4 py-3 text-xs">
          {review.fields.length > 0 && (
            <section>
              <SectionTitle>General</SectionTitle>
              <div className="divide-y divide-border/40 rounded border border-border/60">
                {review.fields.map((change) => (
                  <FieldChange key={change.label} change={change} />
                ))}
              </div>
            </section>
          )}

          {review.sections.map((section) => (
            <section key={section.title}>
              <SectionTitle>{section.title}</SectionTitle>
              <div className="space-y-1.5">
                {section.changes.map((change, index) => (
                  <RecordChange
                    key={`${change.fromIndex ?? "new"}-${change.toIndex ?? "deleted"}-${index}`}
                    change={change}
                  />
                ))}
              </div>
            </section>
          ))}
        </div>

        <div className="flex justify-end gap-2 border-t border-border px-4 py-3">
          <button
            type="button"
            onClick={onCancel}
            disabled={saving}
            className="rounded px-3 py-1.5 text-xs text-muted hover:text-primary disabled:opacity-50"
          >
            Cancel
          </button>
          <button
            ref={confirmRef}
            type="button"
            onClick={onConfirm}
            disabled={saving}
            className="rounded border border-accent-blue/40 bg-accent-blue/20 px-3 py-1.5 text-xs text-accent-blue hover:bg-accent-blue/30 disabled:opacity-50"
          >
            {saving ? "Saving…" : "Save PRL"}
          </button>
        </div>
      </div>
    </div>
  );
}

function SectionTitle({ children }: { children: React.ReactNode }) {
  return (
    <h3 className="mb-1.5 text-[10px] font-semibold uppercase tracking-wider text-dimmed">
      {children}
    </h3>
  );
}

function FieldChange({ change }: { change: ReviewFieldChange }) {
  return (
    <div className="grid gap-1 px-3 py-2 md:grid-cols-[10rem_minmax(0,1fr)]">
      <span className="font-medium text-secondary">{change.label}</span>
      <BeforeAfter before={change.before} after={change.after} />
    </div>
  );
}

function RecordChange({ change }: { change: ReviewRecordChange }) {
  const summaryChanged =
    change.beforeSummary &&
    change.afterSummary &&
    change.beforeSummary !== change.afterSummary;
  return (
    <div className="rounded border border-border/60 bg-bg/30 px-3 py-2">
      <div className="flex items-start gap-2">
        <div className="flex min-w-20 flex-wrap gap-1">
          {change.added && <StatusBadge label="Added" tone="green" />}
          {change.deleted && <StatusBadge label="Deleted" tone="red" />}
          {change.changed && <StatusBadge label="Changed" tone="blue" />}
          {change.moved && <StatusBadge label="Moved" tone="orange" />}
        </div>
        <div className="min-w-0 flex-1 font-mono text-[11px]">
          {change.added ? (
            <span className="text-accent-green">{change.afterSummary}</span>
          ) : change.deleted ? (
            <span className="text-accent-red line-through">{change.beforeSummary}</span>
          ) : summaryChanged ? (
            <BeforeAfter
              before={change.beforeSummary!}
              after={change.afterSummary!}
            />
          ) : (
            <span className="text-secondary">
              {change.afterSummary ?? change.beforeSummary}
            </span>
          )}
          {change.moved && (
            <span className="ml-2 text-accent-orange">
              #{change.fromIndex} → #{change.toIndex}
            </span>
          )}
        </div>
      </div>
      {change.fields.length > 0 && (
        <div className="mt-2 grid gap-x-4 gap-y-1 border-t border-border/30 pt-2 md:grid-cols-2">
          {change.fields.slice(0, 6).map((field, index) => (
            <div key={`${field.label}-${index}`} className="min-w-0">
              <span className="text-dimmed">{field.label}: </span>
              <span className="font-mono text-accent-red line-through">
                {field.before}
              </span>
              <span className="mx-1 text-dimmed">→</span>
              <span className="font-mono text-accent-green">{field.after}</span>
            </div>
          ))}
          {change.fields.length > 6 && (
            <span className="text-dimmed">+{change.fields.length - 6} more fields</span>
          )}
        </div>
      )}
    </div>
  );
}

function BeforeAfter({ before, after }: { before: string; after: string }) {
  return (
    <div className="flex min-w-0 items-baseline gap-2">
      <span className="min-w-0 truncate font-mono text-accent-red line-through" title={before}>
        {before}
      </span>
      <span className="shrink-0 text-dimmed">→</span>
      <span className="min-w-0 truncate font-mono text-accent-green" title={after}>
        {after}
      </span>
    </div>
  );
}

function CountBadge({
  label,
  count,
  tone,
}: {
  label: string;
  count: number;
  tone: Tone;
}) {
  if (count === 0) return null;
  return <StatusBadge label={`${count} ${label}`} tone={tone} />;
}

type Tone = "green" | "red" | "blue" | "orange";

function StatusBadge({ label, tone }: { label: string; tone: Tone }) {
  const classes = {
    green: "border-accent-green/30 bg-accent-green/10 text-accent-green",
    red: "border-accent-red/30 bg-accent-red/10 text-accent-red",
    blue: "border-accent-blue/30 bg-accent-blue/10 text-accent-blue",
    orange: "border-accent-orange/30 bg-accent-orange/10 text-accent-orange",
  }[tone];
  return (
    <span className={`rounded border px-1.5 py-0.5 text-[10px] ${classes}`}>
      {label}
    </span>
  );
}
