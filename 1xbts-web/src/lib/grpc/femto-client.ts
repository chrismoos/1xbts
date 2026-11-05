import { createChannel } from "nice-grpc";
import { grpcClients } from "./rich-client";
import { FemtoManagementServiceDefinition } from "../proto/femto_management/v1/service";

export function getFemtoManagementClient(address: string) {
  const channel = createChannel(address, undefined, {
    "grpc.initial_reconnect_backoff_ms": 100,
    "grpc.max_reconnect_backoff_ms": 1000,
  });
  return grpcClients.create(FemtoManagementServiceDefinition, channel);
}
