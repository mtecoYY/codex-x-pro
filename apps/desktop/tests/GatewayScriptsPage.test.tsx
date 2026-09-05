import React from "react";
import { fireEvent, render, screen, waitFor } from "@testing-library/react";
import { beforeEach, describe, expect, it, vi } from "vitest";
import { GatewayScriptsPage } from "../src/pages/GatewayScriptsPage";

const invoke = vi.hoisted(() => vi.fn());

vi.mock("@tauri-apps/api/core", () => ({
  invoke,
}));

const script = {
  id: "repair-tool-call",
  name: "Repair tool call",
  description: "Synthetic test script",
  status: "passed",
  enabled: false,
  priority: 10,
};

const managedState = {
  running: true,
  managedByCodexX: true,
  codexRouteActive: true,
  listenPort: 8788,
  watchdogRunning: true,
  watchdogAutostart: true,
  watchdogDesired: true,
  watchdogRuntime: "running",
};

describe("GatewayScriptsPage", () => {
  beforeEach(() => {
    localStorage.setItem("codexx.gateway.port", "8788");
    invoke.mockReset();
    invoke.mockImplementation(async (command: string, args?: { input?: { path?: string; body?: unknown } }) => {
      if (command === "get_gateway_process_state") return managedState;
      if (command !== "gateway_request") throw new Error(`unexpected command: ${command}`);
      const path = args?.input?.path;
      if (path === "/scripts") return { scripts: [script] };
      if (path === "/scripts/repair-tool-call/test") return { status: "passed" };
      return {};
    });
  });

  it("refreshes, tests, and enables a script through wrapped requests", async () => {
    render(<GatewayScriptsPage lang="en" />);

    const refresh = await screen.findByRole("button", { name: "Refresh scripts" });
    fireEvent.click(refresh);
    await waitFor(() => expect(invoke).toHaveBeenCalledWith("gateway_request", {
      input: {
        listenPort: 8788,
        method: "POST",
        path: "/scripts/refresh",
        body: {},
      },
    }));

    fireEvent.click(screen.getByRole("button", { name: "Test" }));
    fireEvent.click(screen.getByRole("button", { name: "Run test" }));
    await waitFor(() => expect(invoke).toHaveBeenCalledWith("gateway_request", {
      input: {
        listenPort: 8788,
        method: "POST",
        path: "/scripts/repair-tool-call/test",
        body: {},
      },
    }));

    fireEvent.click(screen.getByRole("button", { name: "Close" }));
    fireEvent.click(screen.getByRole("button", { name: "Test" }));
    fireEvent.click(screen.getByRole("tab", { name: "Custom test packet" }));
    fireEvent.change(screen.getByRole("textbox", { name: "Custom raw-text test packet" }), {
      target: { value: "POST /custom HTTP/1.1\r\n\r\nbody" },
    });
    fireEvent.click(screen.getByRole("button", { name: "Run test" }));
    await waitFor(() => expect(invoke).toHaveBeenCalledWith("gateway_request", {
      input: {
        listenPort: 8788,
        method: "POST",
        path: "/scripts/repair-tool-call/test",
        body: {
          source: "custom",
          raw_text: "POST /custom HTTP/1.1\n\nbody",
        },
      },
    }));

    fireEvent.click(screen.getByRole("button", { name: "Enable" }));
    await waitFor(() => expect(invoke).toHaveBeenCalledWith("gateway_request", {
      input: {
        listenPort: 8788,
        method: "POST",
        path: "/scripts/repair-tool-call/enable",
        body: {},
      },
    }));
  });

  it("shows control API failures instead of silently enabling a script", async () => {
    invoke.mockImplementation(async (command: string, args?: { input?: { path?: string } }) => {
      if (command === "get_gateway_process_state") return managedState;
      if (args?.input?.path === "/scripts") return { scripts: [script] };
      if (args?.input?.path === "/scripts/repair-tool-call/enable") {
        throw new Error("SCRIPT_TEST_FAILED: synthetic failure");
      }
      return {};
    });

    render(<GatewayScriptsPage lang="en" />);
    await screen.findByRole("button", { name: "Enable" });
    fireEvent.click(screen.getByRole("button", { name: "Enable" }));

    expect((await screen.findByRole("alert")).textContent).toContain("SCRIPT_TEST_FAILED");
  });

  it("updates priority with a wrapped PUT request and disables enabled scripts", async () => {
    const enabledScript = { ...script, enabled: true };
    invoke.mockImplementation(async (command: string, args?: { input?: { path?: string; body?: unknown } }) => {
      if (command === "get_gateway_process_state") return managedState;
      if (command !== "gateway_request") throw new Error(`unexpected command: ${command}`);
      const path = args?.input?.path;
      if (path === "/scripts") return { scripts: [enabledScript] };
      return {};
    });

    render(<GatewayScriptsPage lang="en" />);
    const priority = await screen.findByDisplayValue("10");
    expect((screen.getByRole("button", { name: "Disable" }) as HTMLButtonElement).disabled).toBe(false);
    expect((priority as HTMLInputElement).disabled).toBe(true);

    invoke.mockImplementation(async (command: string, args?: { input?: { path?: string; body?: unknown } }) => {
      if (command === "get_gateway_process_state") return managedState;
      if (command !== "gateway_request") throw new Error(`unexpected command: ${command}`);
      const path = args?.input?.path;
      if (path === "/scripts") return { scripts: [script] };
      if (path === "/scripts/repair-tool-call/priority") return {};
      return {};
    });

    const refreshed = await screen.findByDisplayValue("10");
    fireEvent.change(refreshed, { target: { value: "20" } });
    fireEvent.blur(refreshed);
    await waitFor(() => expect(invoke).toHaveBeenCalledWith("gateway_request", {
      input: {
        listenPort: 8788,
        method: "PUT",
        path: "/scripts/repair-tool-call/priority",
        body: { priority: 20 },
      },
    }));
  });

  it("keeps discovery and protocol tests available for a running direct or external gateway", async () => {
    const externalState = { ...managedState, managedByCodexX: false, codexRouteActive: false };
    invoke.mockImplementation(async (command: string, args?: { input?: { path?: string; body?: unknown } }) => {
      if (command === "get_gateway_process_state") return externalState;
      if (command !== "gateway_request") throw new Error(`unexpected command: ${command}`);
      const path = args?.input?.path;
      if (path === "/scripts") return { scripts: [script] };
      if (path === "/scripts/refresh") return {};
      if (path === "/scripts/repair-tool-call/test") return { status: "passed" };
      return {};
    });

    const view = render(<GatewayScriptsPage active lang="en" />);

    const refresh = await screen.findByRole("button", { name: "Refresh scripts" });
    const test = screen.getByRole("button", { name: "Test" });
    expect((refresh as HTMLButtonElement).disabled).toBe(false);
    expect((test as HTMLButtonElement).disabled).toBe(false);
    expect((screen.getByRole("button", { name: "Enable" }) as HTMLButtonElement).disabled).toBe(true);

    view.rerender(<GatewayScriptsPage active={false} lang="en" />);
    expect(screen.getByText("Repair tool call")).toBeTruthy();
    view.rerender(<GatewayScriptsPage active lang="en" />);

    fireEvent.click(refresh);
    await waitFor(() => expect(invoke).toHaveBeenCalledWith("gateway_request", {
      input: {
        listenPort: 8788,
        method: "POST",
        path: "/scripts/refresh",
        body: {},
      },
    }));

    fireEvent.click(test);
    fireEvent.click(screen.getByRole("button", { name: "Run test" }));
    await waitFor(() => expect(invoke).toHaveBeenCalledWith("gateway_request", {
      input: {
        listenPort: 8788,
        method: "POST",
        path: "/scripts/repair-tool-call/test",
        body: {},
      },
    }));
  });

  it("renders a detailed raw-text protocol document", async () => {
    render(<GatewayScriptsPage lang="en" />);
    fireEvent.click(await screen.findByRole("button", { name: "Protocol documentation" }));
    expect(screen.getByText(/What the script receives/)).toBeTruthy();
    expect(screen.getByText(/stdin receives the complete HTTP request as raw text/i)).toBeTruthy();
    expect(screen.getByText(/exit 10.*stdout must be a complete HTTP response raw text/i)).toBeTruthy();
    expect(screen.getByText(/manifest\.json example/)).toBeTruthy();
    expect(screen.getByText(/Template example/)).toBeTruthy();
    expect(screen.getByRole("button", { name: "Download protocol" })).toBeTruthy();
  });

  it("starts the protocol download before cleaning up the temporary anchor", async () => {
    vi.useFakeTimers();
    const createObjectURL = vi.fn(() => "blob:protocol");
    const revokeObjectURL = vi.fn();
    vi.stubGlobal("URL", { createObjectURL, revokeObjectURL });
    const click = vi.spyOn(HTMLAnchorElement.prototype, "click").mockImplementation(() => undefined);

    try {
      render(<GatewayScriptsPage lang="en" />);
      fireEvent.click(screen.getByRole("button", { name: "Download protocol" }));

      expect(createObjectURL).toHaveBeenCalledTimes(1);
      expect(click).toHaveBeenCalledTimes(1);
      const anchor = click.mock.instances[0] as HTMLAnchorElement;
      expect(anchor.download).toBe("user-script-raw-text-protocol.md");
      expect(anchor.href).toContain("blob:protocol");
      expect(document.body.contains(anchor)).toBe(true);

      vi.advanceTimersByTime(1000);
      expect(document.body.contains(anchor)).toBe(false);
      expect(revokeObjectURL).toHaveBeenCalledWith("blob:protocol");
    } finally {
      click.mockRestore();
      vi.unstubAllGlobals();
      vi.useRealTimers();
    }
  });
});
