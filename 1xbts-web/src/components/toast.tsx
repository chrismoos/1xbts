"use client";

import { useCallback, useEffect, useRef, useState } from "react";

const TOAST_MS = 3000;

export interface ToastMessage {
  text: string;
  tone: "ok" | "error";
}

export function useToast(): [ToastMessage | null, (text: string, tone?: ToastMessage["tone"]) => void] {
  const [toast, setToast] = useState<ToastMessage | null>(null);
  const timer = useRef<ReturnType<typeof setTimeout> | null>(null);
  const show = useCallback((text: string, tone: ToastMessage["tone"] = "ok") => {
    if (timer.current) clearTimeout(timer.current);
    setToast({ text, tone });
    timer.current = setTimeout(() => setToast(null), TOAST_MS);
  }, []);
  useEffect(() => () => {
    if (timer.current) clearTimeout(timer.current);
  }, []);
  return [toast, show];
}

export function Toast({ toast }: { toast: ToastMessage | null }) {
  if (!toast) return null;
  return (
    <div className="pointer-events-none fixed inset-x-0 bottom-6 z-50 flex justify-center px-4" role="status" aria-live="polite">
      <div
        className={`flex items-center gap-2 rounded-full border px-4 py-2 text-sm shadow-lg backdrop-blur ${
          toast.tone === "ok"
            ? "border-accent-green/30 bg-surface-solid text-accent-green"
            : "border-accent-red/30 bg-surface-solid text-accent-red"
        }`}
      >
        <span aria-hidden="true">{toast.tone === "ok" ? "✓" : "!"}</span>
        <span className="text-primary">{toast.text}</span>
      </div>
    </div>
  );
}
