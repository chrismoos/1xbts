import { formatHexDump } from "@/lib/format";
import { parseMNotificationInd } from "@/lib/wap-push";

export function KV({ label, value }: { label: string; value: React.ReactNode }) {
  return (
    <div className="flex gap-3 text-xs">
      <div className="text-muted w-40 shrink-0">{label}</div>
      <div className="text-secondary font-mono break-all">{value}</div>
    </div>
  );
}

export function WapBody({ bytes }: { bytes: Uint8Array }) {
  const parsed = parseMNotificationInd(bytes);

  if (parsed) {
    return (
      <div className="flex flex-col gap-1">
        <div className="text-xs text-accent-blue mb-1">Parsed as M-Notification.ind</div>
        {parsed.mmsVersion && <KV label="MMS Version" value={parsed.mmsVersion} />}
        {parsed.transactionId && <KV label="Transaction-ID" value={parsed.transactionId} />}
        {parsed.from && <KV label="From" value={parsed.from} />}
        {parsed.messageClass && <KV label="Class" value={parsed.messageClass} />}
        {parsed.messageSize !== undefined && (
          <KV label="Message-Size" value={`${parsed.messageSize} bytes`} />
        )}
        {parsed.expiryRelativeSeconds !== undefined && (
          <KV label="Expiry" value={`${parsed.expiryRelativeSeconds}s`} />
        )}
        {parsed.contentLocation && (
          <KV label="Content-Location" value={parsed.contentLocation} />
        )}
        {parsed.unparsedBytes !== undefined && parsed.unparsedBytes > 0 && (
          <div className="text-xs text-dimmed mt-1">
            {parsed.unparsedBytes} byte(s) of trailing unparsed data.
          </div>
        )}
      </div>
    );
  }

  return (
    <div className="flex flex-col gap-1">
      <div className="text-xs text-muted mb-1">
        Could not parse as M-Notification.ind — raw bytes:
      </div>
      <pre className="text-xs font-mono text-dimmed bg-surface-solid p-2 rounded overflow-x-auto">
        {formatHexDump(bytes)}
      </pre>
    </div>
  );
}
