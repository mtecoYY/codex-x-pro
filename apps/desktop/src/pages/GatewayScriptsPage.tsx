import React from "react";
import { RefreshCw } from "lucide-react";
import type { Lang } from "../types";
import { Button, ModalShell, StatusBadge } from "../components/ui";
import { gatewayControlsDisabled, gatewayDisplayMode } from "../gatewayState";
import { gatewayText, useGatewayPageState } from "../gatewayPageState";
import {
  DEFAULT_SCRIPT_TEST_RAW_TEXT,
  type GatewayDetail,
  type GatewayScriptListResponse,
  type GatewayScriptSummary,
} from "../gatewayPageTypes";
import "../styles/gateway-scripts-page.css";

type Props = { lang: Lang; active?: boolean };
type TestSource = "default" | "custom";

function scriptStatusTone(status: GatewayScriptSummary["status"]) {
  if (status === "passed") return "success" as const;
  if (status === "failed") return "danger" as const;
  if (status === "testing") return "warning" as const;
  return "neutral" as const;
}

export function GatewayScriptsPage({ lang, active = true }: Props) {
  const {
    processState,
    busy,
    error,
    setError,
    request,
    run,
  } = useGatewayPageState();
  const [scripts, setScripts] = React.useState<GatewayScriptSummary[]>([]);
  const [detail, setDetail] = React.useState<GatewayDetail | null>(null);
  const [protocolOpen, setProtocolOpen] = React.useState(false);
  const [testScriptId, setTestScriptId] = React.useState<string | null>(null);
  const [testSource, setTestSource] = React.useState<TestSource>("default");
  const [testRawText, setTestRawText] = React.useState(DEFAULT_SCRIPT_TEST_RAW_TEXT);

  const loadScripts = React.useCallback(async () => {
    if (!processState?.running) return;
    const result = await request<GatewayScriptListResponse>("GET", "/scripts");
    setScripts(result.scripts || []);
  }, [processState, request]);

  React.useEffect(() => {
    if (!active) return;
    void loadScripts().catch((nextError) => setError(String(nextError)));
    window.addEventListener("codexx-gateway-state-changed", loadScripts);
    return () => window.removeEventListener("codexx-gateway-state-changed", loadScripts);
  }, [active, loadScripts, setError]);

  const refreshScripts = () => {
    void run("scripts:refresh", async () => {
      await request("POST", "/scripts/refresh", {});
      await loadScripts();
    }).catch(() => undefined);
  };

  const scriptAction = (scriptId: string, action: "test" | "enable" | "disable", rawText?: string) => {
    void run(`script:${scriptId}:${action}`, async () => {
      const body = action === "test" && rawText !== undefined
        ? { source: "custom", raw_text: rawText }
        : {};
      const result = await request<GatewayDetail>("POST", `/scripts/${scriptId}/${action}`, body);
      if (action === "test") setDetail(result);
      await loadScripts();
    }).catch(() => undefined);
  };

  const updatePriority = (scriptId: string, priority: number) => {
    void run(`script:${scriptId}:priority`, async () => {
      await request("PUT", `/scripts/${scriptId}/priority`, { priority });
      await loadScripts();
    }).catch(() => undefined);
  };

  const openScriptDetail = async (scriptId: string) => {
    try {
      setDetail(await request<GatewayDetail>("GET", `/scripts/${scriptId}/test-detail`));
    } catch (nextError) {
      setError(String(nextError));
    }
  };

  const displayMode = gatewayDisplayMode(processState);
  const managedGateway = displayMode === "managed";
  const routeDisconnected = displayMode === "disconnected";
  const scriptApiDisabled = !processState?.running || Boolean(busy);
  const chainControlDisabled = gatewayControlsDisabled(processState, Boolean(busy));

  return (
    <section className={`cx-utility cx-page cx-page--stacked cx-gateway-scripts-page${active ? "" : " page-pane-hidden"}`}>
      <header className="cx-page-header">
        <div className="cx-page-header-copy">
          <div className="cx-page-eyebrow">Gateway</div>
          <h2>{gatewayText(lang, "用户脚本处理器", "User script processors")}</h2>
          <p>{gatewayText(lang, "发现、测试和管理本机用户脚本处理器。", "Discover, test, and manage local user script processors.")}</p>
        </div>
        <StatusBadge tone={managedGateway ? "success" : "warning"}>
          {managedGateway
            ? gatewayText(lang, "网关模式", "Gateway mode")
            : routeDisconnected
              ? gatewayText(lang, "网关未接入 Codex", "Gateway not connected to Codex")
              : gatewayText(lang, "直连模式", "Direct mode")}
        </StatusBadge>
      </header>

      <div className={`cx-gateway-script-notice${managedGateway ? " cx-gateway-script-notice--active" : ""}`}>
        {routeDisconnected
          ? gatewayText(lang, "网关进程仍在运行，但 Codex 当前配置未指向本地网关；用户脚本不会处理 Codex 的真实请求。", "The gateway process is still running, but Codex is not routed through it; user scripts do not process real Codex requests.")
          : gatewayText(lang, "用户脚本仅在网关模式下生效。当前为直连模式时，脚本不会处理 Codex 的真实请求。", "User scripts are active only in gateway mode. In direct mode, scripts do not process real Codex requests.")}
      </div>
      {error && <div className="cx-gateway-error" role="alert">{error}</div>}

      <section className="cx-page-panel cx-gateway-scripts-panel">
        <div className="cx-page-panel-header cx-gateway-scripts-head">
          <div className="cx-page-panel-header__copy">
            <h3>{gatewayText(lang, "脚本列表", "Script list")}</h3>
            <p>{gatewayText(lang, `已发现 ${scripts.length} 个脚本。用户脚本使用 raw-text 处理真实请求；测试只校验文本协议和结构。`, `${scripts.length} scripts found. User scripts use raw-text for live requests; tests validate text protocol and structure only.`)}</p>
          </div>
          <div className="cx-gateway-script-toolbar">
            <Button variant="secondary" size="sm" onClick={() => setProtocolOpen(true)}>
              {gatewayText(lang, "协议说明", "Protocol documentation")}
            </Button>
            <Button variant="secondary" size="sm" icon={<RefreshCw size={16} />} onClick={refreshScripts} disabled={scriptApiDisabled}>
              {gatewayText(lang, "刷新脚本", "Refresh scripts")}
            </Button>
          </div>
        </div>

        <div className="cx-gateway-scripts">
          {scripts.length === 0 ? (
            <div className="cx-gateway-disabled-note">
              {processState?.running
                ? gatewayText(lang, "未发现脚本。将 manifest.json 和入口文件放入 ~/.codex-x/gateway-tools/<script-id>/。", "No scripts found. Place manifest.json and an entry file in ~/.codex-x/gateway-tools/<script-id>/.")
                : gatewayText(lang, "当前网关不可访问，脚本列表将在网关恢复后重新读取。", "The gateway is unavailable. The script list will be loaded again when the gateway is available.")}
            </div>
          ) : scripts.map((script) => (
            <article key={script.id} className="cx-gateway-script">
              <div className="cx-gateway-script-main">
                <div className="cx-gateway-script-title">
                  <strong>{script.name || script.id}</strong>
                  <StatusBadge tone={scriptStatusTone(script.status)} dot={false}>{script.status}</StatusBadge>
                  {script.enabled && <StatusBadge tone="accent" dot={false}>{gatewayText(lang, "已启用", "Enabled")}</StatusBadge>}
                </div>
                <p>{script.error || script.description || script.id}</p>
                <small>
                  {gatewayText(lang, "真实调用近 10 次平均耗时", "Average of last 10 live calls")}:{" "}
                  {script.average_ms == null ? "-" : `${script.average_ms} ms`} ({script.sample_count ?? 0}/10)
                </small>
              </div>
              <div className="cx-gateway-script-actions">
                <label className="ui-field ui-field--compact">
                  <span className="ui-field__label">{gatewayText(lang, "优先级", "Priority")}</span>
                  <input
                    className="ui-field__control"
                    type="number"
                    defaultValue={script.priority}
                    disabled={chainControlDisabled || script.enabled}
                    onBlur={(event) => {
                      const value = Number(event.target.value);
                      if (Number.isInteger(value) && value !== script.priority) void updatePriority(script.id, value);
                    }}
                  />
                </label>
                <Button
                  variant="secondary"
                  size="sm"
                  onClick={() => {
                    setTestScriptId(script.id);
                    setTestSource("default");
                    setTestRawText(DEFAULT_SCRIPT_TEST_RAW_TEXT);
                  }}
                  disabled={scriptApiDisabled || Boolean(script.error)}
                >
                  {gatewayText(lang, script.status === "failed" ? "重试" : "测试", script.status === "failed" ? "Retry" : "Test")}
                </Button>
                {script.test_detail_available && (
                  <Button variant="secondary" size="sm" onClick={() => void openScriptDetail(script.id)} disabled={scriptApiDisabled}>
                    {gatewayText(lang, "测试详情", "Test detail")}
                  </Button>
                )}
                <Button
                  variant={script.enabled ? "danger" : "primary"}
                  size="sm"
                  onClick={() => scriptAction(script.id, script.enabled ? "disable" : "enable")}
                  disabled={chainControlDisabled || Boolean(script.error) || (!script.enabled && script.status !== "passed")}
                >
                  {script.enabled ? gatewayText(lang, "禁用", "Disable") : gatewayText(lang, "启用", "Enable")}
                </Button>
              </div>
            </article>
          ))}
        </div>
      </section>

      <ModalShell
        open={protocolOpen}
        onClose={() => setProtocolOpen(false)}
        title={gatewayText(lang, "用户脚本 raw-text 协议", "User script raw-text protocol")}
        closeLabel={gatewayText(lang, "关闭", "Close")}
        size="md"
        bodyClassName="cx-gateway-code-body"
      >
        <pre className="cx-gateway-code-pre">{gatewayText(lang, "stdin：完整 HTTP 请求 raw-text\n退出码 0：stdout 转发请求\n退出码 10：stdout 直接响应\n退出码 11：丢弃\n其他非零退出码：执行错误", "stdin: complete HTTP request raw-text\nexit 0: forward stdout request\nexit 10: direct stdout response\nexit 11: drop\nother nonzero exit: execution error")}</pre>
      </ModalShell>

      <ModalShell
        open={Boolean(testScriptId)}
        onClose={() => setTestScriptId(null)}
        title={`${gatewayText(lang, "脚本测试 raw-text", "Script test raw-text")}: ${testScriptId || ""}`}
        closeLabel={gatewayText(lang, "关闭", "Close")}
        size="lg"
        bodyClassName="cx-gateway-test-body"
        footer={(
          <>
            <Button variant="secondary" onClick={() => {
              setTestSource("default");
              setTestRawText(DEFAULT_SCRIPT_TEST_RAW_TEXT);
            }}>
              {gatewayText(lang, "恢复默认", "Restore default")}
            </Button>
            <Button
              variant="primary"
              disabled={scriptApiDisabled}
              onClick={() => {
                if (!testScriptId) return;
                const id = testScriptId;
                setTestScriptId(null);
                scriptAction(id, "test", testSource === "custom" ? testRawText : undefined);
              }}
            >
              {gatewayText(lang, "运行测试", "Run test")}
            </Button>
          </>
        )}
      >
        <div className="cx-gateway-test-source" role="tablist" aria-label={gatewayText(lang, "测试包来源", "Test packet source")}>
          <Button
            variant={testSource === "default" ? "primary" : "secondary"}
            size="sm"
            role="tab"
            aria-selected={testSource === "default"}
            onClick={() => setTestSource("default")}
          >
            {gatewayText(lang, "默认测试包", "Default test packet")}
          </Button>
          <Button
            variant={testSource === "custom" ? "primary" : "secondary"}
            size="sm"
            role="tab"
            aria-selected={testSource === "custom"}
            onClick={() => setTestSource("custom")}
          >
            {gatewayText(lang, "自定义测试包", "Custom test packet")}
          </Button>
        </div>
        <div className="cx-gateway-test-source-label">
          {gatewayText(lang, "本次测试来源", "Test source")}:{" "}
          {testSource === "default"
            ? gatewayText(lang, "默认", "Default")
            : gatewayText(lang, "自定义", "Custom")}
        </div>
        {testSource === "default" ? (
          <pre className="cx-gateway-code-pre cx-gateway-test-default" aria-label={gatewayText(lang, "默认测试包内容", "Default test packet content")}>
            {testRawText}
          </pre>
        ) : (
          <textarea
            className="ui-field__control cx-gateway-test-input"
            value={testRawText}
            onChange={(event) => setTestRawText(event.target.value)}
            spellCheck={false}
            aria-label={gatewayText(lang, "自定义 raw-text 测试包", "Custom raw-text test packet")}
          />
        )}
      </ModalShell>

      <ModalShell
        open={Boolean(detail)}
        onClose={() => setDetail(null)}
        title={gatewayText(lang, "测试详情", "Test details")}
        closeLabel={gatewayText(lang, "关闭", "Close")}
        size="xl"
        bodyClassName="cx-gateway-code-body"
      >
        <pre className="cx-gateway-code-pre">{JSON.stringify(detail, null, 2)}</pre>
      </ModalShell>
    </section>
  );
}
