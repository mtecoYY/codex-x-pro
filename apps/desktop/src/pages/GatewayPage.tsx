import React from "react";
import { Power, RefreshCw } from "lucide-react";
import type { Lang } from "../types";
import { Button, StatusBadge } from "../components/ui";
import { gatewayCommands } from "../gatewayCommands";
import { gatewayCanStart, gatewayDisplayMode } from "../gatewayState";
import { gatewayText, GATEWAY_PORT_KEY, useGatewayPageState } from "../gatewayPageState";
import "../styles/gateway-page.css";

type Props = { lang: Lang; configDir?: string; active?: boolean };

function runtimeVersion(value: unknown) {
  if (!value || typeof value !== "object") return null;
  const version = (value as Record<string, unknown>).version;
  return typeof version === "number" || typeof version === "string" ? String(version) : null;
}

export function GatewayPage({ lang, configDir = "", active = true }: Props) {
  const {
    port,
    setPort,
    processState,
    busy,
    error,
    setError,
    refreshProcess,
    run,
  } = useGatewayPageState();
  const [portDraft, setPortDraft] = React.useState(String(port));
  const [upstream, setUpstream] = React.useState("https://newapi.gogogogoapp.mom");

  const refresh = React.useCallback(async () => {
    const state = await refreshProcess();
    const providerState = state.state?.provider;
    const persistedUpstream = providerState && typeof providerState === "object" && typeof (providerState as Record<string, unknown>).base_url === "string"
      ? (providerState as Record<string, unknown>).base_url
      : state.state?.upstream;
    if (typeof persistedUpstream === "string" && persistedUpstream.trim()) setUpstream(persistedUpstream);
    return state;
  }, [refreshProcess]);

  const start = () => {
    const nextPort = Number(portDraft);
    if (!Number.isInteger(nextPort) || nextPort < 1 || nextPort > 65535) {
      setError(gatewayText(lang, "GATEWAY_INVALID_LISTEN: 端口必须是 1-65535 的整数", "GATEWAY_INVALID_LISTEN: Listen port must be an integer from 1 to 65535"));
      return;
    }

    localStorage.setItem(GATEWAY_PORT_KEY, String(nextPort));
    setPort(nextPort);
    setPortDraft(String(nextPort));
    window.dispatchEvent(new CustomEvent("codexx-gateway-port-changed", { detail: nextPort }));
    void run(
      "start",
      () => gatewayCommands.start({
        listenHost: "127.0.0.1",
        listenPort: nextPort,
        upstream,
        configDir: configDir || null,
      }),
      nextPort,
    ).catch(() => undefined);
  };

  const stop = () => void run("stop", () => gatewayCommands.stop()).catch(() => undefined);
  const refreshPage = () => void refresh().catch((nextError) => setError(String(nextError)));
  const displayMode = gatewayDisplayMode(processState);
  const externalGateway = displayMode === "external";
  const managedGateway = displayMode === "managed" || displayMode === "disconnected";
  const routeDisconnected = displayMode === "disconnected";
  const stateUnknown = displayMode === "unknown";
  const degradedGateway = displayMode === "degraded";
  const runtimeState = processState?.state;
  const providerVersion = runtimeVersion(runtimeState?.provider);
  const instructionVersion = runtimeVersion(runtimeState?.instruction);
  const runtimeSubmission = !processState?.running
    ? gatewayText(lang, "未提交", "Not submitted")
    : providerVersion || instructionVersion
      ? `${gatewayText(lang, "Provider", "Provider")} ${providerVersion ?? "-"} · ${gatewayText(lang, "提示词", "Instructions")} ${instructionVersion ?? "-"}`
      : gatewayText(lang, "已读取运行时状态", "Runtime state loaded");
  const listenStatus = processState?.running
    ? `127.0.0.1:${processState.listenPort} · ${gatewayText(lang, "已监听", "Listening")}`
    : `127.0.0.1:${port} · ${gatewayText(lang, "未监听", "Not listening")}`;
  const processError = processState?.degraded && processState.error !== error ? processState.error : "";

  return (
    <section className={`cx-utility cx-page cx-page--stacked cx-gateway-page${active ? "" : " page-pane-hidden"}`}>
      <header className="cx-page-header cx-gateway-header">
        <div className="cx-page-header-copy">
          <div className="cx-page-eyebrow">Gateway</div>
          <h2>{gatewayText(lang, "本地网关", "Local gateway")}</h2>
          <p>{gatewayText(lang, "控制本机转发和网关运行状态。", "Control local forwarding and gateway runtime state.")}</p>
        </div>
        <StatusBadge tone={externalGateway || routeDisconnected || degradedGateway ? "warning" : managedGateway ? "success" : "neutral"}>
          {externalGateway
            ? gatewayText(lang, "外部网关运行中", "External gateway running")
            : routeDisconnected
              ? gatewayText(lang, "网关运行中但未接入 Codex", "Gateway running but not connected to Codex")
            : managedGateway
              ? gatewayText(lang, "已运行", "Running")
              : degradedGateway
                ? gatewayText(lang, "网关需要修复", "Gateway recovery required")
                : stateUnknown
                  ? gatewayText(lang, "正在确认状态", "Checking status")
                  : gatewayText(lang, "未启动", "Stopped")}
        </StatusBadge>
      </header>

      <section className="cx-page-panel cx-gateway-control-panel">
        <div className="cx-gateway-toolbar">
          <label className="ui-field">
            <span className="ui-field__label">{gatewayText(lang, "监听端口", "Listen port")}</span>
            <input
              className="ui-field__control"
              value={portDraft}
              onChange={(event) => setPortDraft(event.target.value)}
              disabled={managedGateway || Boolean(busy)}
              inputMode="numeric"
            />
          </label>
          <label className="ui-field cx-gateway-upstream">
            <span className="ui-field__label">{gatewayText(lang, "上游地址", "Upstream")}</span>
            <input
              className="ui-field__control"
              value={upstream}
              onChange={(event) => setUpstream(event.target.value)}
              disabled={managedGateway || Boolean(busy)}
            />
          </label>
          {stateUnknown || degradedGateway ? (
            <Button variant="secondary" icon={<RefreshCw size={16} />} onClick={refreshPage} disabled={Boolean(busy)}>
              Check status
            </Button>
          ) : gatewayCanStart(processState) ? (
            <Button variant="primary" icon={<Power size={16} />} onClick={start} disabled={Boolean(busy)}>
              {busy === "start" ? gatewayText(lang, "正在启动", "Starting") : gatewayText(lang, "启动网关", "Start gateway")}
            </Button>
          ) : (
            <Button variant="danger" icon={<Power size={16} />} onClick={stop} disabled={Boolean(busy)}>
              {busy === "stop" ? gatewayText(lang, "正在停止", "Stopping") : gatewayText(lang, "停止网关", "Stop gateway")}
            </Button>
          )}
          <Button variant="secondary" icon={<RefreshCw size={16} />} onClick={refreshPage} disabled={Boolean(busy)}>
            {gatewayText(lang, "刷新", "Refresh")}
          </Button>
        </div>

        <div className="cx-gateway-runtime-status">
          <div className="cx-gateway-runtime-grid">
            <span>{gatewayText(lang, "监听", "Listen")}</span>
            <strong>{listenStatus}</strong>
            <span>{gatewayText(lang, "运行时提交", "Runtime submission")}</span>
            <strong>{runtimeSubmission}</strong>
            <span>{gatewayText(lang, "网关守护", "Gateway protection")}</span>
            <strong>
              {processState?.watchdogDesired ? gatewayText(lang, "已启用", "Enabled") : gatewayText(lang, "未启用", "Disabled")} ·{" "}
              {processState?.watchdogRuntime === "running"
                ? gatewayText(lang, "运行中", "Running")
                : processState?.watchdogRuntime === "starting"
                  ? gatewayText(lang, "启动中", "Starting")
                  : gatewayText(lang, "未运行", "Stopped")}
            </strong>
            <span>{gatewayText(lang, "登录自启动", "Login autostart")}</span>
            <strong>{processState?.watchdogAutostart ? gatewayText(lang, "已启用", "Enabled") : gatewayText(lang, "未启用", "Disabled")}</strong>
          </div>
          <p>
            {externalGateway
              ? gatewayText(lang, "Codex-X 未进入网关模式；外部网关不受 Codex-X 管理。", "Codex-X is not in gateway mode; the external gateway is not managed by Codex-X.")
              : routeDisconnected
                ? gatewayText(lang, "网关进程和看门狗仍在运行，但 Codex 当前配置未指向本地网关；未覆盖外部配置。", "The gateway process and watchdog are running, but Codex is not routed through the local gateway; the external configuration was not overwritten.")
                : gatewayText(lang, "网关守护和运行时提交状态来自实际网关状态。", "Protection and runtime submission status come from the actual gateway state.")}
          </p>
        </div>
      </section>

      <div className="cx-gateway-restart-warning">
        {gatewayText(lang, "网关配置改动将在重启 Codex 后生效。", "Gateway configuration changes take effect after restarting Codex.")}
      </div>
      {!managedGateway && (
        <div className="cx-gateway-disabled-note">
          {externalGateway
            ? gatewayText(lang, "检测到外部网关运行中，Codex-X 未接管。请修改监听端口后启动 Codex-X 网关；保持当前端口启动会返回端口冲突。", "An external gateway is running and is not managed by Codex-X. Change the listen port before starting the Codex-X gateway; using the current port will report a port conflict.")
            : gatewayText(lang, "实时请求观测和用户脚本处理器需要先进入网关模式。", "Live observation and user script processors require gateway mode.")}
        </div>
      )}
      {routeDisconnected && (
        <div className="cx-gateway-disabled-note">
          {gatewayText(lang, "网关进程仍由 Codex-X 管理，但当前 Codex 配置已被外部修改。为避免覆盖该配置，实时观测、用户脚本和网关热更新暂不可用。", "The gateway process is still managed by Codex-X, but the current Codex configuration was changed externally. Live observation, user scripts, and gateway hot updates are disabled to avoid overwriting that configuration.")}
        </div>
      )}
      {(error || processError) && <div className="cx-gateway-error" role="alert">{error || processError}</div>}
    </section>
  );
}
