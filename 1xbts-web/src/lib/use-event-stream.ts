"use client";

import { useEffect, useRef, useState } from "react";
import { liveSocket } from "./live";

export interface EventStreamOptions {
  /** Deliver the buffered events first. See `subscribeEvents`. */
  replay?: boolean;
  /**
   * Subscribes only while true. A replaying page that filters on data it loads
   * separately sets it once that data is present, so the replay is not dropped.
   */
  enabled?: boolean;
}

export function useEventStream(event: string, onData: (data: string) => void, options: EventStreamOptions = {}) {
  const { replay = false, enabled = true } = options;
  const callbackRef = useRef(onData);

  useEffect(() => {
    callbackRef.current = onData;
  }, [onData]);

  useEffect(() => {
    if (!enabled) return;
    return liveSocket().subscribeEvents(
      (name, data) => {
        if (name === event) callbackRef.current(data);
      },
      { replay },
    );
  }, [event, replay, enabled]);
}

export function useSSEConnected(): boolean {
  const [connected, setConnected] = useState(false);

  useEffect(() => liveSocket().onConnection(setConnected), []);

  return connected;
}

export type ConnectionState = "connecting" | "online" | "offline";

// The event stream takes a few seconds to open on a fresh load, so a page
// starts in "connecting" and only reports "offline" once the grace period
// passes without a connection. A drop after the grace period is reported right
// away.
export function useConnectionState(graceMs = 5000): ConnectionState {
  const connected = useSSEConnected();
  const [graceElapsed, setGraceElapsed] = useState(false);

  useEffect(() => {
    const timer = setTimeout(() => setGraceElapsed(true), graceMs);
    return () => clearTimeout(timer);
  }, [graceMs]);

  if (connected) return "online";
  if (!graceElapsed) return "connecting";
  return "offline";
}
