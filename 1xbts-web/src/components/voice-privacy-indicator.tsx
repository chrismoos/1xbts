"use client";

import { useEffect, useId, useState } from "react";
import { createPortal } from "react-dom";

const privacyLabels = {
  public: "Voice privacy inactive · public long code",
  pending: "Voice privacy pending · waiting for the handset and action time",
  private: "Voice privacy active · private long code",
};

interface TooltipPosition {
  left: number;
  top: number;
  above: boolean;
}

export function VoicePrivacyIndicator({ status }: { status?: string }) {
  const tooltipId = useId();
  const [position, setPosition] = useState<TooltipPosition | null>(null);
  useEffect(() => {
    if (!position) return;
    const hide = () => setPosition(null);
    window.addEventListener("scroll", hide, true);
    window.addEventListener("resize", hide);
    return () => {
      window.removeEventListener("scroll", hide, true);
      window.removeEventListener("resize", hide);
    };
  }, [position]);
  if (status !== "public" && status !== "pending" && status !== "private") return null;
  const label = privacyLabels[status];
  const color = status === "private"
    ? "text-accent-green"
    : status === "pending" ? "text-badge-yellow-text" : "text-muted";
  const show = (element: HTMLElement) => {
    const rect = element.getBoundingClientRect();
    const tooltipWidth = 224;
    const viewportInset = 8;
    const above = rect.top > 80;
    setPosition({
      left: Math.max(viewportInset, Math.min(rect.right - tooltipWidth, window.innerWidth - tooltipWidth - viewportInset)),
      top: above ? rect.top - viewportInset : rect.bottom + viewportInset,
      above,
    });
  };

  return (
    <>
      <span
        className={`inline-flex shrink-0 cursor-help rounded-sm focus-visible:outline focus-visible:outline-2 focus-visible:outline-accent-green ${color}`}
        tabIndex={0}
        aria-label={label}
        aria-describedby={position ? tooltipId : undefined}
        onMouseEnter={(event) => show(event.currentTarget)}
        onMouseLeave={() => setPosition(null)}
        onFocus={(event) => show(event.currentTarget)}
        onBlur={() => setPosition(null)}
        onKeyDown={(event) => { if (event.key === "Escape") setPosition(null); }}
      >
        <svg width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="1.75" strokeLinecap="round" strokeLinejoin="round" aria-hidden="true">
          <rect x="5" y="10" width="14" height="11" rx="2" />
          {status === "private" ? <path d="M8 10V6a4 4 0 0 1 8 0v4" /> : <path d="M8 10V6a4 4 0 0 1 7.5-2" />}
          {status === "pending" ? <path d="M10 15h4m-2-2v4" /> : <path d="M12 14v3" />}
        </svg>
      </span>
      {position && createPortal(
        <span id={tooltipId} role="tooltip" className="pointer-events-none fixed z-50 w-56 max-w-[calc(100vw-16px)] rounded border border-border bg-popover px-2 py-1.5 text-xs font-normal text-primary shadow-md" style={{ left: position.left, top: position.top, transform: position.above ? "translateY(-100%)" : undefined }}>
          {label}
        </span>, document.body,
      )}
    </>
  );
}
