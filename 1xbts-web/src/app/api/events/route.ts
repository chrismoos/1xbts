import {
  getManagementFacadeClient,
  getMscManagementClient,
  waitForBscReady,
} from "@/lib/grpc/client";
import { getEventBusClient } from "@/lib/grpc/events-bus-client";
import { shouldHideAccessEvent } from "@/lib/access-event-filter";
import { EventSource } from "@/lib/proto/events/v1/service";
import { HrpdTrafficReason, HrpdUati } from "@/lib/proto/events/v1/an";
import type {
  HrpdAccessEvent,
  HrpdDecodedMessage,
  HrpdSessionEvent,
  HrpdTrafficEvent,
} from "@/lib/proto/events/v1/an";

export const dynamic = "force-dynamic";

const SSE_RETRY_MS = 2000;
const STREAM_RETRY_BASE_MS = 1000;
const STREAM_RETRY_MAX_MS = 5000;
const STREAM_READY_TIMEOUT_MS = 1500;
const KEEPALIVE_MS = 15000;

function compactHrpdDecodedMessage(message: HrpdDecodedMessage) {
  return {
    typeName: message.typeName,
    summary: message.summary,
    protocolType: message.protocolType,
    messageId: message.messageId,
    payloadLengthBytes: message.payload?.length ?? 0,
  };
}

function isNoisyHrpdDecodedMessage(message: HrpdDecodedMessage) {
  return message.typeName === "Ack" || message.typeName === "DefaultPacketRlpNak";
}

function compactHrpdSessionEvent(event: HrpdSessionEvent) {
  return {
    ...event,
    fullUati: event.fullUati ? HrpdUati.toJSON(event.fullUati) : undefined,
  };
}

function compactHrpdAccessEvent(event: HrpdAccessEvent) {
  return {
    timestampNs: event.timestampNs,
    accessSignature: event.accessSignature,
    reason: event.reason,
    colorCode: event.colorCode,
    direction: event.direction,
    decodedMessages: event.decodedMessages.map(compactHrpdDecodedMessage),
    payloadLengthBytes: event.payloadLengthBytes || event.payload?.length || 0,
    uati: event.uati,
    fullUati: event.fullUati ? HrpdUati.toJSON(event.fullUati) : undefined,
    receiveAti: event.receiveAti,
  };
}

function compactHrpdTrafficEvent(event: HrpdTrafficEvent) {
  return {
    timestampNs: event.timestampNs,
    uati: event.uati,
    reason: event.reason,
    macIndex: event.macIndex,
    drcValue: event.drcValue,
    reversePilotSnrDbTenths: event.reversePilotSnrDbTenths,
    direction: event.direction,
    decodedMessages: event.decodedMessages
      .filter((message) => !isNoisyHrpdDecodedMessage(message))
      .map(compactHrpdDecodedMessage),
    payloadLengthBytes: event.payloadLengthBytes || event.payload?.length || 0,
    fullUati: event.fullUati ? HrpdUati.toJSON(event.fullUati) : undefined,
    receiveAti: event.receiveAti,
  };
}

function shouldForwardHrpdTrafficEvent(event: HrpdTrafficEvent) {
  if (event.reason === HrpdTrafficReason.HRPD_TRAFFIC_REASON_ACK_RECEIVED) {
    return false;
  }
  const hasVisibleMessage = event.decodedMessages.some(
    (message) => !isNoisyHrpdDecodedMessage(message)
  );
  return (
    hasVisibleMessage ||
    event.reason === HrpdTrafficReason.HRPD_TRAFFIC_REASON_DRC_UPDATED ||
    event.reason === HrpdTrafficReason.HRPD_TRAFFIC_REASON_REVERSE_PILOT_SNR_UPDATED ||
    event.reason === HrpdTrafficReason.HRPD_TRAFFIC_REASON_CONNECTION_CLOSE
  );
}

/**
 * Unified SSE endpoint that multiplexes radio-metrics, paging-events,
 * traffic-events, and access-events into a single HTTP connection. Each event
 * type is sent as a named SSE event so clients can subscribe selectively.
 *
 * This avoids hitting the browser's 6-connection-per-origin HTTP/1.1 limit
 * when multiple tabs or pages each need live data.
 */
export async function GET(request: Request) {
  const encoder = new TextEncoder();
  const abort = new AbortController();

  request.signal.addEventListener("abort", () => abort.abort());

  const stream = new ReadableStream({
    async start(controller) {
      const send = (chunk: string) => {
        if (!abort.signal.aborted) {
          try {
            controller.enqueue(encoder.encode(chunk));
          } catch {
            abort.abort();
          }
        }
      };

      const sleep = (ms: number) =>
        new Promise<void>((resolve) => {
          const timer = setTimeout(() => {
            abort.signal.removeEventListener("abort", onAbort);
            resolve();
          }, ms);
          const onAbort = () => {
            clearTimeout(timer);
            abort.signal.removeEventListener("abort", onAbort);
            resolve();
          };
          abort.signal.addEventListener("abort", onAbort);
        });

      let backendConnected = false;
      const publishBackendConnected = () => {
        if (backendConnected) return;
        backendConnected = true;
        send(
          `event: connection\ndata: ${JSON.stringify({ connected: backendConnected })}\n\n`
        );
      };

      const setBackendDisconnected = () => {
        if (!backendConnected) return;
        backendConnected = false;
        send('event: connection\ndata: {"connected":false}\n\n');
      };

      const runFacadeStream = async () => {
        let retryMs = STREAM_RETRY_BASE_MS;
        while (!abort.signal.aborted) {
          try {
            await waitForBscReady(STREAM_READY_TIMEOUT_MS);
            console.log("[events] starting management facade stream");
            const client = getManagementFacadeClient();
            for await (const value of client.streamSystemEvents(
              {},
              { signal: abort.signal }
            )) {
              if (abort.signal.aborted) {
                break;
              }
              const metadata = {
                sourceNodeId: value.sourceNodeId,
                sourceNodeType: value.sourceNodeType,
                classification: value.classification,
              };
              if (value.radioMetrics) {
                retryMs = STREAM_RETRY_BASE_MS;
                publishBackendConnected();
                send(
                  `event: radio-metrics\ndata: ${JSON.stringify({
                    ...value.radioMetrics,
                    management: metadata,
                  })}\n\n`
                );
                continue;
              }
              if (value.pagingEvent) {
                retryMs = STREAM_RETRY_BASE_MS;
                publishBackendConnected();
                send(
                  `event: paging\ndata: ${JSON.stringify({
                    ...value.pagingEvent,
                    management: metadata,
                  })}\n\n`
                );
                continue;
              }
              if (value.accessEvent) {
                if (shouldHideAccessEvent(value.accessEvent)) {
                  continue;
                }
                retryMs = STREAM_RETRY_BASE_MS;
                publishBackendConnected();
                send(
                  `event: access\ndata: ${JSON.stringify({
                    ...value.accessEvent,
                    management: metadata,
                  })}\n\n`
                );
                continue;
              }
              if (value.trafficEvent) {
                retryMs = STREAM_RETRY_BASE_MS;
                publishBackendConnected();
                send(
                  `event: traffic\ndata: ${JSON.stringify({
                    ...value.trafficEvent,
                    management: metadata,
                  })}\n\n`
                );
              }
            }
            if (abort.signal.aborted) {
              break;
            }
            console.log("[events] management facade stream ended");
          } catch (err) {
            if (abort.signal.aborted) {
              break;
            }
            const msg = err instanceof Error ? err.message : "unknown";
            console.log(`[events] management facade error: ${msg}`);
          }

          setBackendDisconnected();
          if (abort.signal.aborted) {
            break;
          }

          await sleep(retryMs);
          retryMs = Math.min(retryMs * 2, STREAM_RETRY_MAX_MS);
        }
      };

      const runOtaspStream = async () => {
        let retryMs = STREAM_RETRY_BASE_MS;
        while (!abort.signal.aborted) {
          try {
            const client = getMscManagementClient();
            for await (const value of client.streamOtaspEvents(
              {},
              { signal: abort.signal }
            )) {
              if (abort.signal.aborted) {
                break;
              }
              if (value.otasp) {
                retryMs = STREAM_RETRY_BASE_MS;
                send(
                  `event: otasp\ndata: ${JSON.stringify(value.otasp)}\n\n`
                );
              }
            }
            if (abort.signal.aborted) {
              break;
            }
            console.log("[events] msc otasp stream ended");
          } catch (err) {
            if (abort.signal.aborted) {
              break;
            }
            const msg = err instanceof Error ? err.message : "unknown";
            console.log(`[events] msc otasp stream error: ${msg}`);
          }
          if (abort.signal.aborted) {
            break;
          }
          await sleep(retryMs);
          retryMs = Math.min(retryMs * 2, STREAM_RETRY_MAX_MS);
        }
      };

      // HRPD/AN events arrive on the aggregated event bus rather than the BSC
      // facade. Re-emit the typed session/access/traffic arms as named SSE
      // events, carrying the bus-resolved subscriber/identity for the UI.
      const runEventBusStream = async () => {
        let retryMs = STREAM_RETRY_BASE_MS;
        while (!abort.signal.aborted) {
          try {
            const client = getEventBusClient();
            for await (const value of client.listenEvents(
              { sourceFilter: [EventSource.EVENT_SOURCE_AN] },
              { signal: abort.signal }
            )) {
              if (abort.signal.aborted) {
                break;
              }
              const an = value.an;
              if (!an) {
                continue;
              }
              const enrich = {
                subscriber: value.subscriber,
                identity: value.identity,
                sequence: value.sequence,
              };
              if (an.session) {
                retryMs = STREAM_RETRY_BASE_MS;
                send(
                  `event: hrpd-session\ndata: ${JSON.stringify({
                    ...compactHrpdSessionEvent(an.session),
                    ...enrich,
                  })}\n\n`
                );
              } else if (an.access) {
                retryMs = STREAM_RETRY_BASE_MS;
                send(
                  `event: hrpd-access\ndata: ${JSON.stringify({
                    ...compactHrpdAccessEvent(an.access),
                    ...enrich,
                  })}\n\n`
                );
              } else if (an.traffic) {
                if (!shouldForwardHrpdTrafficEvent(an.traffic)) {
                  continue;
                }
                retryMs = STREAM_RETRY_BASE_MS;
                send(
                  `event: hrpd-traffic\ndata: ${JSON.stringify({
                    ...compactHrpdTrafficEvent(an.traffic),
                    ...enrich,
                  })}\n\n`
                );
              }
            }
            if (abort.signal.aborted) {
              break;
            }
            console.log("[events] event bus stream ended");
          } catch (err) {
            if (abort.signal.aborted) {
              break;
            }
            const msg = err instanceof Error ? err.message : "unknown";
            console.log(`[events] event bus error: ${msg}`);
          }
          if (abort.signal.aborted) {
            break;
          }
          await sleep(retryMs);
          retryMs = Math.min(retryMs * 2, STREAM_RETRY_MAX_MS);
        }
      };

      send(`retry: ${SSE_RETRY_MS}\n\n`);
      send('event: connection\ndata: {"connected":false}\n\n');

      const keepalive = setInterval(() => {
        send(": keepalive\n\n");
      }, KEEPALIVE_MS);

      try {
        await Promise.all([
          runFacadeStream(),
          runOtaspStream(),
          runEventBusStream(),
        ]);
      } finally {
        clearInterval(keepalive);
        console.log("[events] management facade stream stopped");
        try {
          controller.close();
        } catch {
          // already closed
        }
      }
    },
    cancel() {
      console.log("[events] client disconnected");
      abort.abort();
    },
  });

  return new Response(stream, {
    headers: {
      "Content-Type": "text/event-stream",
      "Cache-Control": "no-cache, no-transform",
      Connection: "keep-alive",
      "X-Accel-Buffering": "no",
    },
  });
}
