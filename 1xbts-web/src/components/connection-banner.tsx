"use client";

import { useSSEConnected } from "@/lib/use-event-stream";

export function ConnectionBanner() {
  const connected = useSSEConnected();

  if (connected) return null;

  return (
    <div className="bg-accent-red-bg border-b border-accent-red/20 px-4 py-2 text-sm text-accent-red flex items-center gap-2 shrink-0">
      <svg className="w-4 h-4 shrink-0" viewBox="0 0 20 20" fill="currentColor">
        <path fillRule="evenodd" d="M8.485 2.495c.673-1.167 2.357-1.167 3.03 0l6.28 10.875c.673 1.167-.17 2.625-1.516 2.625H3.72c-1.347 0-2.189-1.458-1.515-2.625L8.485 2.495zM10 6a.75.75 0 01.75.75v3.5a.75.75 0 01-1.5 0v-3.5A.75.75 0 0110 6zm0 9a1 1 0 100-2 1 1 0 000 2z" clipRule="evenodd" />
      </svg>
      BSC not reachable &mdash; attempting to reconnect...
    </div>
  );
}
