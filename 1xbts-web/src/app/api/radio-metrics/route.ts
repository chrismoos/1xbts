import { cellScopedRequestFromUrl } from "@/lib/cell";
import { getNetworkManagementClient, waitForManagementReady } from "@/lib/grpc/client";

export const dynamic = "force-dynamic";

// Abort a stalled stream so the client reconnects instead of hanging. Metrics
// normally arrive about once a second.
const IDLE_TIMEOUT_MS = 15000;

export async function GET(request: Request) {
  const encoder = new TextEncoder();
  const abort = new AbortController();
  const cellRequest = cellScopedRequestFromUrl(request.url);

  request.signal.addEventListener("abort", () => abort.abort());

  const stream = new ReadableStream({
    async start(controller) {
      let idleTimer: ReturnType<typeof setTimeout> | undefined;
      const armIdleTimeout = () => {
        clearTimeout(idleTimer);
        idleTimer = setTimeout(() => {
          console.log("[radio-metrics] idle timeout, aborting");
          abort.abort();
        }, IDLE_TIMEOUT_MS);
      };

      const send = (chunk: string) => {
        if (!abort.signal.aborted) {
          try {
            controller.enqueue(encoder.encode(chunk));
          } catch {
            abort.abort();
          }
        }
      };

      send("retry: 2000\n\n");

      try {
        await waitForManagementReady();
        console.log("[radio-metrics] starting gRPC stream");
        const client = getNetworkManagementClient();
        armIdleTimeout();
        for await (const metrics of client.streamRadioMetrics(
          cellRequest,
          { signal: abort.signal }
        )) {
          if (abort.signal.aborted) break;
          armIdleTimeout();
          send(`data: ${JSON.stringify(metrics)}\n\n`);
        }
        console.log("[radio-metrics] gRPC stream ended");
      } catch (err) {
        if (!abort.signal.aborted) {
          const msg = err instanceof Error ? err.message : "unknown error";
          console.log(`[radio-metrics] gRPC error: ${msg}`);
          send(`data: ${JSON.stringify({ error: msg })}\n\n`);
        } else {
          console.log("[radio-metrics] aborted");
        }
      }
      clearTimeout(idleTimer);
      try {
        controller.close();
      } catch {
        // already closed
      }
    },
    cancel() {
      console.log("[radio-metrics] client disconnected");
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
