use super::{
    consolidate_legacy_provider_duplicates_on_connection, custom_provider_id,
    experimental_bearer_token_from_doc, list_saved_providers_on_connection,
    normalize_saved_provider, open_store, strip_provider_bearer_tokens,
    upsert_ccswitch_provider_on_connection, ProviderUpsertKind, SavedProvider,
};
use crate::ccswitch::{ccswitch_db_candidates, default_ccswitch_db_path};
use crate::error::{CodexxError, Result};
use crate::sqlite_utils::table_column_set;
use crate::string_value;
use crate::toml_utils::ensure_table;
use rusqlite::{Connection, OpenFlags, TransactionBehavior};
use serde::Serialize;
use serde_json::Value;
use std::collections::HashMap;
use std::path::PathBuf;
use toml_edit::{value, DocumentMut, Item, Table};

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ImportResult {
    imported: usize,
    added: usize,
    updated: usize,
    merged: usize,
    skipped: usize,
    warnings: Vec<String>,
    providers: Vec<SavedProvider>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct OfficialAuthCandidate {
    auth_json: String,
    config_text: Option<String>,
    model: Option<String>,
    source: String,
}

#[derive(Debug, Clone)]
pub(crate) struct CcSwitchCodexRow {
    pub(crate) id: String,
    pub(crate) name: String,
    pub(crate) settings_config: String,
    pub(crate) category: Option<String>,
}

pub(crate) fn is_official_ccswitch_row(row: &CcSwitchCodexRow) -> bool {
    row.id.trim().eq_ignore_ascii_case("codex-official")
        || row
            .category
            .as_deref()
            .is_some_and(|value| value.trim().eq_ignore_ascii_case("official"))
}

pub(crate) fn read_ccswitch_codex_rows(conn: &Connection) -> Result<Vec<CcSwitchCodexRow>> {
    let provider_columns = table_column_set(conn, "providers")?;
    let category_column = if provider_columns.contains("category") {
        "category"
    } else {
        "NULL"
    };
    let provider_query = format!(
        "SELECT id, name, settings_config, {category_column} FROM providers
         WHERE app_type = 'codex' ORDER BY sort_index ASC, created_at ASC"
    );
    let mut stmt = conn
        .prepare(&provider_query)
        .map_err(|e| CodexxError::Database(e.to_string()))?;
    let rows = stmt
        .query_map([], |row| {
            Ok(CcSwitchCodexRow {
                id: row.get::<_, String>(0)?,
                name: row.get::<_, String>(1)?,
                settings_config: row.get::<_, String>(2)?,
                category: row.get::<_, Option<String>>(3)?,
            })
        })
        .map_err(|e| CodexxError::Database(e.to_string()))?;

    let mut result = Vec::new();
    for row in rows {
        result.push(row.map_err(|e| CodexxError::Database(e.to_string()))?);
    }
    Ok(result)
}

#[derive(Debug, Clone)]
pub(crate) struct CcSwitchCodexSection {
    pub(crate) id: String,
    pub(crate) name: Option<String>,
    pub(crate) base_url: String,
    pub(crate) model: Option<String>,
    pub(crate) wire_api: String,
    pub(crate) requires_openai_auth: bool,
    pub(crate) experimental_bearer_token: Option<String>,
    pub(crate) provider_table: Table,
}

fn table_string(table: &Table, key: &str) -> Option<String> {
    table
        .get(key)
        .and_then(|item| item.as_str())
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(ToString::to_string)
}

fn ccswitch_auth_api_key(settings: &Value) -> Option<String> {
    settings
        .get("auth")
        .and_then(|v| v.get("OPENAI_API_KEY"))
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(ToString::to_string)
}

pub(super) fn codex_section_from_table(
    id: &str,
    table: &Table,
    model: Option<String>,
) -> Option<CcSwitchCodexSection> {
    let base_url = table_string(table, "base_url")?
        .trim_end_matches('/')
        .to_string();
    if base_url.is_empty() {
        return None;
    }
    Some(CcSwitchCodexSection {
        id: id.to_string(),
        name: table_string(table, "name"),
        base_url,
        model,
        wire_api: table_string(table, "wire_api").unwrap_or_else(|| "responses".to_string()),
        requires_openai_auth: table
            .get("requires_openai_auth")
            .and_then(|item| item.as_bool())
            .unwrap_or(false),
        experimental_bearer_token: table_string(table, "experimental_bearer_token"),
        provider_table: table.clone(),
    })
}

pub(crate) fn codex_sections_from_config(config_text: &str) -> Vec<CcSwitchCodexSection> {
    let Ok(doc) = config_text.parse::<DocumentMut>() else {
        return Vec::new();
    };
    let model = string_value(&doc, "model");
    let Some(providers) = doc.get("model_providers").and_then(|item| item.as_table()) else {
        return Vec::new();
    };
    providers
        .iter()
        .filter_map(|(id, item)| {
            item.as_table()
                .and_then(|table| codex_section_from_table(id, table, model.clone()))
        })
        .collect()
}

fn select_ccswitch_section_for_row(
    row: &CcSwitchCodexRow,
    settings: &Value,
    global_sections: &HashMap<String, CcSwitchCodexSection>,
) -> Option<CcSwitchCodexSection> {
    let provider_id = custom_provider_id(&row.id);
    let config_text = settings.get("config").and_then(Value::as_str).unwrap_or("");
    let doc = config_text.parse::<DocumentMut>().ok();

    if let Some(doc) = doc.as_ref() {
        let model = string_value(doc, "model");
        let active_provider = string_value(doc, "model_provider");
        let providers = doc.get("model_providers").and_then(|item| item.as_table());

        if let Some(providers) = providers {
            for exact_id in [provider_id.as_str(), row.id.trim()] {
                if let Some(section) = providers
                    .get(exact_id)
                    .and_then(|item| item.as_table())
                    .and_then(|table| codex_section_from_table(exact_id, table, model.clone()))
                {
                    return Some(section);
                }
            }

            if active_provider.as_deref() == Some(row.id.trim())
                || active_provider.as_deref() == Some(provider_id.as_str())
            {
                if let Some(active) = active_provider.as_deref() {
                    if let Some(section) = providers
                        .get(active)
                        .and_then(|item| item.as_table())
                        .and_then(|table| codex_section_from_table(active, table, model.clone()))
                    {
                        return Some(section);
                    }
                }
            }

            // Legacy cc-switch templates store each third-party provider under
            // `[model_providers.custom]` in that row's own complete config.
            if active_provider
                .as_deref()
                .is_none_or(|active| active == "custom")
            {
                if let Some(section) = providers
                    .get("custom")
                    .and_then(|item| item.as_table())
                    .and_then(|table| codex_section_from_table("custom", table, model.clone()))
                {
                    return Some(section);
                }
            }
        }
    }

    for exact_id in [provider_id.as_str(), row.id.trim()] {
        if let Some(section) = global_sections.get(exact_id) {
            return Some(section.clone());
        }
    }

    let doc = doc?;
    let active_provider = string_value(&doc, "model_provider");
    doc.get("base_url")
        .and_then(|item| item.as_str())
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(|base_url| {
            let base_url = base_url.trim_end_matches('/').to_string();
            let token = experimental_bearer_token_from_doc(&doc, active_provider.as_deref());
            let mut provider_table = Table::new();
            provider_table["base_url"] = value(base_url.clone());
            provider_table["wire_api"] = value("responses");
            provider_table["requires_openai_auth"] = value(false);
            if let Some(token) = token.as_deref() {
                provider_table["experimental_bearer_token"] = value(token);
            }
            CcSwitchCodexSection {
                id: provider_id,
                name: None,
                base_url,
                model: string_value(&doc, "model"),
                wire_api: "responses".to_string(),
                requires_openai_auth: false,
                experimental_bearer_token: token,
                provider_table,
            }
        })
}

fn ccswitch_provider_template(
    settings: &Value,
    section: &CcSwitchCodexSection,
    provider_name: &str,
    model: &str,
) -> Option<String> {
    let config_text = settings.get("config").and_then(Value::as_str).unwrap_or("");
    let mut doc = if config_text.trim().is_empty() {
        DocumentMut::new()
    } else {
        config_text.parse::<DocumentMut>().ok()?
    };
    let provider_id = section.id.trim();
    if provider_id.is_empty() {
        return None;
    }

    if string_value(&doc, "model_provider").as_deref() != Some(provider_id) {
        doc["model_provider"] = value(provider_id);
    }
    if string_value(&doc, "model").as_deref() != Some(model) {
        doc["model"] = value(model);
    }
    let providers = ensure_table(doc.as_table_mut(), "model_providers").ok()?;
    if providers
        .get(provider_id)
        .and_then(|item| item.as_table())
        .is_none()
    {
        let mut table = section.provider_table.clone();
        if table_string(&table, "name").is_none() && !provider_name.trim().is_empty() {
            table["name"] = value(provider_name.trim());
        }
        providers.insert(provider_id, Item::Table(table));
    }
    strip_provider_bearer_tokens(&mut doc);
    Some(doc.to_string().trim_end().to_string())
}

pub(crate) fn build_ccswitch_codex_provider(
    row: &CcSwitchCodexRow,
    global_sections: &HashMap<String, CcSwitchCodexSection>,
) -> Option<SavedProvider> {
    let settings: Value = serde_json::from_str(&row.settings_config).ok()?;
    let section = select_ccswitch_section_for_row(row, &settings, global_sections)?;
    let api_key = ccswitch_auth_api_key(&settings).or(section.experimental_bearer_token.clone());
    let provider_name = section
        .name
        .clone()
        .or_else(|| {
            let name = row.name.trim();
            (!name.is_empty()).then(|| name.to_string())
        })
        .unwrap_or_else(|| row.id.clone());
    let model = section.model.clone()?;
    let toml_config = ccswitch_provider_template(&settings, &section, &provider_name, &model)?;
    Some(SavedProvider {
        id: custom_provider_id(&row.id),
        provider_name,
        base_url: section.base_url,
        model,
        api_key,
        toml_config: Some(toml_config),
        wire_api: section.wire_api,
        requires_openai_auth: section.requires_openai_auth,
    })
}

pub(crate) fn import_ccswitch_codex_providers_inner(path: Option<String>) -> Result<ImportResult> {
    let db = path
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .map(PathBuf::from)
        .unwrap_or(default_ccswitch_db_path()?);

    if !db.exists() {
        let candidates = ccswitch_db_candidates()?
            .into_iter()
            .map(|p| p.display().to_string())
            .collect::<Vec<_>>()
            .join("\n- ");
        return Err(CodexxError::Config(format!(
            "cc-switch 数据库不存在: {}\n已检查候选路径:\n- {}",
            db.display(),
            candidates
        )));
    }

    let conn = Connection::open_with_flags(
        &db,
        OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )
    .map_err(|e| {
        CodexxError::Database(format!("打开 cc-switch 数据库失败 {}: {e}", db.display()))
    })?;

    let rows_vec = read_ccswitch_codex_rows(&conn)?;

    let mut global_sections: HashMap<String, CcSwitchCodexSection> = HashMap::new();
    for row in &rows_vec {
        if is_official_ccswitch_row(row) {
            continue;
        }
        let Ok(settings) = serde_json::from_str::<Value>(&row.settings_config) else {
            continue;
        };
        let Some(config_text) = settings.get("config").and_then(Value::as_str) else {
            continue;
        };
        for section in codex_sections_from_config(config_text) {
            if !global_sections.contains_key(&section.id) {
                global_sections.insert(section.id.clone(), section);
            }
        }
    }

    let mut imported = 0usize;
    let mut added = 0usize;
    let mut updated = 0usize;
    let mut merged = 0usize;
    let mut skipped = 0usize;
    let mut warnings = Vec::new();
    let mut local_conn = open_store()?;
    let transaction = local_conn
        .transaction_with_behavior(TransactionBehavior::Immediate)
        .map_err(|e| CodexxError::Database(e.to_string()))?;
    merged += consolidate_legacy_provider_duplicates_on_connection(&transaction)?;

    for row in rows_vec {
        if is_official_ccswitch_row(&row) {
            skipped += 1;
            warnings.push(format!(
                "跳过 {} ({})：官方认证不作为第三方供应商导入",
                row.name, row.id
            ));
            continue;
        }
        match build_ccswitch_codex_provider(&row, &global_sections) {
            Some(provider) => {
                let provider = normalize_saved_provider(provider)?;
                let result =
                    upsert_ccswitch_provider_on_connection(&transaction, provider, row.id.trim())?;
                match result.kind {
                    ProviderUpsertKind::Added => added += 1,
                    ProviderUpsertKind::Updated => updated += 1,
                }
                imported += 1;
            }
            None => {
                skipped += 1;
                warnings.push(format!(
                    "跳过 {} ({})：未找到可用 config/base_url，可能是官方登录或空模板",
                    row.name, row.id
                ));
            }
        }
    }
    merged += consolidate_legacy_provider_duplicates_on_connection(&transaction)?;
    transaction
        .commit()
        .map_err(|e| CodexxError::Database(e.to_string()))?;
    let providers = list_saved_providers_on_connection(&local_conn)?;

    Ok(ImportResult {
        imported,
        added,
        updated,
        merged,
        skipped,
        warnings,
        providers,
    })
}

pub(crate) fn read_ccswitch_official_auth_inner(
    path: Option<String>,
) -> Result<Option<OfficialAuthCandidate>> {
    let db = path
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
        .unwrap_or(default_ccswitch_db_path()?);

    if !db.exists() {
        return Ok(None);
    }

    let conn = Connection::open_with_flags(
        &db,
        OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )
    .map_err(|e| {
        CodexxError::Database(format!("打开 cc-switch 数据库失败 {}: {e}", db.display()))
    })?;

    let provider_columns = table_column_set(&conn, "providers")?;
    let official_filter = if provider_columns.contains("category") {
        "id = 'codex-official' OR category = 'official'"
    } else {
        // Older cc-switch databases predate the category column. The stable
        // codex-official id is still enough to identify the official row.
        "id = 'codex-official'"
    };
    let query = format!(
        "SELECT id, name, settings_config FROM providers
         WHERE app_type = 'codex' AND ({official_filter})
         ORDER BY CASE WHEN id = 'codex-official' THEN 0 ELSE 1 END
         LIMIT 1"
    );
    let mut stmt = conn
        .prepare(&query)
        .map_err(|e| CodexxError::Database(e.to_string()))?;

    let mut rows = stmt
        .query([])
        .map_err(|e| CodexxError::Database(e.to_string()))?;

    let Some(row) = rows
        .next()
        .map_err(|e| CodexxError::Database(e.to_string()))?
    else {
        return Ok(None);
    };

    let id: String = row
        .get(0)
        .map_err(|e| CodexxError::Database(e.to_string()))?;
    let name: String = row
        .get(1)
        .map_err(|e| CodexxError::Database(e.to_string()))?;
    let settings_config: String = row
        .get(2)
        .map_err(|e| CodexxError::Database(e.to_string()))?;
    let settings: Value = serde_json::from_str(&settings_config).map_err(|e| {
        CodexxError::Database(format!("cc-switch official settings JSON 解析失败: {e}"))
    })?;

    let auth = settings
        .get("auth")
        .cloned()
        .filter(|value| value.is_object())
        .ok_or_else(|| {
            CodexxError::Database("cc-switch official provider 缺少 auth object".to_string())
        })?;

    let config_text = settings
        .get("config")
        .and_then(Value::as_str)
        .map(ToString::to_string);
    let model = config_text
        .as_deref()
        .and_then(|text| text.parse::<DocumentMut>().ok())
        .and_then(|doc| string_value(&doc, "model"));

    let auth_json = serde_json::to_string_pretty(&auth)
        .map_err(|e| CodexxError::Database(format!("官方 auth JSON 格式化失败: {e}")))?;

    Ok(Some(OfficialAuthCandidate {
        auth_json,
        config_text,
        model,
        source: format!("cc-switch:{name}:{id}"),
    }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn provider_import_round_trips_complete_config_without_bearer_tokens() {
        let settings_config = json!({
            "auth": {"OPENAI_API_KEY": "sk-from-auth"},
            "config": r#"# keep-imported-comment
model_provider = "custom"
model = "gpt-5.6-sol"
model_reasoning_effort = "xhigh"
service_tier = "priority"
experimental_bearer_token = "sk-top-level"
notify = ["C:\\Users\\Thy\\codex-computer-use.exe", "turn-ended"]

[model_providers.custom]
name = "Sky2api"
base_url = "https://proxy.example.com/v1/"
wire_api = "responses"
requires_openai_auth = false
experimental_bearer_token = "sk-from-config"
request_max_retries = 7

[model_providers.other]
name = "Other provider"
base_url = "https://other.example.com/v1"
experimental_bearer_token = "sk-other"

[projects."/work/project"]
trust_level = "trusted"

[desktop]
followUpQueueMode = "queue"
localeOverride = "zh-CN"

[windows]
sandbox = "elevated"
shell_path = 'D:\Program Files\PowerShell\7\pwsh.exe'

[plugins."browser@openai-bundled"]
enabled = true

[features]
js_repl = false

[shell_environment_policy.set]
CODEX_HOME = 'C:\Users\Thy\.codex'

[mcp_servers.docs]
command = "docs-server"
"#,
        })
        .to_string();
        let row = CcSwitchCodexRow {
            id: "magicai-123".to_string(),
            name: "  Sky2_free  ".to_string(),
            settings_config,
            category: None,
        };

        let provider =
            build_ccswitch_codex_provider(&row, &HashMap::new()).expect("build cc-switch provider");
        assert_eq!(provider.id, "magicai-123");
        assert_eq!(provider.provider_name, "Sky2api");
        assert_eq!(provider.base_url, "https://proxy.example.com/v1");
        assert_eq!(provider.model, "gpt-5.6-sol");
        assert_eq!(provider.api_key.as_deref(), Some("sk-from-auth"));
        assert_eq!(provider.wire_api, "responses");
        assert!(!provider.requires_openai_auth);

        let text = provider.toml_config.expect("complete provider TOML");
        let doc = text.parse::<DocumentMut>().expect("parse provider TOML");
        assert!(text.contains("# keep-imported-comment"));
        assert_eq!(doc["model_provider"].as_str(), Some("custom"));
        assert_eq!(doc["model_reasoning_effort"].as_str(), Some("xhigh"));
        assert_eq!(doc["service_tier"].as_str(), Some("priority"));
        assert_eq!(doc["notify"].as_array().map(|values| values.len()), Some(2));
        assert_eq!(
            doc["model_providers"]["custom"]["name"].as_str(),
            Some("Sky2api")
        );
        assert_eq!(
            doc["model_providers"]["custom"]["base_url"].as_str(),
            Some("https://proxy.example.com/v1/")
        );
        assert_eq!(
            doc["model_providers"]["custom"]["request_max_retries"].as_integer(),
            Some(7)
        );
        assert_eq!(
            doc["model_providers"]["other"]["base_url"].as_str(),
            Some("https://other.example.com/v1")
        );
        assert_eq!(
            doc["projects"]["/work/project"]["trust_level"].as_str(),
            Some("trusted")
        );
        assert_eq!(doc["desktop"]["followUpQueueMode"].as_str(), Some("queue"));
        assert_eq!(doc["desktop"]["localeOverride"].as_str(), Some("zh-CN"));
        assert_eq!(doc["windows"]["sandbox"].as_str(), Some("elevated"));
        assert_eq!(
            doc["windows"]["shell_path"].as_str(),
            Some(r"D:\Program Files\PowerShell\7\pwsh.exe")
        );
        assert_eq!(
            doc["plugins"]["browser@openai-bundled"]["enabled"].as_bool(),
            Some(true)
        );
        assert_eq!(doc["features"]["js_repl"].as_bool(), Some(false));
        assert_eq!(
            doc["shell_environment_policy"]["set"]["CODEX_HOME"].as_str(),
            Some(r"C:\Users\Thy\.codex")
        );
        assert_eq!(
            doc["mcp_servers"]["docs"]["command"].as_str(),
            Some("docs-server")
        );
        assert!(doc.get("experimental_bearer_token").is_none());
        assert!(doc["model_providers"]
            .as_table()
            .expect("model providers table")
            .iter()
            .all(|(_, item)| item
                .as_table()
                .is_none_or(|table| table.get("experimental_bearer_token").is_none())));
        assert!(!text.contains("sk-from-auth"));
        assert!(!text.contains("experimental_bearer_token"));
    }

    #[test]
    fn complete_row_without_a_model_is_not_silently_downgraded() {
        let row = CcSwitchCodexRow {
            id: "missing-model".to_string(),
            name: "Database label".to_string(),
            settings_config: json!({
                "auth": {"OPENAI_API_KEY": "sk-test"},
                "config": r#"model_provider = "custom"

[model_providers.custom]
name = "TOML label"
base_url = "https://proxy.example.com/v1"
wire_api = "responses"
requires_openai_auth = false
"#,
            })
            .to_string(),
            category: None,
        };

        assert!(build_ccswitch_codex_provider(&row, &HashMap::new()).is_none());
    }

    #[test]
    fn malformed_row_config_does_not_create_a_sparse_provider() {
        let section = codex_sections_from_config(
            r#"model_provider = "broken-row"
model = "gpt-5.5"

[model_providers.broken-row]
name = "Recovered only from another row"
base_url = "https://proxy.example.com/v1"
wire_api = "responses"
requires_openai_auth = false
"#,
        )
        .into_iter()
        .next()
        .expect("global provider section");
        let mut global_sections = HashMap::new();
        global_sections.insert(section.id.clone(), section);
        let row = CcSwitchCodexRow {
            id: "broken-row".to_string(),
            name: "Broken row".to_string(),
            settings_config: json!({
                "auth": {"OPENAI_API_KEY": "sk-must-not-import"},
                "config": "model = ["
            })
            .to_string(),
            category: None,
        };

        assert!(build_ccswitch_codex_provider(&row, &global_sections).is_none());
    }
}
