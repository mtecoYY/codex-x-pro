mod mcp;
mod skills;
mod types;

pub(crate) use mcp::{sort_managed_mcp_servers, toggle_codex_mcp_inner};
pub(crate) use skills::{
    check_skill_updates_inner, install_skill_zip_inner, normalize_legacy_zip_skill_dirs,
    sort_managed_skills, toggle_codex_skill_inner,
};
pub(crate) use types::{
    ManagedMcpServer, SkillsMcpActionResult, SkillsMcpImportPreview, SkillsMcpState,
};

#[cfg(test)]
pub(crate) use skills::read_skill_metadata;
#[cfg(test)]
pub(crate) use types::ManagedSkill;

use crate::error::Result;
use crate::file_io::{ensure_directory, io_err};
use crate::paths::{home_dir, normalized_path_scope};
use crate::resolve_codex_dir;
use crate::{now_rfc3339, open_db};
use mcp::{
    db_managed_mcp, import_ccswitch_mcp_servers_for_codex, list_mcp_from_config, mcp_summary,
    preview_ccswitch_mcp_servers_for_codex, save_managed_mcp,
};
use rusqlite::params;
use skills::{
    codex_skills_dir, copy_dir_recursive, disabled_skills_dir, sanitize_dir_name, scan_skill_dir,
};
use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::Path;

const SKILLS_MCP_NOTE_MAX_CHARS: usize = 1000;

fn normalized_note_item_kind(item_kind: &str) -> Result<&'static str> {
    match item_kind.trim().to_ascii_lowercase().as_str() {
        "skill" => Ok("skill"),
        "mcp" => Ok("mcp"),
        _ => Err(crate::error::CodexxError::Config(
            "备注类型必须是 skill 或 mcp".to_string(),
        )),
    }
}

fn skills_mcp_notes(codex_dir: &Path) -> Result<HashMap<(String, String), String>> {
    let conn = open_db()?;
    let mut stmt = conn
        .prepare(
            "SELECT item_kind, item_id, note
             FROM skills_mcp_notes
             WHERE codex_dir = ?1
             ORDER BY item_kind ASC, item_id ASC",
        )
        .map_err(|error| crate::error::CodexxError::Database(error.to_string()))?;
    let rows = stmt
        .query_map([normalized_path_scope(codex_dir)], |row| {
            Ok((
                (row.get::<_, String>(0)?, row.get::<_, String>(1)?),
                row.get::<_, String>(2)?,
            ))
        })
        .map_err(|error| crate::error::CodexxError::Database(error.to_string()))?;
    let mut notes = HashMap::new();
    for row in rows {
        let (key, note) =
            row.map_err(|error| crate::error::CodexxError::Database(error.to_string()))?;
        notes.insert(key, note);
    }
    Ok(notes)
}

fn attach_skills_mcp_notes(
    codex_dir: &Path,
    skills: &mut [types::ManagedSkill],
    mcp_servers: &mut [ManagedMcpServer],
) -> Result<()> {
    let notes = skills_mcp_notes(codex_dir)?;
    for skill in skills {
        skill.note = notes.get(&("skill".to_string(), skill.id.clone())).cloned();
    }
    for server in mcp_servers {
        server.note = notes.get(&("mcp".to_string(), server.id.clone())).cloned();
    }
    Ok(())
}

fn extend_unmanaged_mcp_candidates(
    output: &mut Vec<ManagedMcpServer>,
    seen_ids: &mut HashSet<String>,
    candidates: impl IntoIterator<Item = ManagedMcpServer>,
) {
    for server in candidates {
        if seen_ids.insert(server.id.clone()) {
            output.push(server);
        }
    }
}

pub(crate) fn build_skills_mcp_state_inner(config_dir: Option<String>) -> Result<SkillsMcpState> {
    let codex_dir = resolve_codex_dir(config_dir)?;
    let skills_dir = codex_skills_dir(&codex_dir);
    let disabled_dir = disabled_skills_dir()?;
    let mut warnings = Vec::new();
    let mut skills = Vec::new();
    let mut seen = HashSet::new();
    if let Err(e) = normalize_legacy_zip_skill_dirs(&skills_dir) {
        warnings.push(format!("修正 ZIP Skill 目录名失败: {e}"));
    }
    if let Err(e) = normalize_legacy_zip_skill_dirs(&disabled_dir) {
        warnings.push(format!("修正已禁用 ZIP Skill 目录名失败: {e}"));
    }
    if let Err(e) = scan_skill_dir(&skills_dir, true, "Codex", &mut skills, &mut seen) {
        warnings.push(e.to_string());
    }
    if let Err(e) = scan_skill_dir(
        &disabled_dir,
        false,
        "Codex-X 已禁用",
        &mut skills,
        &mut seen,
    ) {
        warnings.push(e.to_string());
    }

    let mut mcp_servers = list_mcp_from_config(&codex_dir)?;
    let enabled_ids: HashSet<String> = mcp_servers.iter().map(|s| s.id.clone()).collect();
    for (id, name, config, enabled) in db_managed_mcp()? {
        if enabled_ids.contains(&id) {
            continue;
        }
        let (transport, command, url, summary) = mcp_summary(&config);
        mcp_servers.push(ManagedMcpServer {
            id,
            name,
            transport,
            enabled,
            source: "Codex-X".to_string(),
            summary,
            note: None,
            command,
            url,
            config_json: config,
        });
    }
    attach_skills_mcp_notes(&codex_dir, &mut skills, &mut mcp_servers)?;
    sort_managed_mcp_servers(&mut mcp_servers);
    sort_managed_skills(&mut skills);
    Ok(SkillsMcpState {
        codex_dir: codex_dir.display().to_string(),
        codex_skills_dir: skills_dir.display().to_string(),
        disabled_skills_dir: disabled_dir.display().to_string(),
        skills,
        mcp_servers,
        warnings,
    })
}

pub(crate) fn save_skills_mcp_note_inner(
    config_dir: Option<String>,
    item_kind: String,
    id: String,
    note: String,
) -> Result<SkillsMcpState> {
    let item_kind = normalized_note_item_kind(&item_kind)?;
    if id.trim().is_empty() {
        return Err(crate::error::CodexxError::Config(
            "备注对象 ID 不能为空".to_string(),
        ));
    }
    let id = id.as_str();
    let note = note.trim();
    if note.chars().count() > SKILLS_MCP_NOTE_MAX_CHARS {
        return Err(crate::error::CodexxError::Config(format!(
            "备注不能超过 {SKILLS_MCP_NOTE_MAX_CHARS} 个字符"
        )));
    }

    let codex_dir = resolve_codex_dir(config_dir.clone())?;
    let codex_dir_scope = normalized_path_scope(&codex_dir);
    let current = build_skills_mcp_state_inner(config_dir.clone())?;
    let exists = match item_kind {
        "skill" => current.skills.iter().any(|skill| skill.id == id),
        "mcp" => current.mcp_servers.iter().any(|server| server.id == id),
        _ => false,
    };
    if !exists {
        return Err(crate::error::CodexxError::Config(format!(
            "未找到要备注的 {item_kind}: {id}"
        )));
    }

    let conn = open_db()?;
    if note.is_empty() {
        conn.execute(
            "DELETE FROM skills_mcp_notes
             WHERE codex_dir = ?1 AND item_kind = ?2 AND item_id = ?3",
            params![codex_dir_scope, item_kind, id],
        )
        .map_err(|error| crate::error::CodexxError::Database(error.to_string()))?;
    } else {
        conn.execute(
            "INSERT INTO skills_mcp_notes
                (codex_dir, item_kind, item_id, note, updated_at)
             VALUES (?1, ?2, ?3, ?4, ?5)
             ON CONFLICT(codex_dir, item_kind, item_id) DO UPDATE SET
                note = excluded.note,
                updated_at = excluded.updated_at",
            params![codex_dir_scope, item_kind, id, note, now_rfc3339()],
        )
        .map_err(|error| crate::error::CodexxError::Database(error.to_string()))?;
    }
    build_skills_mcp_state_inner(config_dir)
}

pub(crate) fn import_existing_skills_mcp_inner(
    config_dir: Option<String>,
) -> Result<SkillsMcpActionResult> {
    let codex_dir = resolve_codex_dir(config_dir.clone())?;
    let skills_dir = codex_skills_dir(&codex_dir);
    ensure_directory(&skills_dir)?;
    let mut imported_skills = 0usize;
    let candidates = vec![
        home_dir()?.join(".agents").join("skills"),
        home_dir()?.join(".cc-switch").join("skills"),
    ];
    for base in candidates {
        if !base.exists() {
            continue;
        }
        for entry in fs::read_dir(&base).map_err(|e| io_err(&base, e))? {
            let entry = entry.map_err(|e| io_err(&base, e))?;
            let src = entry.path();
            if !src.is_dir() || !src.join("SKILL.md").is_file() {
                continue;
            }
            let directory = sanitize_dir_name(&entry.file_name().to_string_lossy(), "skill");
            let dst = skills_dir.join(&directory);
            if !dst.exists() {
                copy_dir_recursive(&src, &dst)?;
                imported_skills += 1;
            }
        }
    }

    let mut imported_mcp = 0usize;
    let mut imported_mcp_ids = db_managed_mcp()?
        .into_iter()
        .map(|(id, _, _, _)| id)
        .collect::<HashSet<_>>();
    for server in list_mcp_from_config(&codex_dir)? {
        if !imported_mcp_ids.insert(server.id.clone()) {
            continue;
        }
        save_managed_mcp(&server.id, &server.name, &server.config_json, true)?;
        imported_mcp += 1;
    }
    imported_mcp += import_ccswitch_mcp_servers_for_codex(&codex_dir, &mut imported_mcp_ids)?;
    let state = build_skills_mcp_state_inner(config_dir)?;
    Ok(SkillsMcpActionResult {
        imported_skills,
        imported_mcp,
        message: format!("已导入 {imported_skills} 个 Skills，纳管 {imported_mcp} 个 MCP"),
        state,
    })
}

pub(crate) fn preview_existing_skills_mcp_inner(
    config_dir: Option<String>,
) -> Result<SkillsMcpImportPreview> {
    let codex_dir = resolve_codex_dir(config_dir)?;
    let skills_dir = codex_skills_dir(&codex_dir);
    let mut warnings = Vec::new();
    let mut skills = Vec::new();
    let mut seen = HashSet::new();
    let candidates = vec![
        home_dir()?.join(".agents").join("skills"),
        home_dir()?.join(".cc-switch").join("skills"),
    ];
    for base in candidates {
        if !base.exists() {
            continue;
        }
        let source = base
            .parent()
            .and_then(|p| p.file_name())
            .map(|s| s.to_string_lossy().to_string())
            .unwrap_or_else(|| "外部目录".to_string());
        let before = skills.len();
        if let Err(e) = scan_skill_dir(&base, false, &source, &mut skills, &mut seen) {
            warnings.push(e.to_string());
        }
        for skill in &mut skills[before..] {
            if skills_dir.join(&skill.directory).exists() {
                skill.update_status = "已存在，将跳过".to_string();
            } else {
                skill.update_status = "可导入".to_string();
            }
        }
    }
    skills.retain(|skill| skill.update_status != "已存在，将跳过");

    let mut config_mcp_servers = list_mcp_from_config(&codex_dir)?;
    for server in &mut config_mcp_servers {
        server.source = "config.toml".to_string();
    }
    let mut seen_mcp = db_managed_mcp()?
        .into_iter()
        .map(|(id, _, _, _)| id)
        .collect::<HashSet<_>>();
    let mut mcp_servers = Vec::new();
    extend_unmanaged_mcp_candidates(&mut mcp_servers, &mut seen_mcp, config_mcp_servers);
    extend_unmanaged_mcp_candidates(
        &mut mcp_servers,
        &mut seen_mcp,
        preview_ccswitch_mcp_servers_for_codex(&codex_dir)?,
    );
    sort_managed_skills(&mut skills);
    sort_managed_mcp_servers(&mut mcp_servers);
    Ok(SkillsMcpImportPreview {
        skills,
        mcp_servers,
        warnings,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn mcp_candidate(id: &str, source: &str) -> ManagedMcpServer {
        ManagedMcpServer {
            id: id.to_string(),
            name: id.to_string(),
            transport: "stdio".to_string(),
            enabled: true,
            source: source.to_string(),
            summary: id.to_string(),
            note: None,
            command: Some(id.to_string()),
            url: None,
            config_json: json!({ "command": id }),
        }
    }

    #[test]
    fn mcp_import_preview_excludes_managed_and_duplicate_ids() {
        let mut seen = HashSet::from(["alpha".to_string()]);
        let mut candidates = Vec::new();
        extend_unmanaged_mcp_candidates(
            &mut candidates,
            &mut seen,
            [
                mcp_candidate("alpha", "config.toml"),
                mcp_candidate("beta", "config.toml"),
            ],
        );
        extend_unmanaged_mcp_candidates(
            &mut candidates,
            &mut seen,
            [
                mcp_candidate("beta", "cc-switch"),
                mcp_candidate("gamma", "cc-switch"),
            ],
        );

        assert_eq!(
            candidates
                .iter()
                .map(|server| (server.id.as_str(), server.source.as_str()))
                .collect::<Vec<_>>(),
            vec![("beta", "config.toml"), ("gamma", "cc-switch")]
        );

        let mut imported_ids = seen;
        let mut second_preview = Vec::new();
        extend_unmanaged_mcp_candidates(
            &mut second_preview,
            &mut imported_ids,
            [
                mcp_candidate("alpha", "config.toml"),
                mcp_candidate("beta", "config.toml"),
                mcp_candidate("gamma", "cc-switch"),
            ],
        );
        assert!(second_preview.is_empty());
    }

    #[test]
    fn custom_notes_are_isolated_by_kind_and_codex_home_and_empty_text_clears_them() {
        let _db_guard = crate::app_db::test_db_guard();
        let item_id = format!("shared-note-item-{}", std::process::id());
        let spaced_id = " spaced-mcp ";
        let codex_dir =
            std::env::temp_dir().join(format!("codex-x-note-state-{}", std::process::id()));
        let other_codex_dir =
            std::env::temp_dir().join(format!("codex-x-note-state-other-{}", std::process::id()));
        let prepare_codex_dir = |dir: &Path| {
            let _ = fs::remove_dir_all(dir);
            let skill_dir = dir.join("skills").join(&item_id);
            fs::create_dir_all(&skill_dir).expect("create test skill directory");
            fs::write(
                skill_dir.join("SKILL.md"),
                format!("---\nname: {item_id}\ndescription: test\n---\n"),
            )
            .expect("write test skill");
            fs::write(
                dir.join("config.toml"),
                format!(
                    "[mcp_servers.{item_id}]\ncommand = \"test-server\"\n\
                     [mcp_servers.\"{spaced_id}\"]\ncommand = \"spaced-server\"\n"
                ),
            )
            .expect("write test MCP config");
        };
        prepare_codex_dir(&codex_dir);
        prepare_codex_dir(&other_codex_dir);

        let conn = open_db().expect("open app database");
        conn.execute(
            "DELETE FROM skills_mcp_notes WHERE item_id IN (?1, ?2)",
            params![&item_id, spaced_id],
        )
        .expect("clear stale test notes");
        drop(conn);
        let config_dir = Some(codex_dir.display().to_string());

        save_skills_mcp_note_inner(
            config_dir.clone(),
            "skill".to_string(),
            item_id.clone(),
            "  我的 Skill 备注  ".to_string(),
        )
        .expect("save skill note");
        let state = save_skills_mcp_note_inner(
            config_dir.clone(),
            "mcp".to_string(),
            item_id.clone(),
            "MCP note".to_string(),
        )
        .expect("save MCP note");
        assert_eq!(
            state
                .skills
                .iter()
                .find(|skill| skill.id == item_id)
                .and_then(|skill| skill.note.as_deref()),
            Some("我的 Skill 备注")
        );
        assert_eq!(
            state
                .mcp_servers
                .iter()
                .find(|server| server.id == item_id)
                .and_then(|server| server.note.as_deref()),
            Some("MCP note")
        );
        let state = save_skills_mcp_note_inner(
            config_dir.clone(),
            "mcp".to_string(),
            spaced_id.to_string(),
            "Spaced ID note".to_string(),
        )
        .expect("save note without normalizing the MCP ID");
        assert_eq!(
            state
                .mcp_servers
                .iter()
                .find(|server| server.id == spaced_id)
                .and_then(|server| server.note.as_deref()),
            Some("Spaced ID note")
        );

        let other_config_dir = Some(other_codex_dir.display().to_string());
        let other_state = build_skills_mcp_state_inner(other_config_dir.clone())
            .expect("load other CODEX_HOME state");
        assert!(other_state
            .mcp_servers
            .iter()
            .find(|server| server.id == item_id)
            .is_some_and(|server| server.note.is_none()));
        save_skills_mcp_note_inner(
            other_config_dir,
            "mcp".to_string(),
            item_id.clone(),
            "Other MCP note".to_string(),
        )
        .expect("save note for other CODEX_HOME");
        let original_state = build_skills_mcp_state_inner(config_dir.clone())
            .expect("reload original CODEX_HOME state");
        assert_eq!(
            original_state
                .mcp_servers
                .iter()
                .find(|server| server.id == item_id)
                .and_then(|server| server.note.as_deref()),
            Some("MCP note")
        );

        let state = save_skills_mcp_note_inner(
            config_dir.clone(),
            "skill".to_string(),
            item_id.clone(),
            "  ".to_string(),
        )
        .expect("clear skill note");
        assert!(state
            .skills
            .iter()
            .find(|skill| skill.id == item_id)
            .is_some_and(|skill| skill.note.is_none()));
        assert_eq!(
            state
                .mcp_servers
                .iter()
                .find(|server| server.id == item_id)
                .and_then(|server| server.note.as_deref()),
            Some("MCP note")
        );
        assert!(save_skills_mcp_note_inner(
            config_dir.clone(),
            "unknown".to_string(),
            item_id.clone(),
            "note".to_string(),
        )
        .is_err());
        assert!(save_skills_mcp_note_inner(
            config_dir,
            "mcp".to_string(),
            item_id.clone(),
            "x".repeat(SKILLS_MCP_NOTE_MAX_CHARS + 1),
        )
        .is_err());

        let conn = open_db().expect("reopen app database");
        conn.execute(
            "DELETE FROM skills_mcp_notes WHERE item_id IN (?1, ?2)",
            params![&item_id, spaced_id],
        )
        .expect("remove test notes");
        drop(conn);
        fs::remove_dir_all(&codex_dir).expect("remove test Codex directory");
        fs::remove_dir_all(&other_codex_dir).expect("remove other test Codex directory");
    }
}
