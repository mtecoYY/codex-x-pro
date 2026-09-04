import React from "react";
import { fireEvent, render, screen, waitFor } from "@testing-library/react";
import { beforeEach, describe, expect, it, vi } from "vitest";
import { GatewayObservePage } from "../src/pages/GatewayObservePage";

const invoke = vi.hoisted(() => vi.fn());

vi.mock("@tauri-apps/api/core", () => ({
  invoke,
}));

class FakeEventSource {
  static instances: FakeEventSource[] = [];
  closed = false;
  listeners = new Map<string, (event: MessageEvent) => void>();

  constructor(public url: string) {
    FakeEventSource.instances.push(this);
  }

  addEventListener(type: string, listener: (event: MessageEvent) => void) {
    this.listeners.set(type, listener);
  }
  emit(type: string, data: unknown) {
    this.listeners.get(type)?.({ data: JSON.stringify(data) } as MessageEvent);
  }
  close() {
    this.closed = true;
  }
}

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

const observeState = {
  capture_enabled: false,
  capture_limit: 100,
  capture_total_bytes: 2 * 1024 * 1024 * 1024,
  capture_record_max_bytes: 1024 * 1024 * 1024,
  stored_bytes: 0,
  retained_count: 0,
  evicted_count: 0,
  capture_dropped_count: 0,
  next_seq: 1,
};

describe("GatewayObservePage", () => {
  beforeEach(() => {
    vi.stubGlobal("EventSource", FakeEventSource);
    FakeEventSource.instances = [];
    localStorage.setItem("codexx.gateway.port", "8788");
    invoke.mockReset();
    invoke.mockImplementation(async (command: string, args?: { input?: { path?: string } }) => {
      if (command === "get_gateway_process_state") return managedState;
      if (command !== "gateway_request") throw new Error(`unexpected command: ${command}`);
      const path = args?.input?.path;
      if (path === "/observe/state") return observeState;
      if (path === "/observe/requests") return { requests: [] };
      return {};
    });
  });

  it("loads the managed observation page and sends wrapped control requests", async () => {
    render(<GatewayObservePage lang="en" />);

    const startCapture = await screen.findByRole("button", { name: "Start capture" });
    expect((startCapture as HTMLButtonElement).disabled).toBe(false);
    fireEvent.click(startCapture);

    await waitFor(() => expect(invoke).toHaveBeenCalledWith("gateway_request", {
      input: {
        listenPort: 8788,
        method: "POST",
        path: "/observe/start",
        body: {},
      },
    }));

    fireEvent.change(screen.getByDisplayValue("100"), { target: { value: "10" } });
    fireEvent.click(screen.getByRole("button", { name: "Save retention limit" }));
    await waitFor(() => expect(invoke).toHaveBeenCalledWith("gateway_request", {
      input: {
        listenPort: 8788,
        method: "PUT",
        path: "/observe/settings",
        body: { capture_limit: 10 },
      },
    }));
  });

  it("disables observation controls outside managed gateway mode", async () => {
    invoke.mockImplementation(async (command: string) => {
      if (command === "get_gateway_process_state") {
        return { ...managedState, running: false, managedByCodexX: false };
      }
      throw new Error(`unexpected command: ${command}`);
    });

    render(<GatewayObservePage lang="en" />);
    const startCapture = await screen.findByRole("button", { name: "Start capture" });
    expect((startCapture as HTMLButtonElement).disabled).toBe(true);
    expect(screen.getByText("Start gateway mode first")).toBeTruthy();
    fireEvent.click(startCapture);
    expect(invoke).not.toHaveBeenCalledWith("gateway_request", expect.anything());
  });

  it("keeps observation disabled when the managed gateway is not Codex's active route", async () => {
    invoke.mockImplementation(async (command: string) => {
      if (command === "get_gateway_process_state") {
        return { ...managedState, codexRouteActive: false };
      }
      throw new Error(`unexpected command: ${command}`);
    });

    render(<GatewayObservePage lang="en" />);
    const startCapture = await screen.findByRole("button", { name: "Start capture" });
    expect((startCapture as HTMLButtonElement).disabled).toBe(true);
    expect(screen.getByText("Gateway not connected to Codex")).toBeTruthy();
    fireEvent.click(startCapture);
    expect(invoke).not.toHaveBeenCalledWith("gateway_request", expect.anything());
  });

  it("sends pause and clear requests with the selected port", async () => {
    let currentObserve = { ...observeState, capture_enabled: true };
    invoke.mockImplementation(async (command: string, args?: { input?: { path?: string } }) => {
      if (command === "get_gateway_process_state") return managedState;
      if (command !== "gateway_request") throw new Error(`unexpected command: ${command}`);
      const path = args?.input?.path;
      if (path === "/observe/state") return currentObserve;
      if (path === "/observe/requests") return { requests: [] };
      if (path === "/observe/pause") {
        currentObserve = { ...currentObserve, capture_enabled: false };
        return {};
      }
      if (path === "/observe/clear") return {};
      return {};
    });

    render(<GatewayObservePage lang="en" />);
    const pause = await screen.findByRole("button", { name: "Pause" });
    fireEvent.click(pause);
    await waitFor(() => expect(invoke).toHaveBeenCalledWith("gateway_request", {
      input: {
        listenPort: 8788,
        method: "POST",
        path: "/observe/pause",
        body: {},
      },
    }));

    fireEvent.click(await screen.findByRole("button", { name: "Clear" }));
    await waitFor(() => expect(invoke).toHaveBeenCalledWith("gateway_request", {
      input: {
        listenPort: 8788,
        method: "POST",
        path: "/observe/clear",
        body: {},
      },
    }));
  });

  it("shows observation control failures and validates the retention limit locally", async () => {
    invoke.mockImplementation(async (command: string, args?: { input?: { path?: string } }) => {
      if (command === "get_gateway_process_state") return managedState;
      if (args?.input?.path === "/observe/state") return observeState;
      if (args?.input?.path === "/observe/requests") return { requests: [] };
      if (args?.input?.path === "/observe/start") throw new Error("OBSERVE_GATEWAY_REQUIRED: synthetic failure");
      return {};
    });

    render(<GatewayObservePage lang="en" />);
    fireEvent.click(await screen.findByRole("button", { name: "Start capture" }));
    expect((await screen.findByRole("alert")).textContent).toContain("OBSERVE_GATEWAY_REQUIRED");
    expect((screen.getByRole("button", { name: "Start capture" }) as HTMLButtonElement).disabled).toBe(false);

    fireEvent.change(screen.getByDisplayValue("100"), { target: { value: "not-a-number" } });
    fireEvent.click(screen.getByRole("button", { name: "Save retention limit" }));
    expect((await screen.findByRole("alert")).textContent).toContain("Enter an integer");
    expect(invoke).not.toHaveBeenCalledWith("gateway_request", expect.objectContaining({
      input: expect.objectContaining({ path: "/observe/settings" }),
    }));
  });

  it("keeps retained rows visible across page switches and closes SSE while inactive", async () => {
    const row = {
      id: 1,
      channel: "synthetic",
      status_code: 200,
      model: "test-model",
      request_time_ms: 12,
      first_token_ms: 4,
      tokens: 8,
      ok: true,
    };
    let currentProcessState = managedState;
    invoke.mockImplementation(async (command: string, args?: { input?: { path?: string } }) => {
      if (command === "get_gateway_process_state") return currentProcessState;
      if (command !== "gateway_request") throw new Error(`unexpected command: ${command}`);
      const path = args?.input?.path;
      if (path === "/observe/state") return { ...observeState, retained_count: 1 };
      if (path === "/observe/requests") return { requests: [row] };
      return {};
    });

    const view = render(<GatewayObservePage active lang="en" />);
    expect(await screen.findByText("test-model")).toBeTruthy();
    expect(FakeEventSource.instances.length).toBeGreaterThan(0);
    const activeSource = FakeEventSource.instances[FakeEventSource.instances.length - 1];

    view.rerender(<GatewayObservePage active={false} lang="en" />);
    await waitFor(() => expect(activeSource.closed).toBe(true));
    expect(screen.getByText("test-model")).toBeTruthy();
    expect(invoke.mock.calls.some(([, args]) => args?.input?.path === "/observe/pause")).toBe(false);

    currentProcessState = managedState;
    view.rerender(<GatewayObservePage active lang="en" />);
    await waitFor(() => expect(FakeEventSource.instances.length).toBeGreaterThan(1));
    expect(screen.getByText("test-model")).toBeTruthy();
    view.unmount();
  });

  it("appends SSE rows without replacing existing rows", async () => {
    const first = { id: 1, channel: "one", status_code: 200, model: "first", ok: true };
    const second = { id: 2, channel: "two", status_code: 200, model: "second", ok: true };
    invoke.mockImplementation(async (command: string, args?: { input?: { path?: string } }) => {
      if (command === "get_gateway_process_state") return managedState;
      const path = args?.input?.path;
      if (path === "/observe/state") return { ...observeState, capture_enabled: true, retained_count: 2 };
      if (path === "/observe/requests") return { requests: [first] };
      return {};
    });
    render(<GatewayObservePage lang="en" />);
    expect(await screen.findByText("first")).toBeTruthy();
    const source = FakeEventSource.instances.at(-1)!;
    source.emit("request", second);
    expect(await screen.findByText("second")).toBeTruthy();
    expect(screen.getByText("first")).toBeTruthy();
  });

  it("copies the complete current detail view and supports find", async () => {
    const detail = { probes: { global_entry_probe: { raw_text: "POST /v1/responses\r\n\r\nhello hello", request_body_json: { messages: [{ role: "user", content: "hello" }] }, response_body_json: null } } };
    const row = { id: 1, channel: "synthetic", status_code: 200, model: "find-model", ok: true };
    const clipboard = { writeText: vi.fn(async () => undefined) };
    Object.assign(navigator, { clipboard });
    invoke.mockImplementation(async (command: string, args?: { input?: { path?: string } }) => {
      if (command === "get_gateway_process_state") return managedState;
      const path = args?.input?.path;
      if (path === "/observe/state") return { ...observeState, retained_count: 1 };
      if (path === "/observe/requests") return { requests: [row] };
      if (path === "/observe/request/1") return detail;
      return {};
    });
    render(<GatewayObservePage lang="en" />);
    fireEvent.click(await screen.findByText("find-model"));
    const find = await screen.findByRole("textbox", { name: "Find in details" });
    fireEvent.change(find, { target: { value: "hello" } });
    expect(screen.getByText("1/2")).toBeTruthy();
    fireEvent.click(screen.getByRole("button", { name: "Copy all content" }));
    await waitFor(() => expect(clipboard.writeText).toHaveBeenCalledWith(expect.stringContaining("hello hello")));
  });

  it("loads archived packet content in chunks on demand", async () => {
    const detail = {
      id: 1,
      archive_status: "archived",
      packet_sizes: { global_entry_probe: { total_bytes: 9, original_bytes: 9 } },
      probes: { global_entry_probe: { raw_text: "head", retained_bytes: 4, original_bytes: 9, raw_text_truncated: true } },
    };
    const row = { id: 1, channel: "synthetic", status_code: 200, model: "chunk-model", ok: true };
    invoke.mockImplementation(async (command: string, args?: { input?: { path?: string } }) => {
      if (command === "get_gateway_process_state") return managedState;
      const path = args?.input?.path;
      if (path === "/observe/state") return { ...observeState, retained_count: 1 };
      if (path === "/observe/requests") return { requests: [row] };
      if (path === "/observe/request/1") return detail;
      if (path?.startsWith("/observe/packet/1")) return { id: 1, probe: "global_entry_probe", offset: 4, length: 5, total_bytes: 9, original_bytes: 9, text: " tail", next_offset: 9, complete: true };
      return {};
    });
    render(<GatewayObservePage lang="en" />);
    fireEvent.click(await screen.findByText("chunk-model"));
    fireEvent.click(await screen.findByRole("button", { name: /Load more/ }));
    await waitFor(() => expect(screen.getByText("head tail")).toBeTruthy());
    expect(invoke).toHaveBeenCalledWith("gateway_request", expect.objectContaining({
      input: expect.objectContaining({ path: expect.stringContaining("/observe/packet/1?probe=global_entry_probe&offset=4") }),
    }));
  });
});
