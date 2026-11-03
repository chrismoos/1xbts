// Serves the Next.js app and a WebSocket hub on /ws from one port. The hub holds
// one upstream copy of each topic however many tabs subscribe. WebSockets do not
// count against the browser's per-host HTTP connection limit, so any number of
// tabs can stay open over plain http.

import { createServer } from "node:http";
import next from "next";
import { WebSocketServer } from "ws";

const args = process.argv.slice(2);
function arg(names, fallback) {
  const i = args.findIndex((a) => names.includes(a));
  return i >= 0 && args[i + 1] ? args[i + 1] : fallback;
}

const dev = process.env.NODE_ENV !== "production";
const hostname = arg(["--hostname", "-H"], process.env.HOSTNAME || "localhost");
const port = Number(arg(["--port", "-p"], process.env.PORT || "3000"));
const SELF = `http://127.0.0.1:${port}`;

const WS_PATH = "/ws";
const EVENTS_PATH = "/api/events";
const EVENTS_RETRY_MS = 2000;
const RESOURCE_PATTERN = /^\/api\/[a-z0-9/_-]+(\?[^#]*)?$/i;
const DEFAULT_POLL_MS = 3000;
// Message events kept for replay to a tab that subscribes after they arrived.
// Each event name keeps its own most recent REPLAY_DEPTH_PER_EVENT entries, so
// a busy stream cannot push out a quiet one. The buffer lives in this process
// and is lost on restart.
const REPLAY_DEPTH_PER_EVENT = 1000;
const REPLAY_EVENTS = new Set([
  "paging",
  "access",
  "traffic",
  "a1",
  "authentication",
  "hrpd-session",
  "hrpd-access",
  "hrpd-traffic",
]);
const REPLAY_BATCH = 500;
const REPLAY_EPOCH = `${Date.now().toString(36)}-${Math.random().toString(36).slice(2, 8)}`;
const POLL_MS = [
  ["/api/mobiles", 1000],
  ["/api/channels", 1000],
  ["/api/packet-sessions", 2000],
  ["/api/bts", 5000],
  ["/api/base-stations", 5000],
];

function pollMsFor(key) {
  const path = key.split("?")[0];
  return POLL_MS.find(([prefix]) => path === prefix)?.[1] ?? DEFAULT_POLL_MS;
}

const app = next({ dev, hostname, port });
const handle = app.getRequestHandler();

const eventSubscribers = new Set();
let backendConnected = false;
let eventSeq = 0;
const replayBuffers = new Map();

const resources = new Map();

function send(socket, message) {
  if (socket.readyState === socket.OPEN) socket.send(JSON.stringify(message));
}

function broadcastEvent(message) {
  for (const socket of eventSubscribers) send(socket, message);
}

function recordEvent(name, payload) {
  if (!REPLAY_EVENTS.has(name)) return undefined;
  const seq = ++eventSeq;
  let buffer = replayBuffers.get(name);
  if (!buffer) {
    buffer = [];
    replayBuffers.set(name, buffer);
  }
  buffer.push([seq, name, payload]);
  if (buffer.length > REPLAY_DEPTH_PER_EVENT) buffer.shift();
  return seq;
}

// Sent before any live event reaches the socket, so replay and live events
// neither overlap nor leave a gap.
function sendReplay(socket) {
  const items = [...replayBuffers.values()].flat().sort((a, b) => a[0] - b[0]);
  if (items.length === 0) send(socket, { t: "replay", ep: REPLAY_EPOCH, items });
  for (let i = 0; i < items.length; i += REPLAY_BATCH) {
    send(socket, { t: "replay", ep: REPLAY_EPOCH, items: items.slice(i, i + REPLAY_BATCH) });
  }
}

async function readEvents() {
  for (;;) {
    try {
      const res = await fetch(SELF + EVENTS_PATH, { headers: { accept: "text/event-stream" } });
      if (!res.ok || !res.body) throw new Error(`HTTP ${res.status}`);
      const decoder = new TextDecoder();
      let buffer = "";
      for await (const chunk of res.body) {
        buffer += decoder.decode(chunk, { stream: true });
        let split;
        while ((split = buffer.indexOf("\n\n")) >= 0) {
          const block = buffer.slice(0, split);
          buffer = buffer.slice(split + 2);
          let name = "message";
          const data = [];
          for (const line of block.split("\n")) {
            if (line.startsWith("event:")) name = line.slice(6).trim();
            else if (line.startsWith("data:")) data.push(line.slice(5).replace(/^ /, ""));
          }
          if (data.length === 0) continue;
          const payload = data.join("\n");
          if (name === "connection") {
            try {
              backendConnected = JSON.parse(payload).connected === true;
            } catch {
              backendConnected = false;
            }
            broadcastEvent({ t: "conn", c: backendConnected });
          } else {
            const seq = recordEvent(name, payload);
            broadcastEvent(seq ? { t: "ev", e: name, d: payload, s: seq } : { t: "ev", e: name, d: payload });
          }
        }
      }
    } catch {}
    if (backendConnected) {
      backendConnected = false;
      broadcastEvent({ t: "conn", c: false });
    }
    await new Promise((r) => setTimeout(r, EVENTS_RETRY_MS));
  }
}

async function pollResource(key) {
  const resource = resources.get(key);
  if (!resource || resource.inFlight) return;
  resource.inFlight = true;
  let message;
  try {
    const res = await fetch(SELF + key, { headers: { accept: "application/json" } });
    const body = await res.text();
    message = res.ok ? { t: "res", k: key, d: body } : { t: "res", k: key, err: `HTTP ${res.status}` };
  } catch (err) {
    message = { t: "res", k: key, err: err instanceof Error ? err.message : "fetch failed" };
  } finally {
    resource.inFlight = false;
  }
  const encoded = JSON.stringify(message);
  if (encoded === resource.last || !resources.has(key)) return;
  resource.last = encoded;
  for (const socket of resource.subscribers) {
    if (socket.readyState === socket.OPEN) socket.send(encoded);
  }
}

function subscribeResource(socket, key) {
  if (!RESOURCE_PATTERN.test(key) || key.startsWith(EVENTS_PATH)) return;
  let resource = resources.get(key);
  if (!resource) {
    resource = { subscribers: new Set(), last: null, timer: null, inFlight: false };
    resources.set(key, resource);
    resource.timer = setInterval(() => void pollResource(key), pollMsFor(key));
    void pollResource(key);
  } else if (resource.last && socket.readyState === socket.OPEN) {
    socket.send(resource.last);
  }
  resource.subscribers.add(socket);
}

function unsubscribeResource(socket, key) {
  const resource = resources.get(key);
  if (!resource) return;
  resource.subscribers.delete(socket);
  if (resource.subscribers.size === 0) {
    clearInterval(resource.timer);
    resources.delete(key);
  }
}

function refreshResource(key) {
  if (resources.has(key)) void pollResource(key);
}

await app.prepare();
const upgradeNext = app.getUpgradeHandler();

const server = createServer((req, res) => handle(req, res));
const wss = new WebSocketServer({ noServer: true });

server.on("upgrade", (req, socket, head) => {
  const path = (req.url || "").split("?")[0];
  if (path === WS_PATH) {
    wss.handleUpgrade(req, socket, head, (ws) => wss.emit("connection", ws, req));
  } else {
    // Next's dev server uses its own socket for hot reload.
    upgradeNext(req, socket, head);
  }
});

wss.on("connection", (socket) => {
  const keys = new Set();
  socket.on("message", (raw) => {
    let msg;
    try {
      msg = JSON.parse(String(raw));
    } catch {
      return;
    }
    if (msg.op === "events") {
      send(socket, { t: "conn", c: backendConnected });
      if (!eventSubscribers.has(socket)) {
        eventSubscribers.add(socket);
        sendReplay(socket);
      }
    } else if (msg.op === "sub" && typeof msg.k === "string") {
      keys.add(msg.k);
      subscribeResource(socket, msg.k);
    } else if (msg.op === "unsub" && typeof msg.k === "string") {
      keys.delete(msg.k);
      unsubscribeResource(socket, msg.k);
    } else if (msg.op === "refresh" && typeof msg.k === "string") {
      refreshResource(msg.k);
    }
  });
  socket.on("close", () => {
    eventSubscribers.delete(socket);
    for (const key of keys) unsubscribeResource(socket, key);
  });
});

server.listen(port, hostname, () => {
  console.log(`> Ready on http://${hostname}:${port} (${dev ? "dev" : "production"}, live updates on ${WS_PATH})`);
  void readEvents();
});
