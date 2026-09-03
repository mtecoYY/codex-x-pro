import React from "react";
import { ArrowDown, ArrowUp, CirclePause, CirclePlay, Eraser, Save } from "lucide-react";
import type { GatewayObserveRow, GatewayObserveState, Lang } from "../types";
import { Button, IconButton, ModalShell, StatusBadge } from "../components/ui";
import { gatewayProbeTruncation, gatewayProbeValue, type GatewayDetailView } from "../gatewayDetail";
import { gatewayControlsDisabled, gatewayDisplayMode } from "../gatewayState";
import { gatewayText, useGatewayPageState } from "../gatewayPageState";
import type { GatewayDetail, GatewayObserveListResponse, GatewayObserveSortKey } from "../gatewayPageTypes";
import "../styles/gateway-observe-page.css";

type Props = { lang: Lang; active?: boolean };
type Filter = "all" | "success" | "error";

const EMPTY_OBSERVE: GatewayObserveState = {
  capture_enabled: false,
  capture_limit: 100,
  retained_count: 0,
  evicted_count: 0,
  capture_dropped_count: 0,
  next_seq: 1,
};

export function GatewayObservePage({ lang, active = true }: Props) {
  const {
    port,
    processState,
    busy,
    error,
    setError,
    request,
    run,
  } = useGatewayPageState();
  const [observe, setObserve] = React.useState<GatewayObserveState>(EMPTY_OBSERVE);
  const [rows, setRows] = React.useState<GatewayObserveRow[]>([]);
  const [filter, setFilter] = React.useState<Filter>("all");
  const [sort, setSort] = React.useState<{ key: GatewayObserveSortKey; desc: boolean }>({ key: "id", desc: true });
  const [limitDraft, setLimitDraft] = React.useState("100");
  const [detail, setDetail] = React.useState<GatewayDetail | null>(null);
  const [detailProbe, setDetailProbe] = React.useState("global_entry_probe");
  const [detailView, setDetailView] = React.useState<GatewayDetailView>("raw-text");
  const lastSeqRef = React.useRef(0);

  const refresh = React.useCallback(async () => {
    const current = await request<GatewayObserveState>("GET", "/observe/state");
    const listed = await request<GatewayObserveListResponse>("GET", "/observe/requests");
    const nextRows = listed.requests || [];
    setObserve(current);
    setLimitDraft(String(current.capture_limit));
    setRows(nextRows);
    lastSeqRef.current = nextRows.reduce((max, row) => Math.max(max, row.id), lastSeqRef.current);
  }, [request]);

  React.useEffect(() => {
    if (!active) return;
    const refreshWhenNeeded = () => {
      if (gatewayDisplayMode(processState) !== "managed") {
        return;
      }
      void refresh().catch((nextError) => setError(String(nextError)));
    };
    refreshWhenNeeded();
    window.addEventListener("focus", refreshWhenNeeded);
    window.addEventListener("codexx-gateway-state-changed", refreshWhenNeeded);
    return () => {
      window.removeEventListener("focus", refreshWhenNeeded);
      window.removeEventListener("codexx-gateway-state-changed", refreshWhenNeeded);
    };
  }, [active, processState, refresh, setError]);

  React.useEffect(() => {
    if (!active || gatewayDisplayMode(processState) !== "managed") return;
    let disposed = false;
    let source: EventSource | null = null;
    let retryTimer: number | null = null;

    const loadWindow = async () => {
      try {
        const listed = await request<GatewayObserveListResponse>("GET", "/observe/requests");
        if (disposed) return;
        const nextRows = listed.requests || [];
        setRows(nextRows);
        lastSeqRef.current = nextRows.reduce((max, row) => Math.max(max, row.id), lastSeqRef.current);
      } catch {
        return;
      }
    };

    const connect = () => {
      if (disposed) return;
      source = new EventSource(`http://127.0.0.1:${port}/observe/events?after=${lastSeqRef.current}`);
      source.addEventListener("request", (event) => {
        try {
          const row = JSON.parse((event as MessageEvent).data) as GatewayObserveRow;
          lastSeqRef.current = Math.max(lastSeqRef.current, row.id);
          setRows((current) => [...current.filter((item) => item.id !== row.id), row].slice(-Math.max(1, observe.capture_limit)));
        } catch {
          return;
        }
      });
      source.addEventListener("observe_gap", () => void loadWindow());
      source.onerror = () => {
        source?.close();
        source = null;
        if (!disposed) retryTimer = window.setTimeout(connect, 1000);
      };
    };

    connect();
    return () => {
      disposed = true;
      if (retryTimer !== null) window.clearTimeout(retryTimer);
      source?.close();
    };
  }, [active, observe.capture_limit, port, processState, request]);

  const control = (path: string, action: string) => {
    void run(action, () => request("POST", path, {})).catch(() => undefined);
  };

  const saveLimit = () => {
    const value = Number(limitDraft);
    if (!Number.isInteger(value)) {
      setError(gatewayText(lang, "OBSERVE_CAPTURE_LIMIT_INVALID: 请输入整数", "OBSERVE_CAPTURE_LIMIT_INVALID: Enter an integer"));
      return;
    }
    void run("limit", () => request("PUT", "/observe/settings", { capture_limit: value })).catch(() => undefined);
  };

  const openDetail = async (id: number) => {
    try {
      const next = await request<GatewayDetail>("GET", `/observe/request/${id}`);
      setDetail(next);
      const probes = next.probes as Record<string, unknown> | undefined;
      setDetailProbe(probes && Object.keys(probes)[0] || "global_entry_probe");
      setDetailView("raw-text");
    } catch (nextError) {
      setError(String(nextError));
    }
  };

  const displayed = React.useMemo(() => {
    const filtered = rows.filter((row) => filter === "all" || (filter === "success" ? row.ok : !row.ok));
    return [...filtered].sort((a, b) => {
      const av = a[sort.key] ?? "";
      const bv = b[sort.key] ?? "";
      const result = String(av).localeCompare(String(bv), undefined, { numeric: true });
      return sort.desc ? -result : result;
    });
  }, [filter, rows, sort]);

  const changeSort = (key: GatewayObserveSortKey) => {
    setSort((current) => ({ key, desc: current.key === key ? !current.desc : true }));
  };
  const SortIcon = ({ column }: { column: GatewayObserveSortKey }) =>
    sort.key !== column ? null : sort.desc ? <ArrowDown size={13} aria-hidden="true" /> : <ArrowUp size={13} aria-hidden="true" />;
  const detailProbes = detail && detail.probes && typeof detail.probes === "object" ? detail.probes as Record<string, any> : null;
  const activeProbe = detailProbes?.[detailProbe] as Record<string, any> | undefined;
  const activeProbeValue = gatewayProbeValue(activeProbe, detailView);
  const activeProbeTruncation = gatewayProbeTruncation(activeProbe);
  const detailText = detailProbes
    ? activeProbeValue == null
      ? gatewayText(lang, "此视图不可用", "This view is unavailable")
      : typeof activeProbeValue === "string"
        ? activeProbeValue
        : JSON.stringify(activeProbeValue, null, 2)
    : JSON.stringify(detail, null, 2);
  const disabled = gatewayControlsDisabled(processState, Boolean(busy));
  const displayMode = gatewayDisplayMode(processState);
  const managedGateway = displayMode === "managed";
  const routeDisconnected = displayMode === "disconnected";

  return (
    <section className={`cx-utility cx-page cx-page--stacked cx-gateway-observe-page${active ? "" : " page-pane-hidden"}`}>
      <header className="cx-page-header">
        <div className="cx-page-header-copy">
          <div className="cx-page-eyebrow">Gateway</div>
          <h2>{gatewayText(lang, "实时请求观测", "Live request observation")}</h2>
          <p>{gatewayText(lang, "查看网关运行期间的受限请求观测记录。", "View bounded request observations captured while the gateway is running.")}</p>
        </div>
        <StatusBadge tone={managedGateway ? "success" : routeDisconnected ? "warning" : "neutral"}>
          {managedGateway
            ? gatewayText(lang, "网关已接入", "Gateway connected")
            : routeDisconnected
              ? gatewayText(lang, "网关未接入 Codex", "Gateway not connected to Codex")
              : gatewayText(lang, "需要网关模式", "Gateway mode required")}
        </StatusBadge>
      </header>

      {!managedGateway && (
        <div className="cx-gateway-disabled-note">
          {routeDisconnected
            ? gatewayText(lang, "网关进程仍在运行，但 Codex 当前配置未指向本地网关；为避免覆盖外部配置，实时请求观测暂不可用。", "The gateway process is still running, but Codex is not routed through it; live request observation is disabled to avoid overwriting external configuration.")
            : gatewayText(lang, "请先进入网关模式，实时请求观测才会启用。", "Start gateway mode first to enable live request observation.")}
        </div>
      )}
      {error && <div className="cx-gateway-error" role="alert">{error}</div>}

      <section className="cx-page-panel cx-gateway-observe-panel">
        <div className="cx-page-panel-header cx-gateway-observe-head">
          <div className="cx-page-panel-header__copy">
            <h3>{gatewayText(lang, "观测记录", "Observed requests")}</h3>
            <span className="cx-page-panel-header__meta">{observe.retained_count} / {observe.capture_limit} · {gatewayText(lang, "淘汰", "Evicted")} {observe.evicted_count} · {gatewayText(lang, "丢弃", "Dropped")} {observe.capture_dropped_count}</span>
          </div>
          <div className="cx-gateway-observe-actions">
            <Button
              variant="secondary"
              size="sm"
              icon={observe.capture_enabled ? <CirclePause size={16} /> : <CirclePlay size={16} />}
              onClick={() => control(observe.capture_enabled ? "/observe/pause" : "/observe/start", "capture")}
              disabled={disabled}
            >
              {observe.capture_enabled ? gatewayText(lang, "暂停", "Pause") : gatewayText(lang, "启动采集", "Start capture")}
            </Button>
            <Button variant="secondary" size="sm" icon={<Eraser size={16} />} onClick={() => control("/observe/clear", "clear")} disabled={disabled}>
              {gatewayText(lang, "清除", "Clear")}
            </Button>
            <label className="ui-field ui-field--compact">
              <span className="ui-field__label">{gatewayText(lang, "筛选", "Filter")}</span>
              <select className="ui-field__control" value={filter} onChange={(event) => setFilter(event.target.value as Filter)} disabled={disabled}>
                <option value="all">{gatewayText(lang, "全部", "All")}</option>
                <option value="success">{gatewayText(lang, "成功", "Success")}</option>
                <option value="error">{gatewayText(lang, "错误", "Error")}</option>
              </select>
            </label>
            <label className="ui-field ui-field--compact cx-gateway-limit">
              <span className="ui-field__label">{gatewayText(lang, "保留上限", "Retention limit")}</span>
              <span className="cx-gateway-limit__control">
                <input className="ui-field__control" value={limitDraft} onChange={(event) => setLimitDraft(event.target.value)} disabled={disabled} inputMode="numeric" />
                <IconButton icon={<Save size={16} />} label={gatewayText(lang, "保存保留上限", "Save retention limit")} variant="neutral" size="sm" onClick={saveLimit} disabled={disabled} />
              </span>
            </label>
          </div>
        </div>

        <div className="cx-gateway-table-wrap">
          <table className="cx-gateway-table">
            <thead>
              <tr>
                {([
                  ["id", "ID"],
                  ["channel", gatewayText(lang, "渠道", "Channel")],
                  ["status_code", gatewayText(lang, "状态码", "Status")],
                  ["model", gatewayText(lang, "模型", "Model")],
                  ["request_time_ms", gatewayText(lang, "耗时(ms)", "Time(ms)")],
                  ["first_token_ms", gatewayText(lang, "首字(ms)", "First token(ms)")],
                  ["tokens", "Tokens"],
                ] as [GatewayObserveSortKey, string][]).map(([key, label]) => (
                  <th key={key}>
                    <Button variant="ghost" size="sm" onClick={() => changeSort(key)} disabled={disabled}>
                      {label}
                      <SortIcon column={key} />
                    </Button>
                  </th>
                ))}
              </tr>
            </thead>
            <tbody>
              {displayed.length === 0 ? (
                <tr>
                  <td colSpan={7} className="cx-gateway-empty">
                    {disabled ? gatewayText(lang, "请先进入网关模式", "Start gateway mode first") : gatewayText(lang, "暂无观测记录", "No observed requests")}
                  </td>
                </tr>
              ) : displayed.map((row) => (
                <tr
                  key={row.id}
                  className={row.ok ? "" : "cx-gateway-row--error"}
                  onClick={() => void openDetail(row.id)}
                  onKeyDown={(event) => {
                    if (event.key === "Enter" || event.key === " ") void openDetail(row.id);
                  }}
                  tabIndex={0}
                >
                  <td>{row.id}</td>
                  <td>{row.channel || "-"}</td>
                  <td>{row.status_code || row.error || "-"}</td>
                  <td>{row.model || "-"}</td>
                  <td>{row.request_time_ms ?? "-"}</td>
                  <td>{row.first_token_ms ?? "-"}</td>
                  <td title={row.tokens_error || undefined}>{row.tokens ?? gatewayText(lang, "不可用", "Unavailable")}</td>
                </tr>
              ))}
            </tbody>
          </table>
        </div>
      </section>

      <ModalShell
        open={Boolean(detail)}
        onClose={() => setDetail(null)}
        title={gatewayText(lang, "请求详情", "Request details")}
        closeLabel={gatewayText(lang, "关闭", "Close")}
        size="xl"
        bodyClassName="cx-gateway-detail-body"
      >
        {detailProbes && (
          <div className="cx-gateway-detail-controls">
            <label className="ui-field ui-field--compact">
              <span className="ui-field__label">{gatewayText(lang, "探针", "Probe")}</span>
              <select className="ui-field__control" value={detailProbe} onChange={(event) => setDetailProbe(event.target.value)}>
                {Object.keys(detailProbes).map((key) => <option key={key} value={key}>{key}</option>)}
              </select>
            </label>
            <label className="ui-field ui-field--compact">
              <span className="ui-field__label">{gatewayText(lang, "视图", "View")}</span>
              <select className="ui-field__control" value={detailView} onChange={(event) => setDetailView(event.target.value as GatewayDetailView)}>
                <option value="raw-text">raw-text</option>
                <option value="request body JSON">request body JSON</option>
                <option value="response body JSON">response body JSON</option>
              </select>
            </label>
          </div>
        )}
        {activeProbeTruncation && <div className="cx-gateway-truncation">{activeProbeTruncation}</div>}
        <pre className="cx-gateway-detail-pre">{detailText}</pre>
      </ModalShell>
    </section>
  );
}
