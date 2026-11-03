"use client";

import { useEffect, useRef, useState } from "react";

const SMS_MAX_CHARS = 160;

interface SmsResult {
  accepted: boolean;
  message: string;
}

export function SendSmsDialog({
  open,
  onClose,
  phoneNumber,
  imsi,
  onSent,
}: {
  open: boolean;
  onClose: () => void;
  phoneNumber?: string;
  imsi?: string;
  onSent?: (to: string) => void;
}) {
  const [from, setFrom] = useState("5551234");
  const [text, setText] = useState("");
  const [sending, setSending] = useState(false);
  const [result, setResult] = useState<SmsResult | null>(null);
  const textRef = useRef<HTMLTextAreaElement>(null);

  useEffect(() => {
    if (!open) return;
    setResult(null);
    textRef.current?.focus();
    const onKey = (e: KeyboardEvent) => e.key === "Escape" && onClose();
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [open, onClose]);

  if (!open) return null;

  const send = async () => {
    if (!text.trim()) return;
    setSending(true);
    setResult(null);
    try {
      const res = await fetch("/api/sms", {
        method: "POST",
        headers: { "Content-Type": "application/json" },
        body: JSON.stringify({
          originatingNumber: from,
          text,
          destinationNumber: phoneNumber || undefined,
          destinationImsi: phoneNumber ? undefined : imsi || undefined,
        }),
      });
      const data: SmsResult = await res.json();
      setResult(data);
      if (data.accepted) {
        setText("");
        onSent?.(phoneNumber || (imsi ? `IMSI ${imsi}` : "the mobile"));
        onClose();
      }
    } catch (err) {
      setResult({ accepted: false, message: err instanceof Error ? err.message : "unknown error" });
    } finally {
      setSending(false);
    }
  };

  return (
    <div
      className="fixed inset-0 z-50 grid place-items-center bg-black/55 p-4"
      onClick={(e) => e.target === e.currentTarget && onClose()}
      role="dialog"
      aria-modal="true"
      aria-label="Send SMS"
    >
      <div className="w-full max-w-md rounded-xl border border-border bg-surface-solid shadow-2xl">
        <div className="flex items-center justify-between border-b border-border px-4 py-3">
          <span className="text-sm font-semibold text-primary">Send SMS to {phoneNumber || (imsi ? `IMSI ${imsi}` : "this mobile")}</span>
          <button type="button" onClick={onClose} className="text-muted hover:text-secondary" aria-label="Close">
            ✕
          </button>
        </div>
        <div className="space-y-3 p-4">
          <label className="flex items-center gap-3 text-xs text-muted">
            From
            <input
              type="text"
              value={from}
              onChange={(e) => setFrom(e.target.value)}
              className="w-36 glass-input font-mono"
            />
          </label>
          <textarea
            ref={textRef}
            value={text}
            onChange={(e) => setText(e.target.value)}
            rows={4}
            className="w-full glass-input resize-none"
            placeholder="Text message"
          />
          <div className="flex items-center justify-between gap-3">
            <span className={`text-xs ${text.length > SMS_MAX_CHARS ? "text-accent-amber" : "text-muted"}`}>
              {text.length} / {SMS_MAX_CHARS}
            </span>
            <div className="flex gap-2">
              <button
                type="button"
                onClick={onClose}
                className="px-3 py-1.5 text-sm rounded border border-border text-secondary hover:bg-surface-raised"
              >
                Cancel
              </button>
              <button
                type="button"
                onClick={send}
                disabled={sending || !text.trim()}
                className="px-4 py-1.5 text-sm rounded bg-accent-green-bg text-accent-green border border-accent-green/20 hover:bg-accent-green/15 disabled:opacity-50"
              >
                {sending ? "Sending..." : "Send"}
              </button>
            </div>
          </div>
          {result && (
            <p className={`text-xs ${result.accepted ? "text-accent-green" : "text-accent-red"}`}>{result.message}</p>
          )}
        </div>
      </div>
    </div>
  );
}
