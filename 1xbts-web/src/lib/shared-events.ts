/**
 * Shared SSE connection across browser tabs using BroadcastChannel.
 *
 * Only one tab (the "leader") holds the actual EventSource connection.
 * It relays events to all other tabs via BroadcastChannel.  If the leader
 * tab closes, another tab takes over after the SSE retry interval.
 */

type Listener = (data: string) => void;
type ConnectListener = (connected: boolean) => void;

const CHANNEL_NAME = "cdma-sse";
const HEARTBEAT_MS = 3000;
const LEADER_TIMEOUT_MS = 5000;

const GLOBAL_KEY = "__cdma_shared_events__" as const;

export class SharedEventSource {
  private bc: BroadcastChannel;
  private es: EventSource | null = null;
  private listeners = new Map<string, Set<Listener>>();
  private connectListeners = new Set<ConnectListener>();
  private _connected = false;
  private isLeader = false;
  private lastHeartbeat = 0;
  private heartbeatInterval: ReturnType<typeof setInterval> | null = null;
  private checkInterval: ReturnType<typeof setInterval> | null = null;
  /** Dedup: track last dispatched event per type to suppress duplicates */
  private lastDispatched = new Map<string, { data: string; ts: number }>();

  constructor() {
    this.bc = new BroadcastChannel(CHANNEL_NAME);
    this.bc.onmessage = (msg) => {
      const { type, event, data, connected } = msg.data;
      if (type === "event") {
        // Leader already dispatched locally from EventSource — skip to
        // avoid double-delivery when another BroadcastChannel in the same
        // page (e.g. stale instance after HMR) echoes our own broadcast.
        if (!this.isLeader) {
          this.dispatch(event, data);
        }
      } else if (type === "heartbeat") {
        this.lastHeartbeat = Date.now();
      } else if (type === "connected") {
        if (!this.isLeader) {
          this.setConnected(connected);
        }
      }
    };

    // Try to become leader
    this.lastHeartbeat = 0;
    this.checkInterval = setInterval(() => this.tryBecomeLeader(), LEADER_TIMEOUT_MS);

    // Attempt leadership immediately
    setTimeout(() => this.tryBecomeLeader(), 100 + Math.random() * 200);
  }

  private tryBecomeLeader() {
    if (this.isLeader) return;
    if (Date.now() - this.lastHeartbeat < LEADER_TIMEOUT_MS) return;

    // No heartbeat from a leader recently — take over
    this.isLeader = true;
    console.log("[shared-events] becoming leader");
    this.startEventSource();
    this.heartbeatInterval = setInterval(() => {
      this.bc.postMessage({ type: "heartbeat" });
    }, HEARTBEAT_MS);
    // Send first heartbeat immediately
    this.bc.postMessage({ type: "heartbeat" });
  }

  private setConnected(value: boolean) {
    if (this._connected !== value) {
      this._connected = value;
      for (const fn of this.connectListeners) fn(value);
    }
  }

  get connected() {
    return this._connected;
  }

  onConnect(fn: ConnectListener) {
    this.connectListeners.add(fn);
    // Fire immediately with current state
    fn(this._connected);
  }

  offConnect(fn: ConnectListener) {
    this.connectListeners.delete(fn);
  }

  private startEventSource() {
    this.es = new EventSource("/api/events");

    this.es.onopen = () => {
      // Backend connectivity is driven by the SSE "connection" event, not by
      // the HTTP socket opening successfully.
    };

    this.es.addEventListener("connection", (e) => {
      try {
        const payload = JSON.parse(e.data) as { connected?: boolean };
        const connected = payload.connected === true;
        this.setConnected(connected);
        this.bc.postMessage({ type: "connected", connected });
      } catch {
        this.setConnected(false);
        this.bc.postMessage({ type: "connected", connected: false });
      }
    });

    const eventTypes = ["radio-metrics", "paging", "traffic", "access"];
    for (const eventType of eventTypes) {
      this.es.addEventListener(eventType, (e) => {
        // Dispatch locally
        this.dispatch(eventType, e.data);
        // Relay to other tabs
        this.bc.postMessage({ type: "event", event: eventType, data: e.data });
      });
    }

    this.es.onerror = () => {
      // EventSource auto-reconnects; we just keep the leader role
      this.setConnected(false);
      this.bc.postMessage({ type: "connected", connected: false });
    };
  }

  private dispatch(event: string, data: string) {
    // Dedup: suppress identical event+data dispatched within 50ms
    const now = Date.now();
    const last = this.lastDispatched.get(event);
    if (last && last.data === data && now - last.ts < 50) return;
    this.lastDispatched.set(event, { data, ts: now });

    const listeners = this.listeners.get(event);
    if (listeners && listeners.size > 0) {
      for (const fn of listeners) {
        fn(data);
      }
    } else {
      console.log(`[shared-events] no listeners for "${event}" (${this.listeners.size} event types registered)`);
    }
  }

  addEventListener(event: string, fn: Listener) {
    let set = this.listeners.get(event);
    if (!set) {
      set = new Set();
      this.listeners.set(event, set);
    }
    set.add(fn);
  }

  removeEventListener(event: string, fn: Listener) {
    this.listeners.get(event)?.delete(fn);
  }

  close() {
    if (this.es) {
      this.es.close();
      this.es = null;
    }
    if (this.heartbeatInterval) clearInterval(this.heartbeatInterval);
    if (this.checkInterval) clearInterval(this.checkInterval);
    this.bc.close();
    this.isLeader = false;
    delete (globalThis as Record<string, unknown>)[GLOBAL_KEY];
  }
}

/** Get or create the singleton shared event source. */
export function getSharedEvents(): SharedEventSource {
  const g = globalThis as Record<string, unknown>;
  if (!g[GLOBAL_KEY]) {
    g[GLOBAL_KEY] = new SharedEventSource();
  }
  return g[GLOBAL_KEY] as SharedEventSource;
}
