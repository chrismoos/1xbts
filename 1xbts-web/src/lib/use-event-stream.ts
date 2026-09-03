"use client";

import { useEffect, useRef, useState } from "react";
import { getSharedEvents } from "./shared-events";

/**
 * Subscribe to a named event from the shared SSE connection.
 * Only one tab holds the actual HTTP connection; others receive
 * via BroadcastChannel.
 */
export function useEventStream(event: string, onData: (data: string) => void) {
  const callbackRef = useRef(onData);

  useEffect(() => {
    callbackRef.current = onData;
  }, [onData]);

  useEffect(() => {
    const shared = getSharedEvents();
    const handler = (data: string) => callbackRef.current(data);
    shared.addEventListener(event, handler);
    return () => shared.removeEventListener(event, handler);
  }, [event]);
}

/** Returns true when the shared SSE connection is open. */
export function useSSEConnected(): boolean {
  const [connected, setConnected] = useState(false);

  useEffect(() => {
    const shared = getSharedEvents();
    // onConnect fires immediately with current state
    shared.onConnect(setConnected);
    return () => shared.offConnect(setConnected);
  }, []);

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
