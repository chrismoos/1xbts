"use client";

import { useEffect, useState } from "react";
import { isApiErrorBody, type ApiErrorBody } from "./api-error-body";

// One WebSocket per tab to the hub in server.mjs, with reference-counted topics.

const WS_PATH = "/ws";
const RECONNECT_MIN_MS = 500;
const RECONNECT_MAX_MS = 10000;
// Message events this tab keeps for replay to a component that subscribes after
// they arrived, per event name. Values above REPLAY_DEPTH_PER_EVENT in
// server.mjs gain nothing on a fresh tab.
const REPLAY_DEPTH_PER_EVENT = 1000;

type ResourceValue = { data?: unknown; error?: string; errorBody?: ApiErrorBody };
type ResourceListener = (value: ResourceValue) => void;
type EventListener = (event: string, data: string) => void;
type ConnectionListener = (connected: boolean) => void;
type ReplayItem = [seq: number, event: string, data: string];

interface EventSubscription {
  fn: EventListener;
  replay: boolean;
}

interface ResourceState {
  listeners: Set<ResourceListener>;
  last?: ResourceValue;
}

class LiveSocket {
  private socket: WebSocket | null = null;
  private reconnectMs = RECONNECT_MIN_MS;
  private resources = new Map<string, ResourceState>();
  private eventListeners = new Set<EventSubscription>();
  private replayBuffers = new Map<string, ReplayItem[]>();
  private replayEpoch: string | null = null;
  private lastSeq = 0;
  private connectionListeners = new Set<ConnectionListener>();
  private backendConnected = false;

  private ensureSocket() {
    if (this.socket || typeof window === "undefined") return;
    const scheme = window.location.protocol === "https:" ? "wss" : "ws";
    const socket = new WebSocket(`${scheme}://${window.location.host}${WS_PATH}`);
    this.socket = socket;
    socket.onopen = () => {
      this.reconnectMs = RECONNECT_MIN_MS;
      if (this.eventListeners.size > 0 || this.connectionListeners.size > 0) this.sendOp({ op: "events" });
      for (const key of this.resources.keys()) this.sendOp({ op: "sub", k: key });
    };
    socket.onmessage = (msg) => this.receive(String(msg.data));
    socket.onclose = () => {
      this.socket = null;
      this.setBackendConnected(false);
      const delay = this.reconnectMs;
      this.reconnectMs = Math.min(this.reconnectMs * 2, RECONNECT_MAX_MS);
      if (this.hasSubscribers()) setTimeout(() => this.ensureSocket(), delay);
    };
  }

  private hasSubscribers() {
    return this.resources.size > 0 || this.eventListeners.size > 0 || this.connectionListeners.size > 0;
  }

  private sendOp(op: Record<string, unknown>) {
    if (this.socket?.readyState === WebSocket.OPEN) this.socket.send(JSON.stringify(op));
  }

  private receive(raw: string) {
    let msg: {
      t: string;
      k?: string;
      d?: string;
      err?: string;
      e?: string;
      c?: boolean;
      s?: number;
      hs?: number;
      ep?: string;
      items?: ReplayItem[];
    };
    try {
      msg = JSON.parse(raw);
    } catch {
      return;
    }
    if (msg.t === "res" && msg.k) {
      const state = this.resources.get(msg.k);
      if (!state) return;
      let value: ResourceValue;
      if (msg.err) {
        value = { error: "The web server could not load this data. Check that it is running." };
      } else {
        try {
          const data = JSON.parse(msg.d ?? "null");
          value = isApiErrorBody(data)
            ? { error: data.error, errorBody: data }
            : msg.hs
              ? { error: `The web server returned HTTP ${msg.hs}.` }
              : { data };
        } catch {
          value = {
            error: msg.hs
              ? `The web server returned HTTP ${msg.hs}.`
              : "The web server sent a response that could not be read.",
          };
        }
      }
      state.last = value;
      for (const fn of state.listeners) fn(value);
    } else if (msg.t === "ev" && msg.e && msg.d != null) {
      if (msg.s != null) {
        if (msg.s <= this.lastSeq) return;
        this.record([msg.s, msg.e, msg.d]);
      }
      for (const sub of this.eventListeners) sub.fn(msg.e, msg.d);
    } else if (msg.t === "replay" && msg.ep) {
      if (msg.ep !== this.replayEpoch) {
        this.replayEpoch = msg.ep;
        this.replayBuffers.clear();
        this.lastSeq = 0;
      }
      for (const item of msg.items ?? []) {
        if (item[0] <= this.lastSeq) continue;
        this.record(item);
        for (const sub of this.eventListeners) if (sub.replay) sub.fn(item[1], item[2]);
      }
    } else if (msg.t === "conn") {
      this.setBackendConnected(msg.c === true);
    }
  }

  private record(item: ReplayItem) {
    this.lastSeq = item[0];
    let buffer = this.replayBuffers.get(item[1]);
    if (!buffer) {
      buffer = [];
      this.replayBuffers.set(item[1], buffer);
    }
    buffer.push(item);
    if (buffer.length > REPLAY_DEPTH_PER_EVENT) buffer.shift();
  }

  private setBackendConnected(value: boolean) {
    if (this.backendConnected === value) return;
    this.backendConnected = value;
    for (const fn of this.connectionListeners) fn(value);
  }

  subscribeResource(key: string, fn: ResourceListener): () => void {
    let state = this.resources.get(key);
    if (!state) {
      state = { listeners: new Set() };
      this.resources.set(key, state);
      this.sendOp({ op: "sub", k: key });
    } else if (state.last) {
      fn(state.last);
    }
    state.listeners.add(fn);
    this.ensureSocket();
    return () => {
      const current = this.resources.get(key);
      if (!current) return;
      current.listeners.delete(fn);
      if (current.listeners.size === 0) {
        this.resources.delete(key);
        this.sendOp({ op: "unsub", k: key });
      }
    };
  }

  refresh(key: string) {
    this.sendOp({ op: "refresh", k: key });
  }

  /**
   * With `replay`, `fn` first receives the message events already buffered,
   * oldest first. A subscriber that also loads history elsewhere should leave
   * it off, or it sees those events twice.
   */
  subscribeEvents(fn: EventListener, options: { replay?: boolean } = {}): () => void {
    const sub: EventSubscription = { fn, replay: options.replay === true };
    const first = this.eventListeners.size === 0 && this.connectionListeners.size === 0;
    this.eventListeners.add(sub);
    this.ensureSocket();
    if (first) this.sendOp({ op: "events" });
    if (sub.replay) {
      const items = [...this.replayBuffers.values()].flat().sort((a, b) => a[0] - b[0]);
      for (const [, event, data] of items) fn(event, data);
    }
    return () => this.eventListeners.delete(sub);
  }

  onConnection(fn: ConnectionListener): () => void {
    const first = this.eventListeners.size === 0 && this.connectionListeners.size === 0;
    this.connectionListeners.add(fn);
    this.ensureSocket();
    if (first) this.sendOp({ op: "events" });
    fn(this.backendConnected);
    return () => this.connectionListeners.delete(fn);
  }
}

const GLOBAL_KEY = "__1xbts_live_socket__";

export function liveSocket(): LiveSocket {
  const g = globalThis as Record<string, unknown>;
  if (!g[GLOBAL_KEY]) g[GLOBAL_KEY] = new LiveSocket();
  return g[GLOBAL_KEY] as LiveSocket;
}

export interface LiveResource<T> {
  data: T | undefined;
  error: string | null;
  errorBody: ApiErrorBody | null;
  loading: boolean;
}

/** A null key subscribes to nothing. */
export function useLive<T>(key: string | null): LiveResource<T> {
  const [state, setState] = useState<{
    key: string | null;
    data?: T;
    error: string | null;
    errorBody: ApiErrorBody | null;
  }>({
    key,
    error: null,
    errorBody: null,
  });
  useEffect(() => {
    if (!key) return;
    return liveSocket().subscribeResource(key, (value) =>
      setState({
        key,
        data: value.data as T | undefined,
        error: value.error ?? null,
        errorBody: value.errorBody ?? null,
      }),
    );
  }, [key]);
  const current = state.key === key;
  return {
    data: current ? state.data : undefined,
    error: current ? state.error : null,
    errorBody: current ? state.errorBody : null,
    loading: key != null && (!current || (state.data === undefined && state.error == null)),
  };
}
