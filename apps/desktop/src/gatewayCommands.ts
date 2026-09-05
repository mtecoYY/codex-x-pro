import { invoke as tauriInvoke } from "@tauri-apps/api/core";
import type { GatewayProcessState } from "./types";

export type GatewayRequestMethod = "GET" | "POST" | "PUT";

export type GatewayStartInput = {
  listenHost: string;
  listenPort: number;
  upstream: string;
  configDir?: string | null;
};

export type GatewayRequestInput = {
  listenPort: number;
  method: GatewayRequestMethod;
  path: string;
  body: unknown;
};

export type GatewayCommandInvoker = <T>(
  command: string,
  args?: Record<string, unknown>,
) => Promise<T>;

export function createGatewayCommands(invoke: GatewayCommandInvoker) {
  return {
    getProcessState(listenPort: number) {
      return invoke<GatewayProcessState>("get_gateway_process_state", { listenPort });
    },

    start(input: GatewayStartInput) {
      return invoke<GatewayProcessState>("start_gateway", { input });
    },

    recover() {
      return invoke<GatewayProcessState>("recover_gateway");
    },

    stop() {
      return invoke<void>("stop_gateway");
    },

    request<T>(input: GatewayRequestInput) {
      return invoke<T>("gateway_request", { input });
    },
  };
}

const invoke: GatewayCommandInvoker = <T>(
  command: string,
  args?: Record<string, unknown>,
) => tauriInvoke<T>(command, args);

export const gatewayCommands = createGatewayCommands(invoke);
