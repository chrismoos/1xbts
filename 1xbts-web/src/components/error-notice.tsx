"use client";

import Link from "next/link";
import { errorBodyOf } from "@/lib/api-client";
import type { ApiErrorBody } from "@/lib/api-error-body";

interface ErrorLink {
  href: string;
  label: string;
}

function linksFor(body: ApiErrorBody): ErrorLink[] {
  const metadata = body.metadata ?? {};
  const links: ErrorLink[] = [];
  if (body.reason === "IDENTITY_IN_USE" && metadata.subscriber_id) {
    links.push({ href: `/subscribers/${encodeURIComponent(metadata.subscriber_id)}`, label: "Open the subscriber that has it" });
  }
  if (body.reason === "PRL_IN_USE" && metadata.sample_subscriber_id) {
    links.push({
      href: `/subscribers/${encodeURIComponent(metadata.sample_subscriber_id)}`,
      label: "Open a subscriber that uses it",
    });
  }
  if (
    metadata.base_station &&
    (body.reason === "BASE_STATION_UNREACHABLE" || body.reason === "BASE_STATION_NOT_ATTACHED")
  ) {
    links.push({ href: "/base-stations", label: "Check base stations" });
  }
  return links;
}

export function ErrorNotice({
  error,
  onRetry,
  boxed = false,
  className = "",
}: {
  error: unknown;
  onRetry?: () => void;
  boxed?: boolean;
  className?: string;
}) {
  if (error == null || error === "") return null;
  const body = errorBodyOf(error);
  const links = linksFor(body);
  const fields = (body.fields ?? []).filter((field) => field.description && field.description !== body.message);
  const technical = [body.reason, body.service && `from ${body.service}`, body.detail].filter(Boolean).join(" · ");
  const frame = boxed ? "rounded-lg border border-accent-red/20 bg-accent-red-bg p-4" : "";
  return (
    <div role="alert" className={`text-sm text-accent-red ${frame} ${className}`}>
      {body.title ? (
        <p>
          <span className="font-medium">{body.title}.</span> {body.message}
        </p>
      ) : (
        <p>{body.message}</p>
      )}
      {fields.length > 0 && (
        <ul className="mt-1 list-disc pl-5 text-xs">
          {fields.map((field) => (
            <li key={`${field.field}:${field.description}`}>{field.description}</li>
          ))}
        </ul>
      )}
      {(links.length > 0 || onRetry) && (
        <div className="mt-1.5 flex flex-wrap gap-3 text-xs">
          {links.map((link) => (
            <Link key={link.href} href={link.href} className="underline hover:text-primary">
              {link.label}
            </Link>
          ))}
          {onRetry && (
            <button type="button" onClick={onRetry} className="underline hover:text-primary">
              Try again
            </button>
          )}
        </div>
      )}
      {technical && (
        <details className="mt-1.5 text-xs text-muted">
          <summary className="cursor-pointer select-none">Details</summary>
          <p className="mt-1 break-all font-mono text-[11px]">{technical}</p>
        </details>
      )}
    </div>
  );
}

export type ActionResult = { ok: true; text: string } | { ok: false; error: unknown };

export function ResultNotice({ result, className = "" }: { result: ActionResult | null; className?: string }) {
  if (!result) return null;
  if (result.ok) return <p className={`text-xs text-accent-green ${className}`}>{result.text}</p>;
  return <ErrorNotice error={result.error} className={`text-xs ${className}`} />;
}
