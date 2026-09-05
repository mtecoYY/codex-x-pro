import React from "react";
import { ArrowDown, ArrowUp, ChevronDown, ChevronUp, CirclePause, CirclePlay, Clipboard, Eraser, Search, Save, X } from "lucide-react";
import type { GatewayObserveRow, GatewayObserveState, GatewayPacketResponse, Lang } from "../types";
import { Button, IconButton, ModalShell, StatusBadge } from "../components/ui";
import { conversationText, gatewayProbeTruncation, gatewayProbeValue, type GatewayDetailView } from "../gatewayDetail";
import { gatewayControlsDisabled, gatewayDisplayMode } from "../gatewayState";
import { gatewayText, useGatewayPageState } from "../gatewayPageState";
import type { GatewayDetail, GatewayObserveListResponse, GatewayObserveSortKey } from "../gatewayPageTypes";
import "../styles/gateway-observe-page.css";

type Props = { lang: Lang; active?: boolean };
type Filter = "all" | "success" | "error";

const EMPTY_OBSERVE: GatewayObserveState = {
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

const PACKET_CHUNK_BYTES = 256 * 1024;

function formatBytes(value: number) {
  if (value >= 1024 * 1024 * 1024) return `${(value / (1024 * 1024 * 1024)).toFixed(2)} GiB`;
  if (value < 1024 * 1024) return `${value} B`;
  return `${(value / (1024 * 1024)).toFixed(0)} MiB`;
}

function mergeRows(current: GatewayObserveRow[], incoming: GatewayObserveRow[], limit: number) {
  if (!incoming.length) return current;
  const byId = new Map(current.map((row) => [row.id, row]));
  incoming.forEach((row) => byId.set(row.id, row));
  return [...byId.values()].sort((a, b) => a.id - b.id).slice(-Math.max(1, limit));
}

function matchRanges(text: string, query: string) {
  if (!query) return [] as { start: number; end: number }[];
  const ranges: { start: number; end: number }[] = [];
  const lowerText = text.toLocaleLowerCase();
  const lowerQuery = query.toLocaleLowerCase();
  let offset = 0;
  while (offset < lowerText.length) {
    const start = lowerText.indexOf(lowerQuery, offset);
    if (start < 0) break;
    ranges.push({ start, end: start + query.length });
    offset = start + Math.max(1, query.length);
  }
  return ranges;
}

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
  const [totalDraft, setTotalDraft] = React.useState("2048");
  const [recordDraft, setRecordDraft] = React.useState("1024");
  const [detail, setDetail] = React.useState<GatewayDetail | null>(null);
  const [detailProbe, setDetailProbe] = React.useState("global_entry_probe");
  const [detailView, setDetailView] = React.useState<GatewayDetailView>("raw-text");
  const [detailQuery, setDetailQuery] = React.useState("");
  const [detailMatchIndex, setDetailMatchIndex] = React.useState(0);
  const [loadedPacketText, setLoadedPacketText] = React.useState<Record<string, string>>({});
  const [packetOffsets, setPacketOffsets] = React.useState<Record<string, number>>({});
  const [packetComplete, setPacketComplete] = React.useState<Record<string, boolean>>({});
  const [packetLoading, setPacketLoading] = React.useState(false);
  const detailSearchRef = React.useRef<HTMLInputElement>(null);
  const detailPreRef = React.useRef<HTMLPreElement>(null);
  const activeDetailMatchRef = React.useRef<HTMLElement | null>(null);
  const captureLimitRef = React.useRef(EMPTY_OBSERVE.capture_limit);
  const lastSeqRef = React.useRef(0);

  const refresh = React.useCallback(async ({ initial = false } = {}) => {
    const current = await request<GatewayObserveState>("GET", "/observe/state");
    let listed = await request<GatewayObserveListResponse>("GET", initial ? "/observe/requests" : `/observe/requests?after=${lastSeqRef.current}`);
    if (!initial && listed.history_gap) {
      listed = await request<GatewayObserveListResponse>("GET", "/observe/requests");
      initial = true;
    }
    const nextRows = listed.requests || [];
    setObserve(current);
    captureLimitRef.current = current.capture_limit;
    if (initial) {
      setLimitDraft(String(current.capture_limit));
      setTotalDraft(String(Math.round(current.capture_total_bytes / (1024 * 1024))));
      setRecordDraft(String(Math.round(current.capture_record_max_bytes / (1024 * 1024))));
      setRows(nextRows);
    } else if (nextRows.length) {
      setRows((existing) => mergeRows(existing, nextRows, current.capture_limit));
    }
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
    if (gatewayDisplayMode(processState) === "managed") {
      void refresh({ initial: true }).catch((nextError) => setError(String(nextError)));
    }
    window.addEventListener("focus", refreshWhenNeeded);
    window.addEventListener("codexx-gateway-state-changed", refreshWhenNeeded);
    return () => {
      window.removeEventListener("focus", refreshWhenNeeded);
      window.removeEventListener("codexx-gateway-state-changed", refreshWhenNeeded);
    };
  }, [active, processState, refresh, setError]);

  React.useEffect(() => {
    if (!active || gatewayDisplayMode(processState) !== "managed") return;
    const timer = window.setInterval(() => {
      void refresh().catch(() => undefined);
    }, 1000);
    return () => window.clearInterval(timer);
  }, [active, processState, refresh]);

  React.useEffect(() => {
    if (!active || gatewayDisplayMode(processState) !== "managed") return;
    let disposed = false;
    let source: EventSource | null = null;
    let retryTimer: number | null = null;

    const loadWindow = async () => {
      try {
        let listed = await request<GatewayObserveListResponse>("GET", `/observe/requests?after=${lastSeqRef.current}`);
        if (disposed) return;
        if (listed.history_gap) listed = await request<GatewayObserveListResponse>("GET", "/observe/requests");
        const nextRows = listed.requests || [];
        if (listed.history_gap || nextRows.length) setRows((current) => listed.history_gap ? nextRows : mergeRows(current, nextRows, captureLimitRef.current));
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
          setRows((current) => mergeRows(current, [row], captureLimitRef.current));
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
  }, [active, port, processState, request]);

  const control = (path: string, action: string) => {
    void run(action, () => request("POST", path, {}))
      .then(() => refresh({ initial: true }))
      .catch(() => undefined);
  };

  const saveLimit = () => {
    const value = Number(limitDraft);
    if (!Number.isInteger(value)) {
      setError(gatewayText(lang, "OBSERVE_CAPTURE_LIMIT_INVALID: 请输入整数", "OBSERVE_CAPTURE_LIMIT_INVALID: Enter an integer"));
      return;
    }
    void run("limit", () => request("PUT", "/observe/settings", { capture_limit: value }))
      .then(() => refresh({ initial: true }))
      .catch(() => undefined);
  };

  const saveArchiveSettings = () => {
    const totalMiB = Number(totalDraft);
    const recordMiB = Number(recordDraft);
    if (!Number.isInteger(totalMiB) || totalMiB < 64 || !Number.isInteger(recordMiB) || recordMiB < 1 || recordMiB > totalMiB) {
      setError(gatewayText(lang, "OBSERVE_ARCHIVE_SETTINGS_INVALID: 鐩存€诲拰鍗曟潯澶у皬蹇呴』鏄湁鏁堢殑 MiB 鏁板€硷紒", "OBSERVE_ARCHIVE_SETTINGS_INVALID: Enter valid MiB values; record size must not exceed total size"));
      return;
    }
    void run("archive-settings", () => request("PUT", "/observe/settings", {
      capture_total_bytes: totalMiB * 1024 * 1024,
      capture_record_max_bytes: recordMiB * 1024 * 1024,
    }))
      .then(() => refresh({ initial: true }))
      .catch(() => undefined);
  };

  const openDetail = async (id: number) => {
    try {
      const next = await request<GatewayDetail>("GET", `/observe/request/${id}`);
      setDetail(next);
      const probes = next.probes as Record<string, unknown> | undefined;
      setDetailProbe(probes && Object.keys(probes)[0] || "global_entry_probe");
      setDetailView("raw-text");
      setDetailQuery("");
      setDetailMatchIndex(0);
      setLoadedPacketText({});
      setPacketOffsets({});
      setPacketComplete({});
    } catch (nextError) {
      setError(String(nextError));
    }
  };

  const loadMorePacket = async () => {
    if (!detail || !detailProbes || !detailProbe || packetLoading) return;
    const probe = detailProbes[detailProbe] as Record<string, any> | undefined;
    const total = Number((detail as any)?.packet_sizes?.[detailProbe]?.total_bytes || 0);
    const initialOffset = Number(probe?.retained_bytes || 0);
    const offset = packetOffsets[detailProbe] ?? initialOffset;
    if (!total || offset >= total) {
      setPacketComplete((current) => ({ ...current, [detailProbe]: true }));
      return;
    }
    setPacketLoading(true);
    try {
      const packet = await request<GatewayPacketResponse>("GET", `/observe/packet/${detail.id}?probe=${encodeURIComponent(detailProbe)}&offset=${offset}&length=${PACKET_CHUNK_BYTES}`);
      setLoadedPacketText((current) => ({ ...current, [detailProbe]: `${current[detailProbe] || ""}${packet.text}` }));
      setPacketOffsets((current) => ({ ...current, [detailProbe]: packet.next_offset }));
      setPacketComplete((current) => ({ ...current, [detailProbe]: packet.complete }));
    } catch (nextError) {
      setError(String(nextError));
    } finally {
      setPacketLoading(false);
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
  const activeProbeBaseValue = gatewayProbeValue(activeProbe, detailView);
  const activeProbeValue = detailView === "raw-text" && typeof activeProbeBaseValue === "string"
    ? `${activeProbeBaseValue}${loadedPacketText[detailProbe] || ""}`
    : activeProbeBaseValue;
  const activeProbeTruncation = gatewayProbeTruncation(activeProbe);
  const activePacketTotal = Number((detail as any)?.packet_sizes?.[detailProbe]?.total_bytes || 0);
  const activePacketOffset = packetOffsets[detailProbe] ?? Number(activeProbe?.retained_bytes || 0);
  const canLoadMore = Boolean(detailProbes && detailView === "raw-text" && activePacketTotal > activePacketOffset && !packetComplete[detailProbe]);
  const showPreviewTruncation = Boolean(activeProbeTruncation && !packetComplete[detailProbe]);
  const selectedValue = detailProbes ? activeProbeValue : detail;
  const detailText = detailView === "conversation"
    ? conversationText(selectedValue) || gatewayText(lang, "此视图不可用：未识别到对话消息", "This view is unavailable: no conversation messages were detected")
    : detailProbes
    ? activeProbeValue == null
      ? gatewayText(lang, "此视图不可用", "This view is unavailable")
      : typeof activeProbeValue === "string"
        ? activeProbeValue
        : JSON.stringify(activeProbeValue, null, 2)
    : JSON.stringify(detail, null, 2);
  const detailMatches = React.useMemo(() => matchRanges(detailText, detailQuery), [detailText, detailQuery]);
  React.useEffect(() => {
    if (!detailMatches.length) {
      setDetailMatchIndex(0);
      return;
    }
    setDetailMatchIndex((current) => Math.min(current, detailMatches.length - 1));
  }, [detailMatches.length]);
  React.useLayoutEffect(() => {
    if (!detail || !detailQuery || !detailMatches.length) return;
    activeDetailMatchRef.current?.scrollIntoView({ block: "center", inline: "nearest" });
  }, [detail, detailMatches.length, detailMatchIndex, detailQuery, detailProbe, detailText, detailView]);
  React.useEffect(() => {
    const handleFindShortcut = (event: KeyboardEvent) => {
      if (!detail || !(event.ctrlKey || event.metaKey) || event.key.toLowerCase() !== "f") return;
      event.preventDefault();
      detailSearchRef.current?.focus();
      detailSearchRef.current?.select();
    };
    document.addEventListener("keydown", handleFindShortcut);
    return () => document.removeEventListener("keydown", handleFindShortcut);
  }, [detail]);

  const copyDetail = async () => {
    try {
      await navigator.clipboard.writeText(detailText);
    } catch {
      const textarea = document.createElement("textarea");
      textarea.value = detailText;
      textarea.style.position = "fixed";
      textarea.style.opacity = "0";
      document.body.appendChild(textarea);
      textarea.select();
      document.execCommand("copy");
      textarea.remove();
    }
  };
  const moveMatch = (direction: number) => {
    if (!detailMatches.length) return;
    setDetailMatchIndex((current) => (current + direction + detailMatches.length) % detailMatches.length);
  };
  const highlightedDetail = React.useMemo(() => {
    if (!detailQuery || !detailMatches.length) return detailText;
    const parts: React.ReactNode[] = [];
    let cursor = 0;
    detailMatches.forEach((range, index) => {
      if (range.start > cursor) parts.push(detailText.slice(cursor, range.start));
      parts.push(
        <mark
          key={`${range.start}-${range.end}`}
          ref={index === detailMatchIndex ? activeDetailMatchRef : undefined}
          className={index === detailMatchIndex ? "cx-gateway-find-match cx-gateway-find-match--active" : "cx-gateway-find-match"}
        >
          {detailText.slice(range.start, range.end)}
        </mark>,
      );
      cursor = range.end;
    });
    if (cursor < detailText.length) parts.push(detailText.slice(cursor));
    return parts;
  }, [detailMatchIndex, detailMatches, detailQuery, detailText]);
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
            <span className="cx-page-panel-header__meta">{observe.retained_count} / {observe.capture_limit} | {formatBytes(observe.stored_bytes)} / {formatBytes(observe.capture_total_bytes)} | {gatewayText(lang, "淘汰", "Evicted")} {observe.evicted_count} | {gatewayText(lang, "丢弃", "Dropped")} {observe.capture_dropped_count}</span>
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
            <label className="ui-field ui-field--compact cx-gateway-limit">
              <span className="ui-field__label">Archive total (MiB)</span>
              <span className="cx-gateway-limit__control">
                <input className="ui-field__control" value={totalDraft} onChange={(event) => setTotalDraft(event.target.value)} disabled={disabled} inputMode="numeric" />
              </span>
            </label>
            <label className="ui-field ui-field--compact cx-gateway-limit">
              <span className="ui-field__label">Record max (MiB)</span>
              <span className="cx-gateway-limit__control">
                <input className="ui-field__control" value={recordDraft} onChange={(event) => setRecordDraft(event.target.value)} disabled={disabled} inputMode="numeric" />
                <IconButton icon={<Save size={16} />} label="Save archive settings" variant="neutral" size="sm" onClick={saveArchiveSettings} disabled={disabled} />
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
        <div className="cx-gateway-detail-toolbar">
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
                <option value="conversation">conversation</option>
              </select>
            </label>
          </div>
          )}
          <div className="cx-gateway-detail-actions">
            <div className="cx-gateway-find">
              <Search size={15} aria-hidden="true" />
              <input
                ref={detailSearchRef}
                aria-label={gatewayText(lang, "查找内容", "Find in details")}
                value={detailQuery}
                onChange={(event) => { setDetailQuery(event.target.value); setDetailMatchIndex(0); }}
                placeholder={gatewayText(lang, "查找", "Find")}
              />
              {detailQuery && <span className="cx-gateway-find-count">{detailMatches.length ? `${detailMatchIndex + 1}/${detailMatches.length}` : "0/0"}</span>}
              <IconButton icon={<ChevronUp size={15} />} label={gatewayText(lang, "上一个匹配", "Previous match")} variant="ghost" size="sm" onClick={() => moveMatch(-1)} disabled={!detailMatches.length} />
              <IconButton icon={<ChevronDown size={15} />} label={gatewayText(lang, "下一个匹配", "Next match")} variant="ghost" size="sm" onClick={() => moveMatch(1)} disabled={!detailMatches.length} />
              {detailQuery && <IconButton icon={<X size={15} />} label={gatewayText(lang, "清除查找", "Clear find")} variant="ghost" size="sm" onClick={() => setDetailQuery("")} />}
            </div>
            <IconButton icon={<Clipboard size={16} />} label={gatewayText(lang, "复制全部内容", "Copy all content")} variant="neutral" size="sm" onClick={() => void copyDetail()} />
          </div>
        </div>
        {detail?.archive_status === "archive_limit_exceeded" && <div className="cx-gateway-truncation">archive_limit_exceeded: this request exceeded the configured per-record archive limit and was not stored.</div>}
        {showPreviewTruncation && <div className="cx-gateway-truncation">{activeProbeTruncation}</div>}
        <pre ref={detailPreRef} className="cx-gateway-detail-pre">{highlightedDetail}</pre>
        {canLoadMore && <div className="cx-gateway-load-more"><Button variant="secondary" size="sm" onClick={() => void loadMorePacket()} disabled={packetLoading}>{packetLoading ? "Loading..." : `Load more (${formatBytes(activePacketTotal - activePacketOffset)} remaining)`}</Button></div>}
      </ModalShell>
    </section>
  );
}
