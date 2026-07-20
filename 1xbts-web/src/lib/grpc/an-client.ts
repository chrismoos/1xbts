import { createChannel, createClient } from "nice-grpc";
import { AnServiceDefinition } from "../proto/an/v1/service";

const AN_GRPC_ADDRESS = process.env.AN_GRPC_ADDRESS || "127.0.0.1:17030";

const anChannel = createChannel(AN_GRPC_ADDRESS, undefined, {
  "grpc.initial_reconnect_backoff_ms": 100,
  "grpc.max_reconnect_backoff_ms": 1000,
});

export function getAnClient() {
  return createClient(AnServiceDefinition, anChannel);
}
