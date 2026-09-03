import React from "react";
import type { GatewayProcessState, Lang } from "./types";
import { gatewayCommands } from "./gatewayCommands";
import type { GatewayRequestMethod } from "./gatewayCommands";

export type { GatewayRequestMethod } from "./gatewayCommands";

export const GATEWAY_PORT_KEY = "codexx.gateway.port";

export function gatewayText(lang: Lang, zh: string, en: string) {
  return lang === "zh" ? zh : en;
}

export function useGatewayPageState() {
  const [port, setPort] = React.useState(() => Number(localStorage.getItem(GATEWAY_PORT_KEY) || 8787));
  const [processState, setProcessState] = React.useState<GatewayProcessState | null>(null);
  const [busy, setBusy] = React.useState("");
  const [error, setError] = React.useState("");

  const refreshProcess = React.useCallback(async (listenPort = port) => {
    const next = await gatewayCommands.getProcessState(listenPort);
    setProcessState(next);
    return next;
  }, [port]);

  React.useEffect(() => {
    const onPortChange = (event: Event) => {
      const value = Number((event as CustomEvent<number>).detail);
      if (Number.isInteger(value) && value > 0 && value <= 65535) setPort(value);
    };
    const refreshWhenNeeded = () => void refreshProcess().catch((nextError) => setError(String(nextError)));
    const refreshWhenVisible = () => {
      if (document.visibilityState === "visible") refreshWhenNeeded();
    };

    refreshWhenNeeded();
    window.addEventListener("focus", refreshWhenNeeded);
    window.addEventListener("codexx-gateway-state-changed", refreshWhenNeeded);
    window.addEventListener("codexx-gateway-port-changed", onPortChange);
    document.addEventListener("visibilitychange", refreshWhenVisible);
    return () => {
      window.removeEventListener("focus", refreshWhenNeeded);
      window.removeEventListener("codexx-gateway-state-changed", refreshWhenNeeded);
      window.removeEventListener("codexx-gateway-port-changed", onPortChange);
      document.removeEventListener("visibilitychange", refreshWhenVisible);
    };
  }, [refreshProcess]);

  const request = React.useCallback(
    <T,>(method: GatewayRequestMethod, path: string, body?: unknown, listenPort = port) =>
      gatewayCommands.request<T>({
        listenPort,
        method,
        path,
        body: body ?? null,
      }),
    [port],
  );

  const run = React.useCallback(
    async (action: string, fn: () => Promise<unknown>, refreshPort = port) => {
      setBusy(action);
      setError("");
      try {
        const result = await fn();
        await refreshProcess(refreshPort);
        window.dispatchEvent(new Event("codexx-gateway-state-changed"));
        return result;
      } catch (nextError) {
        setError(String(nextError));
        throw nextError;
      } finally {
        setBusy("");
      }
    },
    [port, refreshProcess],
  );

  const clearError = React.useCallback(() => setError(""), []);

  return {
    port,
    setPort,
    processState,
    busy,
    error,
    setError,
    clearError,
    refreshProcess,
    request,
    run,
  };
}
