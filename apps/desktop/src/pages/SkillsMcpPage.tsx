import { useId, useState } from "react";
import type { ChangeEvent, RefObject } from "react";
import {
  AlertCircle,
  Blocks,
  Download,
  FolderInput,
  Loader2,
  PackageOpen,
  PencilLine,
  PlugZap,
  RefreshCw,
  Sparkles,
  Upload,
} from "lucide-react";

import { PageTransition } from "../components/PageTransition";
import { Button, IconButton, ModalShell, StatusBadge, Toggle, cx } from "../components/ui";
import type { Lang, ManagedMcpServer, ManagedSkill, SkillsMcpImportPreview, SkillsMcpState } from "../types";
import "../styles/skills-prompts-pages.css";

export type SkillsMcpTab = "mcp" | "skills";
export type SkillsMcpNoteKind = "mcp" | "skill";

type MaybeAsyncAction = () => void | Promise<void>;

export type SkillsMcpPageProps = {
  lang: Lang;
  state: SkillsMcpState | null;
  activeTab: SkillsMcpTab;
  actionBusy: string;
  noteBusyKey: string;
  importOpen: boolean;
  importPreview: SkillsMcpImportPreview | null;
  zipInputRef: RefObject<HTMLInputElement>;
  className?: string;
  onTabChange: (tab: SkillsMcpTab) => void;
  onLoad: MaybeAsyncAction;
  onOpenImportPreview: MaybeAsyncAction;
  onCloseImportPreview: () => void;
  onConfirmImport: MaybeAsyncAction;
  onInstallZip: (file?: File | null) => void | Promise<void>;
  onCheckUpdates: MaybeAsyncAction;
  onToggleSkill: (id: string, enabled: boolean) => void | Promise<void>;
  onToggleMcp: (id: string, enabled: boolean) => void | Promise<void>;
  onSaveNote: (itemKind: SkillsMcpNoteKind, id: string, note: string) => Promise<boolean>;
};

type NoteTarget = {
  itemKind: SkillsMcpNoteKind;
  id: string;
  name: string;
  note: string;
};

type SkillsMcpCopy = ReturnType<typeof getCopy>;

function getCopy(lang: Lang) {
  return lang === "zh"
    ? {
        eyebrow: "SKILLS / MCP",
        title: "技能和MCP",
        description: "管理 Codex 当前可用的 Skills 与 MCP，导入已有内容、安装技能包并控制启用状态。",
        refresh: "刷新",
        importExisting: "导入已有",
        installZip: "从 ZIP 安装",
        checkUpdates: "检查更新",
        loading: "正在读取本地 Skills / MCP...",
        mcpHelp: (count: number) => `当前共有 ${count} 个 MCP，启用后会写入 Codex config.toml。`,
        skillsHelp: (count: number) => `当前共有 ${count} 个 Skills，启用后会放入 Codex skills 目录。`,
        total: (count: number) => `共 ${count} 个`,
        noMcp: "还没有发现 MCP，请先导入已有内容。",
        noSkills: "还没有发现 Skills，请导入已有内容或安装 ZIP 技能包。",
        enableMcp: "启用 MCP",
        disableMcp: "关闭 MCP",
        enableSkill: "启用 Skill",
        disableSkill: "禁用 Skill",
        updateStatus: "更新状态",
        addNote: "添加备注",
        editNote: "编辑备注",
        noteTitle: "自定义备注",
        noteDescription: (name: string) => `为“${name}”添加便于识别的说明。`,
        noteLabel: "备注内容",
        notePlaceholder: "例如：用于查询项目文档，依赖本地 Node.js",
        noteEmptyHint: "留空保存将清除现有备注",
        noteCount: (count: number) => `${count} / 1000`,
        saveNote: "保存备注",
        savingNote: "保存中",
        importTitle: "确认导入已有内容",
        importDescription: "以下内容来自本机现有配置。导入后可在此统一启用或禁用。",
        noImportItems: "没有发现可导入的已有 Skills / MCP。",
        noImportSkills: "没有可导入的 Skill",
        noImportMcp: "没有可导入的 MCP",
        cancel: "取消",
        importing: "正在导入",
        confirmImport: "导入",
        warnings: "需要留意",
      }
    : {
        eyebrow: "SKILLS / MCP",
        title: "Skills & MCP",
        description: "Manage the Skills and MCP servers available to Codex, import existing items, install packages, and control their state.",
        refresh: "Refresh",
        importExisting: "Import existing",
        installZip: "Install ZIP",
        checkUpdates: "Check updates",
        loading: "Loading local Skills / MCP...",
        mcpHelp: (count: number) => `${count} MCP server(s). Enabling one writes it to Codex config.toml.`,
        skillsHelp: (count: number) => `${count} Skill(s). Enabling one places it in the Codex skills directory.`,
        total: (count: number) => `${count} total`,
        noMcp: "No MCP server found. Import existing items first.",
        noSkills: "No Skills found. Import existing items or install a ZIP package.",
        enableMcp: "Enable MCP",
        disableMcp: "Disable MCP",
        enableSkill: "Enable Skill",
        disableSkill: "Disable Skill",
        updateStatus: "Update status",
        addNote: "Add note",
        editNote: "Edit note",
        noteTitle: "Custom note",
        noteDescription: (name: string) => `Add a description that helps you recognize “${name}”.`,
        noteLabel: "Note",
        notePlaceholder: "For example: searches project docs; requires local Node.js",
        noteEmptyHint: "Save an empty note to remove it",
        noteCount: (count: number) => `${count} / 1000`,
        saveNote: "Save note",
        savingNote: "Saving",
        importTitle: "Confirm import",
        importDescription: "These items were found in the existing local configuration. Import them to manage their state here.",
        noImportItems: "No existing Skills / MCP items were found.",
        noImportSkills: "No Skill to import",
        noImportMcp: "No MCP to import",
        cancel: "Cancel",
        importing: "Importing",
        confirmImport: "Import",
        warnings: "Review these items",
      };
}

function run(action: MaybeAsyncAction) {
  void action();
}

function McpRow({
  server,
  busy,
  disabled,
  copy,
  onToggle,
  onEditNote,
}: {
  server: ManagedMcpServer;
  busy: boolean;
  disabled: boolean;
  copy: SkillsMcpCopy;
  onToggle: SkillsMcpPageProps["onToggleMcp"];
  onEditNote: (target: NoteTarget) => void;
}) {
  const name = server.name || server.id;
  const details = [server.note, server.summary, server.transport, server.source].filter(Boolean).join("\n");
  return (
    <article className="cx-skills-row">
      <div className={cx("cx-skills-row-copy", Boolean(server.note) && "cx-skills-row-copy--noted")}>
        <strong title={details ? `${name}\n${details}` : name}>{name}</strong>
        {server.note && <span className="cx-skills-row-note" title={server.note}>{server.note}</span>}
      </div>
      <div className="cx-skills-row-control">
        {busy && <Loader2 size={15} className="cx-skills-spin" aria-hidden="true" />}
        <IconButton
          icon={<PencilLine size={15} strokeWidth={1.9} />}
          label={server.note ? copy.editNote : copy.addNote}
          size="sm"
          className="cx-skills-note-button"
          onClick={() => onEditNote({ itemKind: "mcp", id: server.id, name, note: server.note || "" })}
          disabled={disabled}
        />
        <Toggle
          checked={server.enabled}
          onCheckedChange={(enabled) => void onToggle(server.id, enabled)}
          disabled={disabled}
          aria-label={server.enabled ? copy.disableMcp : copy.enableMcp}
        />
      </div>
    </article>
  );
}

function SkillRow({
  skill,
  busy,
  disabled,
  copy,
  onToggle,
  onEditNote,
}: {
  skill: ManagedSkill;
  busy: boolean;
  disabled: boolean;
  copy: SkillsMcpCopy;
  onToggle: SkillsMcpPageProps["onToggleSkill"];
  onEditNote: (target: NoteTarget) => void;
}) {
  const name = skill.name || skill.directory;
  const updateTone = skill.updateStatus.includes("失败") || skill.updateStatus.toLowerCase().includes("fail")
    ? "danger"
    : skill.updateStatus.includes("新版本") || skill.updateStatus.toLowerCase().includes("update")
      ? "warning"
      : "neutral";

  const showUpdateStatus = Boolean(skill.updateStatus && skill.updateStatus !== "未检查");
  const details = [skill.note, skill.description, skill.source].filter(Boolean).join("\n");

  return (
    <article className="cx-skills-row">
      <div className={cx("cx-skills-row-copy", Boolean(skill.note) && "cx-skills-row-copy--noted")}>
        <strong title={details ? `${name}\n${details}` : name}>{name}</strong>
        {skill.note && <span className="cx-skills-row-note" title={skill.note}>{skill.note}</span>}
        {showUpdateStatus && (
          <StatusBadge tone={updateTone} dot={false} title={`${copy.updateStatus}: ${skill.updateStatus}`}>
            {skill.updateStatus}
          </StatusBadge>
        )}
      </div>
      <div className="cx-skills-row-control">
        {busy && <Loader2 size={15} className="cx-skills-spin" aria-hidden="true" />}
        <IconButton
          icon={<PencilLine size={15} strokeWidth={1.9} />}
          label={skill.note ? copy.editNote : copy.addNote}
          size="sm"
          className="cx-skills-note-button"
          onClick={() => onEditNote({ itemKind: "skill", id: skill.id, name, note: skill.note || "" })}
          disabled={disabled}
        />
        <Toggle
          checked={skill.enabled}
          onCheckedChange={(enabled) => void onToggle(skill.id, enabled)}
          disabled={disabled}
          aria-label={skill.enabled ? copy.disableSkill : copy.enableSkill}
        />
      </div>
    </article>
  );
}

function ImportPreviewContent({ preview, copy }: { preview: SkillsMcpImportPreview | null; copy: SkillsMcpCopy }) {
  const skillCount = preview?.skills.length ?? 0;
  const mcpCount = preview?.mcpServers.length ?? 0;
  if (!preview || skillCount + mcpCount === 0) {
    return (
      <div className="cx-skills-empty cx-skills-empty--compact">
        <FolderInput size={22} strokeWidth={1.7} aria-hidden="true" />
        <span>{copy.noImportItems}</span>
      </div>
    );
  }

  return (
    <div className="cx-skills-import-content">
      <div className="cx-skills-import-summary" aria-label={`${skillCount} Skills, ${mcpCount} MCP`}>
        <div><strong>{skillCount}</strong><span>Skills</span></div>
        <div><strong>{mcpCount}</strong><span>MCP</span></div>
      </div>

      <section className="cx-skills-import-section" aria-labelledby="cx-skills-import-skills">
        <div className="cx-skills-import-section-head">
          <strong id="cx-skills-import-skills">Skills</strong>
          <span>{skillCount}</span>
        </div>
        <div className="cx-skills-import-list">
          {skillCount === 0 ? <p>{copy.noImportSkills}</p> : preview.skills.map((skill) => (
            <div className="cx-skills-import-row" key={`skill-${skill.id}-${skill.path}`}>
              <strong>{skill.name || skill.directory}</strong>
              <span title={skill.path}>{skill.directory}</span>
              <em>{skill.source}</em>
            </div>
          ))}
        </div>
      </section>

      <section className="cx-skills-import-section" aria-labelledby="cx-skills-import-mcp">
        <div className="cx-skills-import-section-head">
          <strong id="cx-skills-import-mcp">MCP</strong>
          <span>{mcpCount}</span>
        </div>
        <div className="cx-skills-import-list">
          {mcpCount === 0 ? <p>{copy.noImportMcp}</p> : preview.mcpServers.map((server) => (
            <div className="cx-skills-import-row" key={`mcp-${server.id}-${server.source}`}>
              <strong>{server.name || server.id}</strong>
              <span>{server.transport}</span>
              <em>{server.source}</em>
            </div>
          ))}
        </div>
      </section>

      {preview.warnings.length > 0 && (
        <section className="cx-skills-import-warnings" aria-label={copy.warnings}>
          <strong><AlertCircle size={15} aria-hidden="true" />{copy.warnings}</strong>
          {preview.warnings.map((warning, index) => <p key={`${index}-${warning}`}>{warning}</p>)}
        </section>
      )}
    </div>
  );
}

export function SkillsMcpPage({
  lang,
  state,
  activeTab,
  actionBusy,
  noteBusyKey,
  importOpen,
  importPreview,
  zipInputRef,
  className,
  onTabChange,
  onLoad,
  onOpenImportPreview,
  onCloseImportPreview,
  onConfirmImport,
  onInstallZip,
  onCheckUpdates,
  onToggleSkill,
  onToggleMcp,
  onSaveNote,
}: SkillsMcpPageProps) {
  const copy = getCopy(lang);
  const tabId = useId();
  const anyBusy = Boolean(actionBusy);
  const importBusy = actionBusy === "importExistingSkillsMcp";
  const activeItems = activeTab === "mcp" ? state?.mcpServers ?? [] : state?.skills ?? [];
  const [noteTarget, setNoteTarget] = useState<NoteTarget | null>(null);
  const [noteDraft, setNoteDraft] = useState("");
  const noteBusy = noteTarget
    ? noteBusyKey === `note:${noteTarget.itemKind}:${noteTarget.id}`
    : false;
  const openNoteEditor = (target: NoteTarget) => {
    setNoteTarget(target);
    setNoteDraft(target.note);
  };
  const closeNoteEditor = () => {
    if (!noteBusy) setNoteTarget(null);
  };
  const updateNoteDraft = (value: string) => {
    setNoteDraft(Array.from(value).slice(0, 1000).join(""));
  };
  const saveNote = async () => {
    if (!noteTarget || noteBusy) return;
    if (await onSaveNote(noteTarget.itemKind, noteTarget.id, noteDraft)) {
      setNoteTarget(null);
    }
  };
  const handleZipChange = (event: ChangeEvent<HTMLInputElement>) => {
    void onInstallZip(event.currentTarget.files?.[0]);
  };

  return (
    <section className={cx("cx-skills-page", className)} aria-label={copy.title}>
      <header className="cx-skills-header">
        <div className="cx-skills-heading">
          <p><Blocks size={14} aria-hidden="true" />{copy.eyebrow}</p>
          <h2>{copy.title}</h2>
          <span>{copy.description}</span>
        </div>
        <div className="cx-skills-actions">
          <input
            ref={zipInputRef}
            className="cx-skills-file-input"
            type="file"
            accept=".zip,application/zip"
            onChange={handleZipChange}
            disabled={anyBusy}
            tabIndex={-1}
            aria-hidden="true"
          />
          <Button
            variant="secondary"
            icon={actionBusy === "loadSkillsMcp" ? <Loader2 className="cx-skills-spin" /> : <RefreshCw />}
            onClick={() => run(onLoad)}
            disabled={anyBusy}
          >
            {copy.refresh}
          </Button>
          <Button
            variant="secondary"
            icon={actionBusy === "previewExistingSkillsMcp" ? <Loader2 className="cx-skills-spin" /> : <Download />}
            onClick={() => run(onOpenImportPreview)}
            disabled={anyBusy}
          >
            {copy.importExisting}
          </Button>
          <Button
            variant="secondary"
            icon={actionBusy === "installSkillZip" ? <Loader2 className="cx-skills-spin" /> : <Upload />}
            onClick={() => zipInputRef.current?.click()}
            disabled={anyBusy}
          >
            {copy.installZip}
          </Button>
          <Button
            icon={actionBusy === "checkSkillUpdates" ? <Loader2 className="cx-skills-spin" /> : <Sparkles />}
            onClick={() => run(onCheckUpdates)}
            disabled={anyBusy}
          >
            {copy.checkUpdates}
          </Button>
        </div>
      </header>

      {!state ? (
        <div className="cx-skills-empty cx-skills-empty--loading" role="status">
          <Loader2 size={23} className="cx-skills-spin" aria-hidden="true" />
          <span>{copy.loading}</span>
        </div>
      ) : (
        <>
          <div className="cx-skills-tabs" role="tablist" aria-label="Skills and MCP">
            <button
              id={`${tabId}-mcp-tab`}
              type="button"
              role="tab"
              aria-selected={activeTab === "mcp"}
              aria-controls={`${tabId}-mcp-panel`}
              className={cx("cx-skills-tab", activeTab === "mcp" && "cx-skills-tab--active")}
              onClick={() => onTabChange("mcp")}
            >
              MCP <span>{state.mcpServers.length}</span>
            </button>
            <button
              id={`${tabId}-skills-tab`}
              type="button"
              role="tab"
              aria-selected={activeTab === "skills"}
              aria-controls={`${tabId}-skills-panel`}
              className={cx("cx-skills-tab", activeTab === "skills" && "cx-skills-tab--active")}
              onClick={() => onTabChange("skills")}
            >
              Skills <span>{state.skills.length}</span>
            </button>
          </div>

          <p className="cx-skills-help">
            <span aria-hidden="true" />
            {activeTab === "mcp" ? copy.mcpHelp(state.mcpServers.length) : copy.skillsHelp(state.skills.length)}
          </p>

          <PageTransition pageKey={`skills-mcp:${activeTab}`}>
            <section
              id={`${tabId}-${activeTab}-panel`}
              role="tabpanel"
              aria-labelledby={`${tabId}-${activeTab}-tab`}
              className="cx-skills-list-panel"
            >
              <div className="cx-skills-list-head">
                <div>
                  {activeTab === "mcp" ? <PlugZap size={17} aria-hidden="true" /> : <PackageOpen size={17} aria-hidden="true" />}
                  <h3>{activeTab === "mcp" ? "MCP" : "Skills"}</h3>
                </div>
                <span>{copy.total(activeItems.length)}</span>
              </div>

              <div className="cx-skills-list">
                {activeTab === "mcp" ? (
                  state.mcpServers.length === 0 ? (
                    <div className="cx-skills-empty"><PlugZap size={22} aria-hidden="true" /><span>{copy.noMcp}</span></div>
                  ) : state.mcpServers.map((server) => (
                    <McpRow
                      key={server.id}
                      server={server}
                      busy={actionBusy === `mcp:${server.id}`}
                      disabled={anyBusy}
                      copy={copy}
                      onToggle={onToggleMcp}
                      onEditNote={openNoteEditor}
                    />
                  ))
                ) : state.skills.length === 0 ? (
                  <div className="cx-skills-empty"><PackageOpen size={22} aria-hidden="true" /><span>{copy.noSkills}</span></div>
                ) : state.skills.map((skill) => (
                  <SkillRow
                    key={skill.id}
                    skill={skill}
                    busy={actionBusy === `skill:${skill.id}`}
                    disabled={anyBusy}
                    copy={copy}
                    onToggle={onToggleSkill}
                    onEditNote={openNoteEditor}
                  />
                ))}
              </div>
            </section>
          </PageTransition>

          {state.warnings.length > 0 && (
            <section className="cx-skills-warnings" aria-label={copy.warnings}>
              {state.warnings.map((warning, index) => (
                <p key={`${index}-${warning}`}><AlertCircle size={15} aria-hidden="true" />{warning}</p>
              ))}
            </section>
          )}
        </>
      )}

      <ModalShell
        open={importOpen}
        onClose={() => {
          if (!importBusy) onCloseImportPreview();
        }}
        title={copy.importTitle}
        description={copy.importDescription}
        size="lg"
        closeOnBackdrop={!importBusy}
        closeOnEscape={!importBusy}
        showCloseButton={!importBusy}
        className="cx-skills-import-modal"
        bodyClassName="cx-skills-import-modal-body"
        footer={(
          <>
            <Button variant="secondary" onClick={onCloseImportPreview} disabled={importBusy}>
              {copy.cancel}
            </Button>
            <Button
              icon={importBusy ? <Loader2 className="cx-skills-spin" /> : <Download />}
              onClick={() => run(onConfirmImport)}
              disabled={importBusy || !importPreview || importPreview.skills.length + importPreview.mcpServers.length === 0}
            >
              {importBusy ? copy.importing : copy.confirmImport}
            </Button>
          </>
        )}
      >
        <ImportPreviewContent preview={importPreview} copy={copy} />
      </ModalShell>

      <ModalShell
        open={Boolean(noteTarget)}
        onClose={closeNoteEditor}
        title={copy.noteTitle}
        description={noteTarget ? copy.noteDescription(noteTarget.name) : undefined}
        size="sm"
        closeOnBackdrop={!noteBusy}
        closeOnEscape={!noteBusy}
        showCloseButton={!noteBusy}
        className="cx-skills-note-modal"
        footer={(
          <>
            <Button variant="secondary" onClick={closeNoteEditor} disabled={noteBusy}>
              {copy.cancel}
            </Button>
            <Button
              icon={noteBusy ? <Loader2 className="cx-skills-spin" /> : <PencilLine />}
              onClick={() => void saveNote()}
              disabled={noteBusy}
            >
              {noteBusy ? copy.savingNote : copy.saveNote}
            </Button>
          </>
        )}
      >
        <label className="cx-skills-note-field">
          <span>{copy.noteLabel}</span>
          <textarea
            value={noteDraft}
            onChange={(event) => updateNoteDraft(event.target.value)}
            placeholder={copy.notePlaceholder}
            rows={5}
            disabled={noteBusy}
            data-initial-focus
          />
        </label>
        <div className="cx-skills-note-meta">
          <span>{copy.noteEmptyHint}</span>
          <span>{copy.noteCount(Array.from(noteDraft).length)}</span>
        </div>
      </ModalShell>
    </section>
  );
}
