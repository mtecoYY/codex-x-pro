import type { GatewayProcessState } from "./types";

export type GatewayDisplayMode = "unknown" | "stopped" | "managed" | "disconnected" | "external" | "degraded";

export function gatewayDisplayMode(state: GatewayProcessState | null): GatewayDisplayMode {
  if (!state) return "unknown";
  if (state.degraded) return "degraded";
  if (!state.running) return "stopped";
  if (!state.managedByCodexX) return "external";
  return state.codexRouteActive ? "managed" : "disconnected";
}

export function gatewayCanStart(state: GatewayProcessState | null): boolean {
  const mode = gatewayDisplayMode(state);
  return mode === "stopped" || mode === "external";
}

export function gatewayCanRecover(state: GatewayProcessState | null): boolean {
  return gatewayDisplayMode(state) === "degraded";
}

export function gatewayCanStop(state: GatewayProcessState | null): boolean {
  const mode = gatewayDisplayMode(state);
  return mode === "managed" || mode === "disconnected" || mode === "degraded";
}

export function gatewayControlsDisabled(state: GatewayProcessState | null, busy: boolean): boolean {
  return gatewayDisplayMode(state) !== "managed" || busy;
}

export function gatewayRouteActive(state: GatewayProcessState | null): boolean {
  return gatewayDisplayMode(state) === "managed";
}

export function gatewayUsesRuntime(state: GatewayProcessState | null): boolean {
  const mode = gatewayDisplayMode(state);
  return mode === "managed" || mode === "disconnected";
}
