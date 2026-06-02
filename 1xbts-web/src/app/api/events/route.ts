import {
  getManagementFacadeClient,
  getMscManagementClient,
  waitForBscReady,
} from "@/lib/grpc/client";
import { shouldHideAccessEvent } from "@/lib/access-event-filter";

export const dynamic = "force-dynamic";

const SSE_RETRY_MS = 2000;
const STREAM_RETRY_BASE_MS = 1000;
const STREAM_RETRY_MAX_MS = 5000;
const STREAM_READY_TIMEOUT_MS = 1500;
const KEEPALIVE_MS = 15000;

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

      send(`retry: ${SSE_RETRY_MS}\n\n`);
      send('event: connection\ndata: {"connected":false}\n\n');

      const keepalive = setInterval(() => {
        send(": keepalive\n\n");
      }, KEEPALIVE_MS);

      try {
        await Promise.all([runFacadeStream(), runOtaspStream()]);
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
