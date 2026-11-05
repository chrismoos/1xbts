import { createChannel } from "nice-grpc";
import { grpcClients } from "./rich-client";
import { EventServiceDefinition } from "../proto/events/v1/service";

const EVENTS_GRPC_ADDRESS =
  process.env.EVENTS_GRPC_ADDRESS || "127.0.0.1:17023";

const eventsChannel = createChannel(EVENTS_GRPC_ADDRESS, undefined, {
  "grpc.initial_reconnect_backoff_ms": 100,
  "grpc.max_reconnect_backoff_ms": 1000,
});

export function getEventBusClient() {
  return grpcClients.create(EventServiceDefinition, eventsChannel);
}
