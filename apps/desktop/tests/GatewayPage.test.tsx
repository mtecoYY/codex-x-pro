import React from "react";
import { fireEvent, render, screen, waitFor } from "@testing-library/react";
import { beforeEach, describe, expect, it, vi } from "vitest";
import { GatewayPage } from "../src/pages/GatewayPage";
import type { GatewayProcessState } from "../src/types";

const invoke = vi.hoisted(() => vi.fn());

vi.mock("@tauri-apps/api/core", () => ({
  invoke,
}));

const stoppedState: GatewayProcessState = {
  running: false,
  managedByCodexX: false,
  codexRouteActive: false,
  listenPort: 8788,
  watchdogRunning: false,
  watchdogAutostart: false,
  watchdogDesired: false,
  watchdogRuntime: "stopped",
};

describe("GatewayPage", () => {
  beforeEach(() => {
    invoke.mockReset();
    invoke.mockImplementation(async (command: string) => {
      if (command === "get_gateway_process_state") return stoppedState;
      if (command === "start_gateway") return { ...stoppedState, running: true, managedByCodexX: true };
      if (command === "stop_gateway") return undefined;
      throw new Error(`unexpected command: ${command}`);
    });
  });

  it("does not present start or stop controls before gateway state is known", async () => {
    let resolveState!: (value: GatewayProcessState) => void;
    invoke.mockImplementation((command: string) => {
      if (command === "get_gateway_process_state") return new Promise<GatewayProcessState>((resolve) => { resolveState = resolve; });
      throw new Error(`unexpected command: ${command}`);
    });

    render(<GatewayPage lang="en" />);
    expect(screen.getByRole("button", { name: "Check status" })).toBeTruthy();
    expect(screen.queryByRole("button", { name: "Start gateway" })).toBeNull();
    expect(screen.queryByRole("button", { name: "Stop gateway" })).toBeNull();

    resolveState(stoppedState);
    await waitFor(() => expect(screen.getByRole("button", { name: "Start gateway" })).toBeTruthy());
  });

  it("shows degraded recovery and direct-mode escape actions", async () => {
    let recovered = false;
    invoke.mockImplementation(async (command: string) => {
      if (command === "get_gateway_process_state") return { ...stoppedState, managedByCodexX: true, degraded: true, error: "GATEWAY_DEGRADED: recovery failed" };
      if (command === "recover_gateway") {
        recovered = true;
        return { ...stoppedState, running: true, managedByCodexX: true, codexRouteActive: true, degraded: false };
      }
      if (command === "stop_gateway") return undefined;
      throw new Error(`unexpected command: ${command}`);
    });

    render(<GatewayPage lang="en" />);
    expect(await screen.findByText("Gateway recovery required")).toBeTruthy();
    expect(screen.getByRole("button", { name: "Recover gateway" })).toBeTruthy();
    expect(screen.getByRole("button", { name: "Stop and restore direct mode" })).toBeTruthy();
    expect(screen.getByRole("alert").textContent).toContain("GATEWAY_DEGRADED");

    fireEvent.click(screen.getByRole("button", { name: "Recover gateway" }));
    await waitFor(() => expect(invoke).toHaveBeenCalledWith("recover_gateway", undefined));
    expect(recovered).toBe(true);
  });

  it("keeps the direct-mode escape available when recovery fails", async () => {
    invoke.mockImplementation(async (command: string) => {
      if (command === "get_gateway_process_state") return { ...stoppedState, managedByCodexX: true, degraded: true, error: "GATEWAY_DEGRADED: recovery failed" };
      if (command === "recover_gateway") throw new Error("GATEWAY_RECOVERY_FAILED: still unavailable");
      if (command === "stop_gateway") return undefined;
      throw new Error(`unexpected command: ${command}`);
    });

    render(<GatewayPage lang="en" />);
    fireEvent.click(await screen.findByRole("button", { name: "Recover gateway" }));
    expect((await screen.findByRole("alert")).textContent).toContain("GATEWAY_RECOVERY_FAILED");
    expect(screen.getByRole("button", { name: "Recover gateway" })).toBeTruthy();
    expect(screen.getByRole("button", { name: "Stop and restore direct mode" })).toBeTruthy();
  });

  it("uses stop gateway as the direct-mode escape from degraded state", async () => {
    let stopped = false;
    invoke.mockImplementation(async (command: string) => {
      if (command === "get_gateway_process_state") {
        return stopped
          ? stoppedState
          : { ...stoppedState, managedByCodexX: true, degraded: true, error: "GATEWAY_DEGRADED: recovery failed" };
      }
      if (command === "stop_gateway") {
        stopped = true;
        return undefined;
      }
      throw new Error(`unexpected command: ${command}`);
    });

    render(<GatewayPage lang="en" />);
    fireEvent.click(await screen.findByRole("button", { name: "Stop and restore direct mode" }));
    await waitFor(() => expect(invoke).toHaveBeenCalledWith("stop_gateway", undefined));
    expect(await screen.findByRole("button", { name: "Start gateway" })).toBeTruthy();
    expect(screen.queryByRole("button", { name: "Recover gateway" })).toBeNull();
  });

  it("sends the wrapped start payload from the real page interaction", async () => {
    render(<GatewayPage lang="en" configDir="C:/Codex-Test" />);

    await waitFor(() => expect((screen.getByRole("button", { name: "Start gateway" }) as HTMLButtonElement).disabled).toBe(false));
    fireEvent.change(screen.getByDisplayValue("8787"), { target: { value: "8788" } });
    fireEvent.click(screen.getByRole("button", { name: "Start gateway" }));

    await waitFor(() => expect(invoke).toHaveBeenCalledWith("start_gateway", {
      input: {
        listenHost: "127.0.0.1",
        listenPort: 8788,
        upstream: "https://newapi.gogogogoapp.mom",
        configDir: "C:/Codex-Test",
      },
    }));
    expect(invoke).not.toHaveBeenCalledWith("start_gateway", expect.objectContaining({
      input: expect.objectContaining({ input: expect.anything() }),
    }));
  });

  it("shows the actual startup error and does not report a false running state", async () => {
    invoke.mockImplementation(async (command: string) => {
      if (command === "get_gateway_process_state") return stoppedState;
      if (command === "start_gateway") throw new Error("WATCHDOG_TASK_START_FAILED: synthetic test failure");
      throw new Error(`unexpected command: ${command}`);
    });

    render(<GatewayPage lang="en" />);
    await waitFor(() => expect((screen.getByRole("button", { name: "Start gateway" }) as HTMLButtonElement).disabled).toBe(false));
    fireEvent.click(screen.getByRole("button", { name: "Start gateway" }));

    expect((await screen.findByRole("alert")).textContent).toContain("WATCHDOG_TASK_START_FAILED");
    expect(screen.getByRole("button", { name: "Start gateway" })).toBeTruthy();
    expect((screen.getByRole("button", { name: "Start gateway" }) as HTMLButtonElement).disabled).toBe(false);
  });

  it("sends the stop command without an argument wrapper", async () => {
    invoke.mockImplementation(async (command: string) => {
      if (command === "get_gateway_process_state") return {
        ...stoppedState,
        running: true,
        managedByCodexX: true,
        codexRouteActive: true,
        watchdogRunning: true,
        watchdogAutostart: true,
        watchdogDesired: true,
        watchdogRuntime: "running",
      };
      if (command === "stop_gateway") return undefined;
      throw new Error(`unexpected command: ${command}`);
    });

    render(<GatewayPage lang="en" />);
    const stop = await screen.findByRole("button", { name: "Stop gateway" });
    fireEvent.click(stop);

    await waitFor(() => expect(invoke).toHaveBeenCalledWith("stop_gateway", undefined));
    expect(invoke).not.toHaveBeenCalledWith("stop_gateway", expect.any(Object));
    expect((screen.getByRole("button", { name: "Stop gateway" }) as HTMLButtonElement).disabled).toBe(false);
  });

  it("does not report an unavailable control API after a successful stop", async () => {
    let stopped = false;
    invoke.mockImplementation(async (command: string) => {
      if (command === "get_gateway_process_state") {
        if (stopped) {
          return {
            ...stoppedState,
            error: "CONTROL_API_UNAVAILABLE: connection refused",
            degraded: false,
          };
        }
        return {
          ...stoppedState,
          running: true,
          managedByCodexX: true,
          codexRouteActive: true,
        };
      }
      if (command === "stop_gateway") {
        stopped = true;
        return undefined;
      }
      throw new Error(`unexpected command: ${command}`);
    });

    render(<GatewayPage lang="en" />);
    fireEvent.click(await screen.findByRole("button", { name: "Stop gateway" }));

    await screen.findByRole("button", { name: "Start gateway" });
    expect(screen.queryByRole("alert")).toBeNull();
  });

  it("continues to report a real stop transaction failure", async () => {
    invoke.mockImplementation(async (command: string) => {
      if (command === "get_gateway_process_state") {
        return {
          ...stoppedState,
          running: true,
          managedByCodexX: true,
          codexRouteActive: true,
        };
      }
      if (command === "stop_gateway") {
        throw new Error("DIRECT_CONFIG_WRITE_CONFLICT: concurrent change");
      }
      throw new Error(`unexpected command: ${command}`);
    });

    render(<GatewayPage lang="en" />);
    fireEvent.click(await screen.findByRole("button", { name: "Stop gateway" }));

    expect((await screen.findByRole("alert")).textContent).toContain("DIRECT_CONFIG_WRITE_CONFLICT");
  });

  it("reports an unavailable control API when gateway mode still expects it", async () => {
    invoke.mockImplementation(async (command: string) => {
      if (command === "get_gateway_process_state") {
        return {
          ...stoppedState,
          managedByCodexX: true,
          degraded: true,
          error: "CONTROL_API_UNAVAILABLE: connection refused",
        };
      }
      throw new Error(`unexpected command: ${command}`);
    });

    render(<GatewayPage lang="en" />);

    expect((await screen.findByRole("alert")).textContent).toContain("CONTROL_API_UNAVAILABLE");
  });

  it("shows a managed gateway as disconnected when Codex is routed elsewhere", async () => {
    invoke.mockImplementation(async (command: string) => {
      if (command === "get_gateway_process_state") {
        return {
          ...stoppedState,
          running: true,
          managedByCodexX: true,
          codexRouteActive: false,
          watchdogRunning: true,
          watchdogAutostart: true,
          watchdogDesired: true,
          watchdogRuntime: "running",
        };
      }
      throw new Error(`unexpected command: ${command}`);
    });

    render(<GatewayPage lang="en" />);

    expect(await screen.findByText("Gateway running but not connected to Codex")).toBeTruthy();
    expect(screen.getByRole("button", { name: "Stop gateway" })).toBeTruthy();
  });

  it("rejects invalid ports before invoking the native command", async () => {
    render(<GatewayPage lang="en" />);
    await waitFor(() => expect((screen.getByRole("button", { name: "Start gateway" }) as HTMLButtonElement).disabled).toBe(false));
    fireEvent.change(screen.getByDisplayValue("8787"), { target: { value: "70000" } });
    fireEvent.click(screen.getByRole("button", { name: "Start gateway" }));

    expect((await screen.findByRole("alert")).textContent).toContain("Listen port must be an integer from 1 to 65535");
    expect(invoke).not.toHaveBeenCalledWith("start_gateway", expect.anything());
  });

  it("shows the backend process error and runtime submission status", async () => {
    const state: GatewayProcessState = {
      ...stoppedState,
      running: true,
      managedByCodexX: true,
      codexRouteActive: true,
      state: {
        provider: { version: 4 },
        instruction: { version: 7 },
      },
      degraded: true,
      error: "GATEWAY_RUNTIME_SYNC_FAILED: provider state was rejected",
    };
    invoke.mockImplementation(async (command: string) => {
      if (command === "get_gateway_process_state") return state;
      throw new Error(`unexpected command: ${command}`);
    });

    render(<GatewayPage lang="en" />);

    expect(await screen.findByText("Provider 4 · Instructions 7")).toBeTruthy();
    expect(screen.getByRole("alert").textContent).toContain("GATEWAY_RUNTIME_SYNC_FAILED");
  });

  it("keeps gateway form state when the page is hidden and shown again", async () => {
    const view = render(<GatewayPage active lang="en" />);
    await screen.findByRole("button", { name: "Start gateway" });

    fireEvent.change(screen.getByDisplayValue("https://newapi.gogogogoapp.mom"), {
      target: { value: "http://127.0.0.1:19090" },
    });
    fireEvent.change(screen.getByDisplayValue("8787"), { target: { value: "8888" } });

    view.rerender(<GatewayPage active={false} lang="en" />);
    expect((screen.getByDisplayValue("8888") as HTMLInputElement).value).toBe("8888");
    expect((screen.getByDisplayValue("http://127.0.0.1:19090") as HTMLInputElement).value).toBe("http://127.0.0.1:19090");

    view.rerender(<GatewayPage active lang="en" />);
    expect((screen.getByDisplayValue("8888") as HTMLInputElement).value).toBe("8888");
  });
});
