use reqwest::blocking::Client;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::fs;
#[cfg(test)]
use std::io::Write;
use std::io::{ErrorKind, Read};
use std::path::Path;
use std::path::PathBuf;
use std::process::{Child, Command, Output, Stdio};
use std::sync::{Mutex, OnceLock};
use std::time::Duration;

use crate::backups::create_backup;
use crate::constants::{AGENTS_MANAGED_BEGIN, AGENTS_MANAGED_END, AGENTS_TEMPLATE_PREFIX};
use crate::error::{CodexxError, Result};
use crate::file_io::{atomic_write, ensure_directory, parse_toml_document};
use crate::live_config::{
    acquire_live_config_lock, apply_file_change, atomic_write_if_unchanged, read_file_snapshot,
    rollback_file_changes,
};
use crate::paths::app_home;
use crate::platform::program_command;
use crate::prompts::resolve_instruction_path;
use crate::prompts::{
    agents_path, install_managed_agents_block_in_content, managed_agents_bounds,
    remove_managed_agents_block_from_content,
};
use crate::{auth_path, config_path, resolve_codex_dir, string_value};
use toml_edit::{value, Item, Table};

static GATEWAY_CHILD: OnceLock<Mutex<Option<Child>>> = OnceLock::new();
static WATCHDOG_CHILD: OnceLock<Mutex<Option<Child>>> = OnceLock::new();
const MAX_GATEWAY_STATE_FILE_BYTES: u64 = 4 * 1024 * 1024;

fn child_slot() -> &'static Mutex<Option<Child>> {
    GATEWAY_CHILD.get_or_init(|| Mutex::new(None))
}

fn watchdog_slot() -> &'static Mutex<Option<Child>> {
    WATCHDOG_CHILD.get_or_init(|| Mutex::new(None))
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct GatewayStartInput {
    pub listen_host: String,
    pub listen_port: u16,
    pub upstream: String,
    #[serde(default)]
    pub config_dir: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct GatewayRequestInput {
    pub listen_port: u16,
    pub method: String,
    pub path: String,
    pub body: Option<Value>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct GatewayProcessState {
    pub running: bool,
    pub managed_by_codex_x: bool,
    pub codex_route_active: bool,
    pub listen_port: u16,
    pub process_id: Option<u32>,
    pub state: Option<Value>,
    pub error: Option<String>,
    pub watchdog_running: bool,
    pub watchdog_autostart: bool,
    pub watchdog_desired: bool,
    pub watchdog_runtime: String,
    pub degraded: bool,
}

fn local_client() -> Result<Client> {
    crate::remote::ensure_crypto_provider();
    Client::builder()
        .no_proxy()
        .connect_timeout(Duration::from_millis(700))
        .timeout(Duration::from_secs(4))
        .build()
        .map_err(|error| CodexxError::Config(format!("CONTROL_API_UNAVAILABLE: {error}")))
}

fn control_url(port: u16, path: &str) -> Result<String> {
    if port == 0 {
        return Err(CodexxError::Config(
            "GATEWAY_INVALID_LISTEN: 端口必须在 1-65535 范围内".to_string(),
        ));
    }
    if !path.starts_with('/') || path.contains("..") || path.contains("//") {
        return Err(CodexxError::Config(
            "CONTROL_API_INVALID_REQUEST: 控制接口路径无效".to_string(),
        ));
    }
    Ok(format!("http://127.0.0.1:{port}{path}"))
}

fn request_control(input: &GatewayRequestInput) -> Result<Value> {
    let method = reqwest::Method::from_bytes(input.method.trim().to_ascii_uppercase().as_bytes())
        .map_err(|_| {
        CodexxError::Config("CONTROL_API_INVALID_REQUEST: HTTP 方法无效".to_string())
    })?;
    if !matches!(
        method,
        reqwest::Method::GET | reqwest::Method::POST | reqwest::Method::PUT
    ) {
        return Err(CodexxError::Config(
            "CONTROL_API_INVALID_REQUEST: 只允许 GET、POST 或 PUT".to_string(),
        ));
    }
    let response = local_client()?
        .request(method, control_url(input.listen_port, &input.path)?)
        .header(reqwest::header::ACCEPT, "application/json")
        .json(&input.body.clone().unwrap_or_else(|| json!({})))
        .send()
        .map_err(|error| CodexxError::Config(format!("CONTROL_API_UNAVAILABLE: {error}")))?;
    let status = response.status();
    let value = response
        .json::<Value>()
        .map_err(|error| CodexxError::Config(format!("CONTROL_API_INVALID_RESPONSE: {error}")))?;
    if !status.is_success() {
        let code = value
            .pointer("/error/code")
            .and_then(Value::as_str)
            .unwrap_or("CONTROL_API_FAILED");
        let message = value
            .pointer("/error/message")
            .and_then(Value::as_str)
            .unwrap_or("网关控制接口返回失败");
        return Err(CodexxError::Config(format!("{code}: {message}")));
    }
    Ok(value)
}

fn gateway_script() -> Result<PathBuf> {
    if let Some(path) = std::env::var_os("CODEX_X_GATEWAY_SCRIPT").map(PathBuf::from) {
        if path.is_file() {
            return Ok(path);
        }
    }
    if let Some(profile) = std::env::var_os("USERPROFILE").map(PathBuf::from) {
        let personal = profile
            .join(".codex-x")
            .join("personal-gateway")
            .join("codex_responses_repair_gateway.py");
        if personal.is_file() {
            return Ok(personal);
        }
    }
    Err(CodexxError::Config(
        "GATEWAY_PROCESS_START_FAILED: 找不到外部本地网关脚本，请设置 CODEX_X_GATEWAY_SCRIPT"
            .to_string(),
    ))
}

fn gateway_health(port: u16) -> Result<Value> {
    request_control(&GatewayRequestInput {
        listen_port: port,
        method: "GET".to_string(),
        path: "/state".to_string(),
        body: None,
    })
}

fn gateway_process_id(state: &Value) -> Option<u32> {
    state
        .get("process_id")
        .and_then(Value::as_u64)
        .and_then(|value| u32::try_from(value).ok())
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct GatewayModeMeta {
    version: u8,
    desired_mode: String,
    codex_dir: String,
    listen_port: u16,
    projected_config_sha256: String,
    original_config_file: String,
    original_auth_file: String,
    original_instruction_file: Option<String>,
    original_instruction_backup: Option<String>,
    #[serde(default)]
    original_agents_file: Option<String>,
    #[serde(default)]
    original_agents_backup: Option<String>,
    #[serde(default)]
    instruction_mode: Option<String>,
}

fn persistently_owned_gateway(meta: Option<&GatewayModeMeta>, port: u16, state: &Value) -> bool {
    meta.is_some_and(|meta| {
        meta.desired_mode == "gateway"
            && meta.listen_port == port
            && state.get("state").and_then(Value::as_str) == Some("gateway")
            && state
                .get("listen")
                .and_then(Value::as_str)
                .is_some_and(|listen| {
                    listen == format!("127.0.0.1:{port}") || listen == format!("localhost:{port}")
                })
            && gateway_process_id(state).is_some()
    })
}

fn codex_route_active(meta: Option<&GatewayModeMeta>, port: u16) -> bool {
    let Some(meta) = meta else {
        return false;
    };
    if meta.desired_mode != "gateway" || meta.listen_port != port {
        return false;
    }
    let config = config_path(Path::new(&meta.codex_dir));
    let Ok(bytes) = fs::read(&config) else {
        return false;
    };
    let Ok(provider) = provider_from_config(&config, &bytes, "") else {
        return false;
    };
    let Some(base_url) = provider.get("base_url").and_then(Value::as_str) else {
        return false;
    };
    let expected = projected_base_url(port);
    base_url.trim().trim_end_matches('/') == expected.trim_end_matches('/')
}

fn mode_dir() -> Result<PathBuf> {
    Ok(app_home()?.join("gateway-mode"))
}

fn mode_meta_path() -> Result<PathBuf> {
    Ok(mode_dir()?.join("state.json"))
}

fn degraded_path() -> Result<PathBuf> {
    Ok(mode_dir()?.join("degraded.json"))
}

pub(crate) fn mark_degraded_state(error: &str) {
    if let Ok(path) = degraded_path() {
        if let Ok(dir) = mode_dir() {
            let _ = ensure_directory(&dir);
        }
        if let Ok(bytes) = serde_json::to_vec_pretty(&json!({"error": error})) {
            let _ = atomic_write(&path, &bytes);
        }
    }
}

fn clear_degraded_state() {
    if let Ok(path) = degraded_path() {
        let _ = fs::remove_file(path);
    }
}

fn degraded_state() -> (bool, Option<String>) {
    let Ok(path) = degraded_path() else {
        return (false, None);
    };
    let Ok(bytes) = fs::read(path) else {
        return (false, None);
    };
    let message = serde_json::from_slice::<Value>(&bytes)
        .ok()
        .and_then(|value| {
            value
                .get("error")
                .and_then(Value::as_str)
                .map(str::to_string)
        });
    (true, message)
}

fn classify_unavailable_gateway_state(
    managed: bool,
    gateway_mode_expected: bool,
    persisted_degraded: bool,
    persisted_error: Option<String>,
    health_error: String,
) -> (bool, Option<String>) {
    let degraded = managed || gateway_mode_expected || persisted_degraded;
    let error = persisted_error.or_else(|| degraded.then_some(health_error));
    (degraded, error)
}

fn read_bounded_file(path: &Path, max_bytes: u64) -> Result<Option<Vec<u8>>> {
    let file = match fs::File::open(path) {
        Ok(file) => file,
        Err(error) if error.kind() == ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(crate::file_io::io_err(path, error)),
    };
    let metadata = file
        .metadata()
        .map_err(|error| crate::file_io::io_err(path, error))?;
    if !metadata.is_file() {
        return Err(CodexxError::Config(format!(
            "GATEWAY_STATE_UNAVAILABLE: 状态路径不是普通文件: {}",
            path.display()
        )));
    }
    if metadata.len() > max_bytes {
        return Err(CodexxError::Config(format!(
            "GATEWAY_STATE_TOO_LARGE: 状态文件超过 {} 字节: {}",
            max_bytes,
            path.display()
        )));
    }
    let mut bytes = Vec::new();
    file.take(max_bytes.saturating_add(1))
        .read_to_end(&mut bytes)
        .map_err(|error| crate::file_io::io_err(path, error))?;
    if bytes.len() as u64 > max_bytes {
        return Err(CodexxError::Config(format!(
            "GATEWAY_STATE_TOO_LARGE: 状态文件超过 {} 字节: {}",
            max_bytes,
            path.display()
        )));
    }
    Ok(Some(bytes))
}

fn read_optional_runtime_state(path: &Path) -> Option<Value> {
    match read_bounded_file(path, MAX_GATEWAY_STATE_FILE_BYTES) {
        Ok(Some(bytes)) => serde_json::from_slice::<Value>(&bytes).ok(),
        Ok(None) => None,
        Err(error) => {
            eprintln!(
                "gateway runtime state ignored for {}: {error}",
                path.display()
            );
            None
        }
    }
}

fn projected_base_url(port: u16) -> String {
    format!("http://127.0.0.1:{port}/v1")
}

fn sha256_hex(bytes: &[u8]) -> String {
    let mut digest = Sha256::new();
    digest.update(bytes);
    format!("{:x}", digest.finalize())
}

fn project_config(
    path: &Path,
    original: &[u8],
    port: u16,
    remove_instruction: bool,
) -> Result<Vec<u8>> {
    let text = std::str::from_utf8(original).map_err(|error| {
        CodexxError::Config(format!(
            "LIVE_CONFIG_INVALID: config.toml 不是有效 UTF-8: {error}"
        ))
    })?;
    let mut document = parse_toml_document(path, text)
        .map_err(|error| CodexxError::Config(format!("LIVE_CONFIG_INVALID: {error}")))?;
    let local = projected_base_url(port);
    document["base_url"] = value(local.clone());
    if remove_instruction {
        document.as_table_mut().remove("model_instructions_file");
    }
    if let Some(provider_id) = string_value(&document, "model_provider") {
        if let Some(providers) = document
            .get_mut("model_providers")
            .and_then(|item| item.as_table_mut())
        {
            if let Some(provider) = providers
                .get_mut(provider_id.as_str())
                .and_then(|item| item.as_table_mut())
            {
                provider["base_url"] = value(local);
            }
        }
    }
    Ok(document.to_string().into_bytes())
}

fn instruction_from_config(
    codex_dir: &Path,
    path: &Path,
    original: &[u8],
) -> Result<Option<(PathBuf, String, Vec<u8>)>> {
    let text = std::str::from_utf8(original).map_err(|error| {
        CodexxError::Config(format!(
            "LIVE_CONFIG_INVALID: config.toml 不是有效 UTF-8: {error}"
        ))
    })?;
    let document = parse_toml_document(path, text)
        .map_err(|error| CodexxError::Config(format!("LIVE_CONFIG_INVALID: {error}")))?;
    let Some(instruction_file) = string_value(&document, "model_instructions_file") else {
        return Ok(None);
    };
    let instruction_path = resolve_instruction_path(codex_dir, &instruction_file);
    let bytes = fs::read(&instruction_path).map_err(|error| {
        CodexxError::Config(format!(
            "LIVE_CONFIG_INVALID: 提示词文件 {} 不可读: {error}",
            instruction_path.display()
        ))
    })?;
    let content = String::from_utf8(bytes.clone()).map_err(|error| {
        CodexxError::Config(format!(
            "LIVE_CONFIG_INVALID: 提示词文件 {} 不是有效 UTF-8: {error}",
            instruction_path.display()
        ))
    })?;
    if content.trim().is_empty() {
        return Err(CodexxError::Config(format!(
            "LIVE_CONFIG_INVALID: 提示词文件 {} 为空",
            instruction_path.display()
        )));
    }
    Ok(Some((instruction_path, content, bytes)))
}

fn instruction_from_agents(
    codex_dir: &Path,
    original: Option<&[u8]>,
) -> Result<Option<(PathBuf, String, Vec<u8>, Vec<u8>)>> {
    let Some(bytes) = original else {
        return Ok(None);
    };
    let text = std::str::from_utf8(bytes).map_err(|error| {
        CodexxError::Config(format!(
            "LIVE_CONFIG_INVALID: AGENTS.md 不是有效 UTF-8: {error}"
        ))
    })?;
    let Some((start, end)) = managed_agents_bounds(text)? else {
        return Ok(None);
    };
    let block = &text[start..end];
    let content = block
        .lines()
        .filter(|line| {
            !line.contains(AGENTS_MANAGED_BEGIN)
                && !line.contains(AGENTS_MANAGED_END)
                && !line.trim_start().starts_with(AGENTS_TEMPLATE_PREFIX)
        })
        .collect::<Vec<_>>()
        .join("\n")
        .trim()
        .to_string();
    if content.is_empty() {
        return Err(CodexxError::Config(
            "LIVE_CONFIG_INVALID: AGENTS.md 受管提示词为空".to_string(),
        ));
    }
    let (without_managed, _) = remove_managed_agents_block_from_content(text)?;
    Ok(Some((
        agents_path(codex_dir),
        content,
        bytes.to_vec(),
        without_managed.into_bytes(),
    )))
}

fn project_runtime_provider(path: &Path, original: &[u8], provider: &Value) -> Result<Vec<u8>> {
    let text = std::str::from_utf8(original).map_err(|error| {
        CodexxError::Config(format!(
            "DIRECT_CONFIG_INVALID_AFTER_WRITE: config.toml 不是有效 UTF-8: {error}"
        ))
    })?;
    let mut document = parse_toml_document(path, text).map_err(|error| {
        CodexxError::Config(format!("DIRECT_CONFIG_INVALID_AFTER_WRITE: {error}"))
    })?;
    let provider_id = provider
        .get("provider_id")
        .and_then(Value::as_str)
        .filter(|v| !v.trim().is_empty())
        .unwrap_or("default")
        .trim();
    let base_url = provider
        .get("base_url")
        .and_then(Value::as_str)
        .filter(|v| !v.trim().is_empty())
        .ok_or_else(|| {
            CodexxError::Config(
                "DIRECT_CONFIG_INVALID_AFTER_WRITE: Provider 缺少 base_url".to_string(),
            )
        })?;
    document["model_provider"] = value(provider_id);
    document["base_url"] = value(base_url.trim());
    let providers = document
        .as_table_mut()
        .entry("model_providers")
        .or_insert_with(|| Item::Table(Table::new()));
    let providers = providers.as_table_mut().ok_or_else(|| {
        CodexxError::Config("DIRECT_CONFIG_INVALID_AFTER_WRITE: model_providers 不是表".to_string())
    })?;
    let table = providers
        .entry(provider_id)
        .or_insert_with(|| Item::Table(Table::new()));
    let table = table.as_table_mut().ok_or_else(|| {
        CodexxError::Config("DIRECT_CONFIG_INVALID_AFTER_WRITE: Provider 配置不是表".to_string())
    })?;
    table["base_url"] = value(base_url.trim());
    if let Some(name) = provider
        .get("provider_name")
        .and_then(Value::as_str)
        .filter(|v| !v.trim().is_empty())
    {
        table["name"] = value(name.trim());
    }
    if let Some(wire_api) = provider
        .get("wire_api")
        .and_then(Value::as_str)
        .filter(|v| !v.trim().is_empty())
    {
        table["wire_api"] = value(wire_api.trim());
    }
    if let Some(requires_openai_auth) = provider
        .get("requires_openai_auth")
        .and_then(Value::as_bool)
    {
        table["requires_openai_auth"] = value(requires_openai_auth);
    }
    if let Some(model) = provider
        .get("model")
        .and_then(Value::as_str)
        .filter(|v| !v.trim().is_empty())
    {
        document["model"] = value(model.trim());
    }
    Ok(document.to_string().into_bytes())
}

fn runtime_auth_bytes(provider: &Value, current: Option<&[u8]>) -> Result<Option<Vec<u8>>> {
    let key = provider
        .get("api_key")
        .or_else(|| provider.get("key"))
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|v| !v.is_empty());
    let Some(current) = current else {
        return Ok(key.map(|value| {
            serde_json::to_vec_pretty(&json!({"OPENAI_API_KEY": value})).unwrap_or_default()
        }));
    };
    let mut document = serde_json::from_slice::<Value>(current).map_err(|error| {
        CodexxError::Config(format!(
            "DIRECT_CONFIG_INVALID_AFTER_WRITE: auth.json 不是有效 JSON: {error}"
        ))
    })?;
    let object = document.as_object_mut().ok_or_else(|| {
        CodexxError::Config(
            "DIRECT_CONFIG_INVALID_AFTER_WRITE: auth.json 顶层必须是对象".to_string(),
        )
    })?;
    if let Some(value) = key {
        object.insert(
            "OPENAI_API_KEY".to_string(),
            Value::String(value.to_string()),
        );
    } else if provider
        .get("requires_openai_auth")
        .and_then(Value::as_bool)
        == Some(false)
        || provider
            .get("provider_id")
            .and_then(Value::as_str)
            .is_some_and(|id| id == "openai-official" || id == "openai")
    {
        object.remove("OPENAI_API_KEY");
    }
    serde_json::to_vec_pretty(&document)
        .map(Some)
        .map_err(|error| {
            CodexxError::Config(format!(
                "DIRECT_CONFIG_INVALID_AFTER_WRITE: auth.json: {error}"
            ))
        })
}

fn provider_from_config(path: &Path, original: &[u8], fallback: &str) -> Result<Value> {
    let text = std::str::from_utf8(original).map_err(|error| {
        CodexxError::Config(format!(
            "LIVE_CONFIG_INVALID: config.toml 不是有效 UTF-8: {error}"
        ))
    })?;
    let document = parse_toml_document(path, text)
        .map_err(|error| CodexxError::Config(format!("LIVE_CONFIG_INVALID: {error}")))?;
    let provider_id =
        string_value(&document, "model_provider").unwrap_or_else(|| "default".to_string());
    let model = string_value(&document, "model");
    let top_base_url = string_value(&document, "base_url");
    let base_url = document
        .get("model_providers")
        .and_then(|item| item.as_table())
        .and_then(|providers| providers.get(provider_id.as_str()))
        .and_then(|item| item.get("base_url"))
        .and_then(|item| item.as_str())
        .or(top_base_url.as_deref())
        .unwrap_or(fallback)
        .to_string();
    let provider_name = document
        .get("model_providers")
        .and_then(|item| item.as_table())
        .and_then(|providers| providers.get(provider_id.as_str()))
        .and_then(|item| item.get("name"))
        .and_then(|item| item.as_str())
        .unwrap_or(provider_id.as_str());
    Ok(
        json!({"provider_id": provider_id, "provider_name": provider_name, "base_url": base_url, "model": model, "wire_api": "responses"}),
    )
}

fn read_mode_meta() -> Result<Option<GatewayModeMeta>> {
    let path = mode_meta_path()?;
    let Some(bytes) = read_bounded_file(&path, MAX_GATEWAY_STATE_FILE_BYTES)? else {
        return Ok(None);
    };
    serde_json::from_slice(&bytes).map(Some).map_err(|error| {
        CodexxError::Config(format!(
            "DISABLE_STATE_UNAVAILABLE: 网关状态文件无效: {error}"
        ))
    })
}

pub(crate) fn ensure_direct_config_write_allowed(config_dir: Option<&str>) -> Result<()> {
    let Some(meta) = read_mode_meta()? else {
        return Ok(());
    };
    if meta.desired_mode != "gateway" {
        return Ok(());
    }
    let requested = resolve_codex_dir(config_dir.map(ToString::to_string))?;
    let managed = PathBuf::from(&meta.codex_dir);
    let same_directory = requested == managed
        || (requested.canonicalize().ok().is_some()
            && requested.canonicalize().ok() == managed.canonicalize().ok());
    if same_directory {
        return Err(CodexxError::Config(
            "GATEWAY_CONFIG_WRITE_BLOCKED: 网关模式正在托管 Provider、认证和提示词配置，请通过网关运行时修改或先停止网关"
                .to_string(),
        ));
    }
    Ok(())
}

fn write_mode_meta(meta: &GatewayModeMeta) -> Result<()> {
    let dir = mode_dir()?;
    ensure_directory(&dir)?;
    let bytes = serde_json::to_vec_pretty(meta).map_err(|error| {
        CodexxError::Config(format!(
            "GATEWAY_UNKNOWN_FAILURE: 无法序列化网关状态: {error}"
        ))
    })?;
    atomic_write(&mode_meta_path()?, &bytes)
}

fn set_watchdog_intent(
    desired_mode: &str,
    watchdog_desired: bool,
    port: u16,
    upstream: &str,
) -> Result<PathBuf> {
    let path = mode_dir()?.join("watchdog-intent.json");
    ensure_directory(&mode_dir()?)?;
    let body = json!({"desired_mode": desired_mode, "watchdog_desired": watchdog_desired, "listen": format!("127.0.0.1:{port}"), "upstream": upstream, "state_file": mode_dir()?.join("runtime-state.json")});
    let bytes = serde_json::to_vec_pretty(&body)
        .map_err(|error| CodexxError::Config(format!("GATEWAY_UNKNOWN_FAILURE: {error}")))?;
    atomic_write(&path, &bytes)?;
    Ok(path)
}

fn xml_escape(value: &str) -> String {
    value
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&apos;")
}

fn current_task_user_xml() -> String {
    let domain = std::env::var("USERDOMAIN")
        .ok()
        .filter(|value| !value.trim().is_empty());
    let username = std::env::var("USERNAME")
        .ok()
        .filter(|value| !value.trim().is_empty());
    match (domain, username) {
        (Some(domain), Some(username)) => format!(
            "<UserId>{}</UserId>",
            xml_escape(&format!("{domain}\\{username}"))
        ),
        _ => String::new(),
    }
}

fn watchdog_task_xml_for_name(
    task_name: &str,
    input: &GatewayStartInput,
    intent: &Path,
    script: &Path,
) -> String {
    let python = std::env::var("CODEX_X_PYTHON").unwrap_or_else(|_| "python".to_string());
    let arguments = format!(
        "-NoProfile -NonInteractive -WindowStyle Hidden -ExecutionPolicy Bypass -File \"{}\" -Listen \"127.0.0.1:{}\" -Upstream \"{}\" -Python \"{}\" -StateFile \"{}\"",
        script.display(),
        input.listen_port,
        input.upstream.trim(),
        python,
        intent.display()
    );
    let user = current_task_user_xml();
    format!(
        "<?xml version=\"1.0\" encoding=\"UTF-16\"?>\n\
         <Task version=\"1.3\" xmlns=\"http://schemas.microsoft.com/windows/2004/02/mit/task\">\n\
           <RegistrationInfo><URI>\\{}</URI></RegistrationInfo>\n\
           <Principals><Principal id=\"Author\">{}<LogonType>InteractiveToken</LogonType><RunLevel>LeastPrivilege</RunLevel></Principal></Principals>\n\
           <Settings>\n\
             <DisallowStartIfOnBatteries>false</DisallowStartIfOnBatteries>\n\
             <StopIfGoingOnBatteries>false</StopIfGoingOnBatteries>\n\
             <MultipleInstancesPolicy>IgnoreNew</MultipleInstancesPolicy>\n\
             <RestartOnFailure><Count>999</Count><Interval>PT1M</Interval></RestartOnFailure>\n\
             <StartWhenAvailable>true</StartWhenAvailable>\n\
             <ExecutionTimeLimit>PT0S</ExecutionTimeLimit>\n\
             <UseUnifiedSchedulingEngine>true</UseUnifiedSchedulingEngine>\n\
             <Hidden>true</Hidden>\n\
             <Enabled>true</Enabled>\n\
           </Settings>\n\
           <Triggers><LogonTrigger>{}<Enabled>true</Enabled></LogonTrigger></Triggers>\n\
           <Actions Context=\"Author\"><Exec><Command>powershell.exe</Command><Arguments>{}</Arguments></Exec></Actions>\n\
         </Task>",
        xml_escape(task_name),
        user,
        user,
        xml_escape(&arguments)
    )
}

fn watchdog_task_xml(input: &GatewayStartInput, intent: &Path, script: &Path) -> String {
    watchdog_task_xml_for_name(WATCHDOG_TASK_NAME, input, intent, script)
}

fn utf16le_with_bom(value: &str) -> Vec<u8> {
    let mut bytes = Vec::with_capacity(value.len() * 2 + 2);
    bytes.extend_from_slice(&[0xff, 0xfe]);
    for unit in value.encode_utf16() {
        bytes.extend_from_slice(&unit.to_le_bytes());
    }
    bytes
}

#[derive(Debug)]
struct GatewayStartStateSnapshot {
    files: Vec<(PathBuf, Option<Vec<u8>>)>,
}

impl GatewayStartStateSnapshot {
    fn capture(directory: &Path) -> Result<Self> {
        let mut files = Vec::new();
        for name in [
            "state.json",
            "runtime-state.json",
            "watchdog-intent.json",
            "original-config.toml",
            "original-auth.json",
            "original-instruction.md",
            "original-agents.md",
        ] {
            let path = directory.join(name);
            let snapshot = if name == "runtime-state.json" {
                read_bounded_file(&path, MAX_GATEWAY_STATE_FILE_BYTES)?
            } else {
                read_file_snapshot(&path)?
            };
            files.push((path.clone(), snapshot));
        }
        Ok(Self { files })
    }

    fn restore(&self) {
        for (path, snapshot) in &self.files {
            match snapshot {
                Some(bytes) => {
                    let _ = atomic_write(path, bytes);
                }
                None => match fs::remove_file(path) {
                    Ok(()) => {}
                    Err(error) if error.kind() == ErrorKind::NotFound => {}
                    Err(_) => {}
                },
            }
        }
    }
}

fn restore_agents_after_failed_start(initial_append: Option<&(PathBuf, String, Vec<u8>, Vec<u8>)>) {
    if let Some((path, _, original, without_managed)) = initial_append {
        let _ =
            atomic_write_if_unchanged(path, Some(without_managed.as_slice()), original.as_slice());
    }
}

#[cfg(target_os = "windows")]
fn decode_command_bytes(bytes: &[u8]) -> String {
    if bytes.starts_with(&[0xff, 0xfe]) || (bytes.len() >= 4 && bytes[1] == 0 && bytes[3] == 0) {
        let offset = usize::from(bytes.starts_with(&[0xff, 0xfe])) * 2;
        let units = bytes[offset..]
            .chunks_exact(2)
            .map(|chunk| u16::from_le_bytes([chunk[0], chunk[1]]))
            .collect::<Vec<_>>();
        return String::from_utf16_lossy(&units);
    }
    if let Ok(value) = std::str::from_utf8(bytes) {
        return value.to_string();
    }
    if bytes.len() > i32::MAX as usize {
        return String::from_utf8_lossy(bytes).into_owned();
    }
    #[link(name = "kernel32")]
    extern "system" {
        fn GetOEMCP() -> u32;
        fn MultiByteToWideChar(
            code_page: u32,
            flags: u32,
            input: *const u8,
            input_len: i32,
            output: *mut u16,
            output_len: i32,
        ) -> i32;
    }
    unsafe {
        let code_page = GetOEMCP();
        let required = MultiByteToWideChar(
            code_page,
            0,
            bytes.as_ptr(),
            bytes.len() as i32,
            std::ptr::null_mut(),
            0,
        );
        if required > 0 {
            let mut units = vec![0_u16; required as usize];
            let written = MultiByteToWideChar(
                code_page,
                0,
                bytes.as_ptr(),
                bytes.len() as i32,
                units.as_mut_ptr(),
                required,
            );
            if written > 0 {
                units.truncate(written as usize);
                return String::from_utf16_lossy(&units);
            }
        }
    }
    String::from_utf8_lossy(bytes).into_owned()
}

#[cfg(not(target_os = "windows"))]
fn decode_command_bytes(bytes: &[u8]) -> String {
    String::from_utf8_lossy(bytes).into_owned()
}

fn command_output_summary(output: &Output) -> String {
    let stderr = decode_command_bytes(&output.stderr).trim().to_string();
    let stdout = decode_command_bytes(&output.stdout).trim().to_string();
    let detail = match (stderr.is_empty(), stdout.is_empty()) {
        (false, false) => format!("stderr: {stderr}; stdout: {stdout}"),
        (false, true) => format!("stderr: {stderr}"),
        (true, false) => format!("stdout: {stdout}"),
        (true, true) => output.status.code().map_or_else(
            || "进程未返回可用错误信息".to_string(),
            |code| format!("退出码: {code}"),
        ),
    };
    detail.chars().take(1200).collect()
}

fn create_watchdog_task(task_name: &str, xml_path: &Path) -> Result<()> {
    let xml_path = xml_path.to_string_lossy();
    let output = program_command(
        Path::new("schtasks.exe"),
        &["/Create", "/TN", task_name, "/XML", xml_path.as_ref(), "/F"],
    )
    .output()
    .map_err(|error| CodexxError::Config(format!("WATCHDOG_TASK_START_FAILED: {error}")))?;
    if !output.status.success() {
        return Err(CodexxError::Config(format!(
            "WATCHDOG_TASK_START_FAILED: 无法创建或启用 Windows 计划任务: {}",
            command_output_summary(&output)
        )));
    }
    Ok(())
}

#[derive(Debug, Default)]
struct ScheduledTaskSnapshot {
    xml: Option<String>,
    running: bool,
}

#[cfg(target_os = "windows")]
fn query_scheduled_task_xml(task_name: &str) -> Result<Option<String>> {
    let escaped_name = task_name.replace('\'', "''");
    let query = format!(
        "$task=Get-ScheduledTask -TaskName '{escaped_name}' -ErrorAction SilentlyContinue; \
         if ($null -eq $task) {{ exit 3 }}; \
         Export-ScheduledTask -TaskName '{escaped_name}' -ErrorAction Stop"
    );
    let output = program_command(
        Path::new("powershell.exe"),
        &["-NoProfile", "-NonInteractive", "-Command", &query],
    )
    .output()
    .map_err(|error| CodexxError::Config(format!("WATCHDOG_TASK_START_FAILED: {error}")))?;
    if output.status.code() == Some(3) {
        return Ok(None);
    }
    if !output.status.success() {
        return Err(CodexxError::Config(format!(
            "WATCHDOG_TASK_START_FAILED: 无法导出原 Windows 计划任务: {}",
            command_output_summary(&output)
        )));
    }
    Ok(Some(decode_command_bytes(&output.stdout)))
}

#[cfg(not(target_os = "windows"))]
fn query_scheduled_task_xml(_task_name: &str) -> Result<Option<String>> {
    Ok(None)
}

#[cfg(all(target_os = "windows", not(test)))]
fn scheduled_task_running(task_name: &str) -> bool {
    let query = format!(
        "$task=Get-ScheduledTask -TaskName '{}' -ErrorAction SilentlyContinue; if ($null -ne $task -and $task.State -eq 'Running') {{ 'True' }} else {{ 'False' }}",
        task_name.replace('\'', "''")
    );
    program_command(
        Path::new("powershell.exe"),
        &["-NoProfile", "-NonInteractive", "-Command", &query],
    )
    .output()
    .is_ok_and(|output| {
        output.status.success()
            && decode_command_bytes(&output.stdout)
                .trim()
                .eq_ignore_ascii_case("True")
    })
}

#[cfg(not(all(target_os = "windows", not(test))))]
fn scheduled_task_running(_task_name: &str) -> bool {
    false
}

fn capture_scheduled_task(task_name: &str) -> Result<ScheduledTaskSnapshot> {
    Ok(ScheduledTaskSnapshot {
        xml: query_scheduled_task_xml(task_name)?,
        running: scheduled_task_running(task_name),
    })
}

#[cfg(target_os = "windows")]
fn restore_scheduled_task_from_directory(
    task_name: &str,
    snapshot: &ScheduledTaskSnapshot,
    directory: &Path,
) -> Result<()> {
    if let Some(xml) = snapshot.xml.as_deref() {
        let path = directory.join(format!("watchdog-task-restore-{}.xml", std::process::id()));
        atomic_write(&path, &utf16le_with_bom(xml)).map_err(|error| {
            CodexxError::Config(format!("WATCHDOG_TASK_ROLLBACK_FAILED: {error}"))
        })?;
        let result = create_watchdog_task(task_name, &path).map_err(|error| {
            CodexxError::Config(format!("WATCHDOG_TASK_ROLLBACK_FAILED: {error}"))
        });
        let _ = fs::remove_file(path);
        result?;
        if snapshot.running {
            let output = program_command(Path::new("schtasks.exe"), &["/Run", "/TN", task_name])
                .output()
                .map_err(|error| {
                    CodexxError::Config(format!("WATCHDOG_TASK_ROLLBACK_FAILED: {error}"))
                })?;
            if !output.status.success() {
                return Err(CodexxError::Config(format!(
                    "WATCHDOG_TASK_ROLLBACK_FAILED: {}",
                    command_output_summary(&output)
                )));
            }
        }
    } else {
        let output = program_command(
            Path::new("schtasks.exe"),
            &["/Delete", "/TN", task_name, "/F"],
        )
        .output()
        .map_err(|error| CodexxError::Config(format!("WATCHDOG_TASK_ROLLBACK_FAILED: {error}")))?;
        if !output.status.success() && query_scheduled_task_xml(task_name)?.is_some() {
            return Err(CodexxError::Config(format!(
                "WATCHDOG_TASK_ROLLBACK_FAILED: {}",
                command_output_summary(&output)
            )));
        }
    }
    Ok(())
}

#[cfg(not(target_os = "windows"))]
fn restore_scheduled_task_from_directory(
    _task_name: &str,
    _snapshot: &ScheduledTaskSnapshot,
    _directory: &Path,
) -> Result<()> {
    Ok(())
}

fn restore_scheduled_task(task_name: &str, snapshot: &ScheduledTaskSnapshot) -> Result<()> {
    restore_scheduled_task_from_directory(task_name, snapshot, &mode_dir()?)
}

#[cfg(not(test))]
fn configure_watchdog_autostart(input: &GatewayStartInput, enabled: bool) -> Result<()> {
    let intent = mode_dir()?.join("watchdog-intent.json");
    if enabled {
        let script = watchdog_script()?;
        let intent = if intent.is_file() {
            intent
        } else {
            set_watchdog_intent("gateway", true, input.listen_port, input.upstream.trim())?
        };
        let _ = program_command(
            Path::new("schtasks.exe"),
            &["/End", "/TN", WATCHDOG_TASK_NAME],
        )
        .output();
        let xml_path = mode_dir()?.join(format!(
            "watchdog-task-{}-{}.xml",
            std::process::id(),
            input.listen_port
        ));
        let xml = watchdog_task_xml(input, &intent, &script);
        let result = (|| -> Result<()> {
            atomic_write(&xml_path, &utf16le_with_bom(&xml)).map_err(|error| {
                CodexxError::Config(format!(
                    "WATCHDOG_TASK_START_FAILED: 无法写入任务定义: {error}"
                ))
            })?;
            create_watchdog_task(WATCHDOG_TASK_NAME, &xml_path)
        })();
        let _ = fs::remove_file(&xml_path);
        result?;
    } else {
        let output = program_command(
            Path::new("schtasks.exe"),
            &["/Change", "/TN", WATCHDOG_TASK_NAME, "/DISABLE"],
        )
        .output()
        .map_err(|error| CodexxError::Config(format!("WATCHDOG_TASK_STOP_FAILED: {error}")))?;
        if !output.status.success() && watchdog_autostart() {
            return Err(CodexxError::Config(
                "WATCHDOG_TASK_STOP_FAILED: 无法禁用 Windows 计划任务".to_string(),
            ));
        }
    }
    Ok(())
}

#[cfg(test)]
fn configure_watchdog_autostart(_input: &GatewayStartInput, _enabled: bool) -> Result<()> {
    Ok(())
}

#[cfg(not(test))]
fn run_watchdog_task() -> Result<()> {
    let output = program_command(
        Path::new("schtasks.exe"),
        &["/Run", "/TN", WATCHDOG_TASK_NAME],
    )
    .output()
    .map_err(|error| CodexxError::Config(format!("WATCHDOG_TASK_START_FAILED: {error}")))?;
    if !output.status.success() {
        return Err(CodexxError::Config(format!(
            "WATCHDOG_TASK_START_FAILED: 无法立即启动 Windows 看门狗任务: {}",
            command_output_summary(&output)
        )));
    }
    Ok(())
}

#[cfg(test)]
fn run_watchdog_task() -> Result<()> {
    Ok(())
}

fn remove_watchdog_intent() -> Result<()> {
    let path = mode_dir()?.join("watchdog-intent.json");
    match fs::remove_file(&path) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == ErrorKind::NotFound => Ok(()),
        Err(error) => Err(CodexxError::Config(format!(
            "WATCHDOG_TASK_STOP_FAILED: {}: {error}",
            path.display()
        ))),
    }
}

fn restore_watchdog_after_failure(input: &GatewayStartInput) {
    let _ = set_watchdog_intent("gateway", true, input.listen_port, input.upstream.trim())
        .and_then(|_| configure_watchdog_autostart(input, true))
        .and_then(|_| run_watchdog_task());
}

fn read_watchdog_intent() -> Option<Value> {
    fs::read(mode_dir().ok()?.join("watchdog-intent.json"))
        .ok()
        .and_then(|bytes| serde_json::from_slice(&bytes).ok())
}

fn recovery_upstream(intent: Option<&Value>, runtime: Option<&Value>) -> Option<String> {
    intent
        .and_then(|value| value.get("upstream"))
        .and_then(Value::as_str)
        .or_else(|| {
            runtime
                .and_then(|value| value.pointer("/provider/base_url"))
                .and_then(Value::as_str)
        })
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToString::to_string)
}

fn spawn_gateway_process(port: u16, upstream: &str, runtime_state_file: &Path) -> Result<()> {
    let python = std::env::var("CODEX_X_PYTHON").unwrap_or_else(|_| "python".to_string());
    let script = gateway_script()?;
    let mut command = Command::new(python);
    command
        .args([
            script.to_string_lossy().as_ref(),
            "--listen",
            &format!("127.0.0.1:{port}"),
            "--upstream",
            upstream,
            "--state-file",
            runtime_state_file.to_string_lossy().as_ref(),
        ])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    #[cfg(target_os = "windows")]
    {
        use std::os::windows::process::CommandExt;
        command.creation_flags(0x08000000);
    }
    let child = command
        .spawn()
        .map_err(|error| CodexxError::Config(format!("GATEWAY_PROCESS_START_FAILED: {error}")))?;
    *child_slot()
        .lock()
        .unwrap_or_else(|error| error.into_inner()) = Some(child);
    Ok(())
}

pub(crate) fn initialize_on_startup() -> Result<()> {
    let Some(meta) = read_mode_meta()? else {
        return Ok(());
    };
    if meta.desired_mode != "gateway" {
        return Ok(());
    }
    let runtime_path = mode_dir()?.join("runtime-state.json");
    let runtime = read_optional_runtime_state(&runtime_path);
    let intent = read_watchdog_intent();
    let upstream = recovery_upstream(intent.as_ref(), runtime.as_ref()).ok_or_else(|| {
        CodexxError::Config("DISABLE_STATE_UNAVAILABLE: 无法从持久化状态恢复网关上游".to_string())
    })?;
    let gateway_input = GatewayStartInput {
        listen_host: "127.0.0.1".to_string(),
        listen_port: meta.listen_port,
        upstream: upstream.clone(),
        config_dir: Some(meta.codex_dir.clone()),
    };
    if let Ok(state) = gateway_health(meta.listen_port) {
        if persistently_owned_gateway(Some(&meta), meta.listen_port, &state) {
            set_watchdog_intent("gateway", true, meta.listen_port, &upstream)?;
            configure_watchdog_autostart(&gateway_input, true)?;
            run_watchdog_task()?;
            clear_degraded_state();
            return Ok(());
        }
        return Err(CodexxError::Config(
            "GATEWAY_PORT_IN_USE: 持久化端口由非 Codex-X-Pro 网关占用".to_string(),
        ));
    }
    set_watchdog_intent("gateway", true, meta.listen_port, &upstream)?;
    configure_watchdog_autostart(&gateway_input, true)?;
    #[cfg(test)]
    spawn_gateway_process(meta.listen_port, &upstream, &runtime_path)?;
    #[cfg(not(test))]
    run_watchdog_task()?;
    for _ in 0..30 {
        std::thread::sleep(Duration::from_millis(100));
        if gateway_health(meta.listen_port).is_ok() {
            clear_degraded_state();
            return Ok(());
        }
    }
    #[cfg(test)]
    let _ = stop_process_only();
    Err(CodexxError::Config(
        "GATEWAY_HEALTHCHECK_FAILED: 启动恢复后的网关未在 3 秒内就绪".to_string(),
    ))
}

pub(crate) fn process_state(port: u16) -> GatewayProcessState {
    let mut slot = child_slot()
        .lock()
        .unwrap_or_else(|error| error.into_inner());
    let managed = slot
        .as_mut()
        .is_some_and(|child| child.try_wait().ok().flatten().is_none());
    if !managed {
        *slot = None;
    }
    let process_id = slot.as_ref().map(Child::id);
    drop(slot);
    let watchdog_desired = watchdog_desired();
    let watchdog = watchdog_status(watchdog_desired);
    let watchdog_runtime = watchdog_runtime(watchdog, watchdog_desired);
    let (degraded, degraded_error) = degraded_state();
    let persistent = read_mode_meta().ok().flatten();
    let gateway_mode_expected = persistent
        .as_ref()
        .is_some_and(|meta| meta.desired_mode == "gateway" && meta.listen_port == port);
    match gateway_health(port) {
        Ok(state) => {
            let owned = managed || persistently_owned_gateway(persistent.as_ref(), port, &state);
            let route_active = owned && codex_route_active(persistent.as_ref(), port);
            let process_id = process_id.or_else(|| gateway_process_id(&state));
            GatewayProcessState {
                running: true,
                managed_by_codex_x: owned,
                codex_route_active: route_active,
                listen_port: port,
                process_id,
                state: Some(state),
                error: degraded_error.clone(),
                watchdog_running: watchdog.running,
                watchdog_autostart: watchdog.autostart,
                watchdog_desired,
                watchdog_runtime,
                degraded,
            }
        }
        Err(error) => {
            let (unavailable_degraded, unavailable_error) = classify_unavailable_gateway_state(
                managed,
                gateway_mode_expected,
                degraded,
                degraded_error,
                error.to_string(),
            );
            GatewayProcessState {
                running: false,
                managed_by_codex_x: managed || gateway_mode_expected,
                codex_route_active: false,
                listen_port: port,
                process_id,
                state: None,
                error: unavailable_error,
                watchdog_running: watchdog.running,
                watchdog_autostart: watchdog.autostart,
                watchdog_desired,
                watchdog_runtime,
                degraded: unavailable_degraded,
            }
        }
    }
}

pub(crate) fn start(input: GatewayStartInput) -> Result<GatewayProcessState> {
    if input.listen_host.trim() != "127.0.0.1"
        && input.listen_host.trim().to_ascii_lowercase() != "localhost"
    {
        return Err(CodexxError::Config(
            "GATEWAY_INVALID_LISTEN: 网关只允许监听 127.0.0.1".to_string(),
        ));
    }
    let upstream = reqwest::Url::parse(input.upstream.trim())
        .map_err(|error| CodexxError::Config(format!("PROVIDER_INVALID: {error}")))?;
    if !matches!(upstream.scheme(), "http" | "https") || upstream.host_str().is_none() {
        return Err(CodexxError::Config(
            "PROVIDER_INVALID: 上游必须是 http(s) URL".to_string(),
        ));
    }
    let codex_dir = resolve_codex_dir(input.config_dir.clone())?;
    ensure_directory(&codex_dir)?;
    let config = config_path(&codex_dir);
    let auth = auth_path(&codex_dir);
    let _live_lock = acquire_live_config_lock(&codex_dir)?;
    let original_config = read_file_snapshot(&config)?.unwrap_or_default();
    let original_auth = read_file_snapshot(&auth)?;
    let original_agents = read_file_snapshot(&agents_path(&codex_dir))?;
    let initial_replace = instruction_from_config(&codex_dir, &config, &original_config)?;
    let initial_append = instruction_from_agents(&codex_dir, original_agents.as_deref())?;
    if initial_replace.is_some() && initial_append.is_some() {
        return Err(CodexxError::Config(
            "INSTRUCTION_PATH_UNSUPPORTED: config.toml 与 AGENTS.md 同时包含受管提示词".to_string(),
        ));
    }
    let instruction_mode = if initial_append.is_some() {
        Some("append")
    } else if initial_replace.is_some() {
        Some("replace")
    } else {
        None
    };
    let projected_config = project_config(
        &config,
        &original_config,
        input.listen_port,
        initial_replace.is_some(),
    )?;
    let current = process_state(input.listen_port);
    if current.running {
        if current.managed_by_codex_x && read_mode_meta()?.is_some() {
            return start_watchdog(input);
        }
        return Err(CodexxError::Config(
            "GATEWAY_PORT_IN_USE: 目标端口已被非 Codex-X-Pro 网关占用".to_string(),
        ));
    }
    let mode_dir = mode_dir()?;
    ensure_directory(&mode_dir)?;
    let start_state_snapshot = GatewayStartStateSnapshot::capture(&mode_dir)?;
    let original_config_file = mode_dir.join("original-config.toml");
    let original_auth_file = mode_dir.join("original-auth.json");
    let original_instruction_backup = mode_dir.join("original-instruction.md");
    let original_agents_file = mode_dir.join("original-agents.md");
    atomic_write(&original_config_file, &original_config)?;
    match original_auth.as_deref() {
        Some(bytes) => atomic_write(&original_auth_file, bytes)?,
        None => {
            let _ = fs::remove_file(&original_auth_file);
        }
    }
    if let Some((_, _, bytes)) = initial_replace.as_ref() {
        atomic_write(&original_instruction_backup, bytes)?;
    } else {
        let _ = fs::remove_file(&original_instruction_backup);
    }
    if let Some((path, _, bytes, without_managed)) = initial_append.as_ref() {
        atomic_write(&original_agents_file, bytes)?;
        atomic_write_if_unchanged(path, Some(bytes.as_slice()), without_managed)?;
    } else {
        match original_agents.as_deref() {
            Some(bytes) => atomic_write(&original_agents_file, bytes)?,
            None => {
                let _ = fs::remove_file(&original_agents_file);
            }
        }
    }
    let runtime_state_file = mode_dir.join("runtime-state.json");
    if let Err(error) = spawn_gateway_process(
        input.listen_port,
        input.upstream.trim(),
        &runtime_state_file,
    ) {
        restore_agents_after_failed_start(initial_append.as_ref());
        let _ = fs::remove_file(&original_config_file);
        let _ = fs::remove_file(&original_auth_file);
        let _ = fs::remove_file(&original_instruction_backup);
        let _ = fs::remove_file(&original_agents_file);
        start_state_snapshot.restore();
        return Err(error);
    }
    for _ in 0..30 {
        std::thread::sleep(Duration::from_millis(100));
        let state = process_state(input.listen_port);
        if state.running {
            let provider =
                match provider_from_config(&config, &original_config, input.upstream.trim()) {
                    Ok(provider) => provider,
                    Err(error) => {
                        drop(_live_lock);
                        let _ = stop_process_only();
                        restore_agents_after_failed_start(initial_append.as_ref());
                        let _ = fs::remove_file(&original_config_file);
                        let _ = fs::remove_file(&original_auth_file);
                        let _ = fs::remove_file(&original_instruction_backup);
                        let _ = fs::remove_file(&original_agents_file);
                        start_state_snapshot.restore();
                        return Err(error);
                    }
                };
            if let Err(error) = request_control(&GatewayRequestInput {
                listen_port: input.listen_port,
                method: "PUT".to_string(),
                path: "/state/provider".to_string(),
                body: Some(provider),
            }) {
                drop(_live_lock);
                let _ = stop_process_only();
                restore_agents_after_failed_start(initial_append.as_ref());
                let _ = fs::remove_file(&original_config_file);
                let _ = fs::remove_file(&original_auth_file);
                let _ = fs::remove_file(&original_instruction_backup);
                let _ = fs::remove_file(&original_agents_file);
                start_state_snapshot.restore();
                return Err(CodexxError::Config(format!(
                    "GATEWAY_RUNTIME_SYNC_FAILED: {error}"
                )));
            }
            if let Some((_, content, _)) = initial_replace.as_ref() {
                let instruction = json!({"enabled": true, "content": content, "injection_mode": "replace", "template_id": "external"});
                if let Err(error) = request_control(&GatewayRequestInput {
                    listen_port: input.listen_port,
                    method: "PUT".to_string(),
                    path: "/state/instruction".to_string(),
                    body: Some(instruction),
                }) {
                    drop(_live_lock);
                    let _ = stop_process_only();
                    restore_agents_after_failed_start(initial_append.as_ref());
                    let _ = fs::remove_file(&original_config_file);
                    let _ = fs::remove_file(&original_auth_file);
                    let _ = fs::remove_file(&original_instruction_backup);
                    let _ = fs::remove_file(&original_agents_file);
                    start_state_snapshot.restore();
                    return Err(CodexxError::Config(format!(
                        "GATEWAY_RUNTIME_SYNC_FAILED: {error}"
                    )));
                }
            }
            if let Some((_, content, _, _)) = initial_append.as_ref() {
                let instruction = json!({"enabled": true, "content": content, "injection_mode": "append", "template_id": "external"});
                if let Err(error) = request_control(&GatewayRequestInput {
                    listen_port: input.listen_port,
                    method: "PUT".to_string(),
                    path: "/state/instruction".to_string(),
                    body: Some(instruction),
                }) {
                    drop(_live_lock);
                    let _ = stop_process_only();
                    restore_agents_after_failed_start(initial_append.as_ref());
                    let _ = fs::remove_file(&original_config_file);
                    let _ = fs::remove_file(&original_auth_file);
                    let _ = fs::remove_file(&original_instruction_backup);
                    let _ = fs::remove_file(&original_agents_file);
                    start_state_snapshot.restore();
                    return Err(CodexxError::Config(format!(
                        "GATEWAY_RUNTIME_SYNC_FAILED: {error}"
                    )));
                }
            }
            if let Err(error) = atomic_write_if_unchanged(
                &config,
                Some(original_config.as_slice()),
                &projected_config,
            ) {
                drop(_live_lock);
                let _ = stop_process_only();
                restore_agents_after_failed_start(initial_append.as_ref());
                let _ = fs::remove_file(&original_config_file);
                let _ = fs::remove_file(&original_auth_file);
                let _ = fs::remove_file(&original_instruction_backup);
                let _ = fs::remove_file(&original_agents_file);
                start_state_snapshot.restore();
                return Err(error);
            }
            let meta = GatewayModeMeta {
                version: 1,
                desired_mode: "gateway".to_string(),
                codex_dir: codex_dir.to_string_lossy().into_owned(),
                listen_port: input.listen_port,
                projected_config_sha256: sha256_hex(&projected_config),
                original_config_file: original_config_file.to_string_lossy().into_owned(),
                original_auth_file: original_auth_file.to_string_lossy().into_owned(),
                original_instruction_file: initial_replace
                    .as_ref()
                    .map(|(path, _, _)| path.to_string_lossy().into_owned()),
                original_instruction_backup: initial_replace
                    .as_ref()
                    .map(|_| original_instruction_backup.to_string_lossy().into_owned()),
                original_agents_file: Some(agents_path(&codex_dir).to_string_lossy().into_owned()),
                original_agents_backup: original_agents
                    .as_ref()
                    .map(|_| original_agents_file.to_string_lossy().into_owned()),
                instruction_mode: instruction_mode.map(ToString::to_string),
            };
            if let Err(error) = write_mode_meta(&meta) {
                let _ = atomic_write_if_unchanged(
                    &config,
                    Some(projected_config.as_slice()),
                    &original_config,
                );
                drop(_live_lock);
                let _ = stop_process_only();
                restore_agents_after_failed_start(initial_append.as_ref());
                let _ = fs::remove_file(&original_config_file);
                let _ = fs::remove_file(&original_auth_file);
                let _ = fs::remove_file(&original_instruction_backup);
                let _ = fs::remove_file(&original_agents_file);
                start_state_snapshot.restore();
                return Err(error);
            }
            if let Err(error) = start_watchdog(input.clone()) {
                let _ = atomic_write_if_unchanged(
                    &config,
                    Some(projected_config.as_slice()),
                    &original_config,
                );
                drop(_live_lock);
                let _ = stop_process_only();
                restore_agents_after_failed_start(initial_append.as_ref());
                let _ = fs::remove_file(&original_config_file);
                let _ = fs::remove_file(&original_auth_file);
                let _ = fs::remove_file(&original_instruction_backup);
                let _ = fs::remove_file(&original_agents_file);
                let _ = remove_watchdog_intent();
                start_state_snapshot.restore();
                return Err(error);
            }
            clear_degraded_state();
            return Ok(state);
        }
    }
    drop(_live_lock);
    stop_process_only()?;
    restore_agents_after_failed_start(initial_append.as_ref());
    let _ = fs::remove_file(&original_config_file);
    let _ = fs::remove_file(&original_auth_file);
    let _ = fs::remove_file(&original_instruction_backup);
    let _ = fs::remove_file(&original_agents_file);
    start_state_snapshot.restore();
    Err(CodexxError::Config(
        "GATEWAY_LISTEN_TIMEOUT: 网关进程已启动，但控制接口未在 3 秒内就绪".to_string(),
    ))
}

fn stop_process_only() -> Result<()> {
    let mut slot = child_slot()
        .lock()
        .unwrap_or_else(|error| error.into_inner());
    let Some(mut child) = slot.take() else {
        return Err(CodexxError::Config(
            "GATEWAY_PROCESS_NOT_MANAGED: 当前监听并非由 Codex-X-Pro 启动，未执行终止".to_string(),
        ));
    };
    child
        .kill()
        .map_err(|error| CodexxError::Config(format!("GATEWAY_PROCESS_STOP_FAILED: {error}")))?;
    child
        .wait()
        .map_err(|error| CodexxError::Config(format!("GATEWAY_PROCESS_STOP_FAILED: {error}")))?;
    Ok(())
}

fn terminate_persisted_gateway(meta: &GatewayModeMeta) -> Result<()> {
    {
        let mut slot = child_slot()
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        let running = slot
            .as_mut()
            .is_some_and(|child| child.try_wait().ok().flatten().is_none());
        if !running {
            *slot = None;
        }
        if running {
            drop(slot);
            return stop_process_only();
        }
    }
    let state = match gateway_health(meta.listen_port) {
        Ok(state) => state,
        Err(_) => return Ok(()),
    };
    if !persistently_owned_gateway(Some(meta), meta.listen_port, &state) {
        return Err(CodexxError::Config(
            "GATEWAY_PROCESS_NOT_MANAGED: 健康端口与持久化网关身份不匹配".to_string(),
        ));
    }
    let process_id = gateway_process_id(&state).ok_or_else(|| {
        CodexxError::Config("GATEWAY_PROCESS_NOT_MANAGED: 网关未提供可验证的进程 ID".to_string())
    })?;
    #[cfg(target_os = "windows")]
    let process_id_text = process_id.to_string();
    let status = program_command(
        Path::new("taskkill.exe"),
        &["/PID", &process_id_text, "/T", "/F"],
    )
    .status();
    #[cfg(not(target_os = "windows"))]
    let status = Command::new("kill")
        .args(["-TERM", &process_id.to_string()])
        .status();
    let status = status
        .map_err(|error| CodexxError::Config(format!("GATEWAY_PROCESS_STOP_FAILED: {error}")))?;
    if !status.success() && gateway_health(meta.listen_port).is_ok() {
        return Err(CodexxError::Config(format!(
            "GATEWAY_PROCESS_STOP_FAILED: 无法终止网关 PID {process_id}"
        )));
    }
    for _ in 0..40 {
        if gateway_health(meta.listen_port).is_err() {
            return Ok(());
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    Err(CodexxError::Config(
        "GATEWAY_PROCESS_STOP_FAILED: 网关终止后端口仍在监听".to_string(),
    ))
}

pub(crate) fn stop() -> Result<()> {
    let meta = read_mode_meta()?.ok_or_else(|| {
        CodexxError::Config(
            "DISABLE_STATE_UNAVAILABLE: 未找到网关模式快照，无法安全恢复直连配置".to_string(),
        )
    })?;
    if let Ok(state) = gateway_health(meta.listen_port) {
        if !persistently_owned_gateway(Some(&meta), meta.listen_port, &state) {
            return Err(CodexxError::Config(
                "DISABLE_STATE_UNAVAILABLE: 网关身份与持久化快照不匹配".to_string(),
            ));
        }
    }
    let codex_dir = PathBuf::from(&meta.codex_dir);
    let _live_lock = acquire_live_config_lock(&codex_dir)?;
    let config = config_path(&codex_dir);
    let auth = auth_path(&codex_dir);
    let agents = meta
        .original_agents_file
        .as_deref()
        .map(PathBuf::from)
        .unwrap_or_else(|| agents_path(&codex_dir));
    let current_config = read_file_snapshot(&config)?.ok_or_else(|| {
        CodexxError::Config(
            "DIRECT_CONFIG_WRITE_FAILED: config.toml 不存在，无法生成直连配置".to_string(),
        )
    })?;
    let current_auth = read_file_snapshot(&auth)?;
    let current_agents = read_file_snapshot(&agents)?;
    let _stop_backup = create_backup(&codex_dir, "gateway-stop")?;
    let persisted_runtime = read_optional_runtime_state(&mode_dir()?.join("runtime-state.json"));
    let runtime_provider = request_control(&GatewayRequestInput {
        listen_port: meta.listen_port,
        method: "GET".to_string(),
        path: "/state/provider/secret".to_string(),
        body: None,
    })
    .ok()
    .or_else(|| persisted_runtime.clone())
    .and_then(|value| value.get("provider").cloned())
    .ok_or_else(|| {
        CodexxError::Config("DISABLE_STATE_UNAVAILABLE: 网关状态缺少当前 Provider".to_string())
    })?;
    let runtime_instruction = gateway_request_instruction(meta.listen_port)
        .or_else(|| persisted_runtime.and_then(|value| value.get("instruction").cloned()))
        .ok_or_else(|| {
            CodexxError::Config("DISABLE_STATE_UNAVAILABLE: 无法读取网关当前提示词状态".to_string())
        })?;
    let runtime_instruction_enabled =
        runtime_instruction.get("enabled").and_then(Value::as_bool) == Some(true);
    let runtime_injection_mode = runtime_instruction
        .get("injection_mode")
        .and_then(Value::as_str)
        .unwrap_or("append");
    let runtime_base_url = runtime_provider
        .get("base_url")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .trim();
    if runtime_base_url.trim_end_matches('/')
        == projected_base_url(meta.listen_port).trim_end_matches('/')
    {
        return Err(CodexxError::Config(
            "DIRECT_CONFIG_INVALID_AFTER_WRITE: runtime Provider still points to the gateway being stopped"
                .to_string(),
        ));
    }
    let mut restored_config =
        project_runtime_provider(&config, &current_config, &runtime_provider)?;
    if !runtime_instruction_enabled || runtime_injection_mode == "append" {
        let text = std::str::from_utf8(&restored_config).map_err(|error| {
            CodexxError::Config(format!("DIRECT_CONFIG_INVALID_AFTER_WRITE: {error}"))
        })?;
        let mut document = parse_toml_document(&config, text).map_err(|error| {
            CodexxError::Config(format!("DIRECT_CONFIG_INVALID_AFTER_WRITE: {error}"))
        })?;
        document.as_table_mut().remove("model_instructions_file");
        restored_config = document.to_string().into_bytes();
    }
    let mut instruction_update =
        if runtime_instruction_enabled && runtime_injection_mode == "replace" {
            let path = meta
                .original_instruction_file
                .as_deref()
                .map(PathBuf::from)
                .unwrap_or_else(|| codex_dir.join("codex-x-gateway-prompt.md"));
            let current = read_file_snapshot(&path)?;
            let content = runtime_instruction
                .get("content")
                .and_then(Value::as_str)
                .ok_or_else(|| {
                    CodexxError::Config(
                        "DIRECT_CONFIG_INVALID_AFTER_WRITE: 网关提示词状态缺少 content".to_string(),
                    )
                })?;
            let text = std::str::from_utf8(&restored_config).map_err(|error| {
                CodexxError::Config(format!(
                    "DIRECT_CONFIG_INVALID_AFTER_WRITE: config.toml: {error}"
                ))
            })?;
            let mut document = parse_toml_document(&config, text).map_err(|error| {
                CodexxError::Config(format!("DIRECT_CONFIG_INVALID_AFTER_WRITE: {error}"))
            })?;
            document["model_instructions_file"] = value(path.to_string_lossy().as_ref());
            restored_config = document.to_string().into_bytes();
            Some((path, current, content.as_bytes().to_vec()))
        } else {
            None
        };
    if instruction_update.is_none()
        && runtime_instruction_enabled
        && runtime_injection_mode == "replace"
    {
        let path = codex_dir.join("codex-x-gateway-prompt.md");
        let content = runtime_instruction
            .get("content")
            .and_then(Value::as_str)
            .ok_or_else(|| {
                CodexxError::Config(
                    "DIRECT_CONFIG_INVALID_AFTER_WRITE: 网关提示词状态缺少 content".to_string(),
                )
            })?;
        let text = std::str::from_utf8(&restored_config).map_err(|error| {
            CodexxError::Config(format!("DIRECT_CONFIG_INVALID_AFTER_WRITE: {error}"))
        })?;
        let mut document = parse_toml_document(&config, text).map_err(|error| {
            CodexxError::Config(format!("DIRECT_CONFIG_INVALID_AFTER_WRITE: {error}"))
        })?;
        document["model_instructions_file"] = value("./codex-x-gateway-prompt.md");
        restored_config = document.to_string().into_bytes();
        instruction_update = Some((path, None, content.as_bytes().to_vec()));
    }
    /*
     * Replace mode owns the selected prompt file. Append mode owns only the
     * Codex-X-Pro block in AGENTS.md; all content outside that block comes from
     * the file currently on disk.
     */
    let agents_update = if runtime_injection_mode == "append" {
        let current_text =
            String::from_utf8(current_agents.clone().unwrap_or_default()).map_err(|error| {
                CodexxError::Config(format!(
                    "DIRECT_CONFIG_INVALID_AFTER_WRITE: AGENTS.md 不是 UTF-8: {error}"
                ))
            })?;
        let (without_managed, _) = remove_managed_agents_block_from_content(&current_text)?;
        let replacement = if runtime_instruction_enabled {
            let content = runtime_instruction
                .get("content")
                .and_then(Value::as_str)
                .ok_or_else(|| {
                    CodexxError::Config(
                        "DIRECT_CONFIG_INVALID_AFTER_WRITE: 提示词状态缺少 content".to_string(),
                    )
                })?;
            install_managed_agents_block_in_content(&without_managed, "external", content)?
        } else {
            without_managed
        };
        let replacement = replacement.into_bytes();
        if (current_agents.is_none() && replacement.is_empty())
            || current_agents.as_deref() == Some(replacement.as_slice())
        {
            None
        } else {
            Some((agents.clone(), current_agents.clone(), replacement))
        }
    } else {
        None
    };
    let watchdog_input = GatewayStartInput {
        listen_host: "127.0.0.1".to_string(),
        listen_port: meta.listen_port,
        upstream: runtime_provider
            .get("base_url")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string(),
        config_dir: Some(meta.codex_dir.clone()),
    };
    set_watchdog_intent("gateway", false, meta.listen_port, &watchdog_input.upstream)?;
    if let Err(error) =
        stop_watchdog_runtime().and_then(|_| configure_watchdog_autostart(&watchdog_input, false))
    {
        restore_watchdog_after_failure(&watchdog_input);
        return Err(error);
    }
    if let Err(error) = terminate_persisted_gateway(&meta) {
        restore_watchdog_after_failure(&watchdog_input);
        return Err(error);
    }
    let restoration = (|| -> Result<()> {
        let mut changes = Vec::new();
        let mutation = (|| -> Result<()> {
            changes.push(apply_file_change(
                &config,
                Some(current_config.clone()),
                Some(restored_config.clone()),
            )?);
            if let Some((path, before, replacement)) = instruction_update.as_ref() {
                changes.push(apply_file_change(
                    path,
                    before.clone(),
                    Some(replacement.clone()),
                )?);
            }
            let auth_after = runtime_auth_bytes(&runtime_provider, current_auth.as_deref())?;
            if auth_after != current_auth {
                changes.push(apply_file_change(&auth, current_auth.clone(), auth_after)?);
            }
            if let Some((path, before, replacement)) = agents_update.as_ref() {
                changes.push(apply_file_change(
                    path,
                    before.clone(),
                    Some(replacement.clone()),
                )?);
            }
            Ok(())
        })();
        if let Err(error) = mutation {
            let error_text = error.to_string();
            let error = if error_text.contains("已被其他程序修改") {
                CodexxError::Config(format!("DIRECT_CONFIG_WRITE_CONFLICT: {error_text}"))
            } else {
                error
            };
            return match rollback_file_changes(&changes) {
                Ok(()) => Err(error),
                Err(rollback_error) => Err(CodexxError::Config(format!(
                    "GATEWAY_DEGRADED: {error}; file rollback failed: {rollback_error}"
                ))),
            };
        }
        remove_watchdog_intent()?;
        let _ = fs::remove_file(&meta.original_config_file);
        let _ = fs::remove_file(&meta.original_auth_file);
        if let Some(path) = meta.original_instruction_backup.as_deref() {
            let _ = fs::remove_file(path);
        }
        if let Some(path) = meta.original_agents_backup.as_deref() {
            let _ = fs::remove_file(path);
        }
        let _ = fs::remove_file(mode_meta_path()?);
        let _ = fs::remove_file(mode_dir()?.join("runtime-state.json"));
        clear_degraded_state();
        Ok(())
    })();
    if let Err(error) = restoration {
        let recovery = restore_gateway_after_stop_failure(&meta, &watchdog_input);
        if let Err(recovery_error) = recovery {
            mark_degraded_state(&format!(
                "{error}; gateway recovery failed: {recovery_error}"
            ));
            return Err(CodexxError::Config(format!(
                "GATEWAY_DEGRADED: {error}; gateway recovery failed: {recovery_error}"
            )));
        }
        return Err(error);
    }
    Ok(())
}

fn restore_gateway_after_stop_failure(
    meta: &GatewayModeMeta,
    input: &GatewayStartInput,
) -> Result<()> {
    set_watchdog_intent("gateway", true, meta.listen_port, input.upstream.trim())?;
    configure_watchdog_autostart(input, true)?;
    initialize_on_startup()?;
    if gateway_health(meta.listen_port).is_err() {
        return Err(CodexxError::Config(
            "GATEWAY_HEALTHCHECK_FAILED: gateway recovery did not become healthy".to_string(),
        ));
    }
    Ok(())
}

fn gateway_request_instruction(port: u16) -> Option<Value> {
    request_control(&GatewayRequestInput {
        listen_port: port,
        method: "GET".to_string(),
        path: "/state/instruction".to_string(),
        body: None,
    })
    .ok()
}

#[derive(Debug, Clone, Copy, Default)]
struct WatchdogStatus {
    running: bool,
    autostart: bool,
}

fn watchdog_task_status(desired: bool) -> WatchdogStatus {
    if !desired {
        return WatchdogStatus::default();
    }
    #[cfg(target_os = "windows")]
    {
        let query = format!(
            "$task=Get-ScheduledTask -TaskName '{}' -ErrorAction SilentlyContinue; \
             if ($null -eq $task) {{ 'Missing|False' }} \
             else {{ '{{0}}|{{1}}' -f $task.State, $task.Settings.Enabled }}",
            WATCHDOG_TASK_NAME.replace('\'', "''")
        );
        return program_command(
            Path::new("powershell.exe"),
            &["-NoProfile", "-NonInteractive", "-Command", &query],
        )
        .output()
        .ok()
        .and_then(|output| {
            let value = String::from_utf8_lossy(&output.stdout);
            let mut fields = value.trim().split('|');
            Some(WatchdogStatus {
                running: fields.next() == Some("Running"),
                autostart: fields
                    .next()
                    .is_some_and(|field| field.eq_ignore_ascii_case("True")),
            })
        })
        .unwrap_or_default();
    }
    #[cfg(not(target_os = "windows"))]
    {
        WatchdogStatus::default()
    }
}

fn watchdog_running() -> bool {
    watchdog_status(watchdog_desired()).running
}

fn watchdog_status(desired: bool) -> WatchdogStatus {
    let mut slot = watchdog_slot()
        .lock()
        .unwrap_or_else(|error| error.into_inner());
    let running = slot
        .as_mut()
        .is_some_and(|child| child.try_wait().ok().flatten().is_none());
    if !running {
        *slot = None;
    }
    let task = watchdog_task_status(desired);
    WatchdogStatus {
        running: running || task.running,
        autostart: task.autostart,
    }
}

const WATCHDOG_TASK_NAME: &str = "Codex-X-Pro Local Gateway";

fn watchdog_autostart() -> bool {
    program_command(
        Path::new("schtasks.exe"),
        &["/Query", "/TN", WATCHDOG_TASK_NAME, "/XML"],
    )
    .output()
    .is_ok_and(|output| {
        output.status.success()
            && !String::from_utf8_lossy(&output.stdout)
                .to_ascii_lowercase()
                .contains("<enabled>false</enabled>")
    })
}

fn watchdog_desired() -> bool {
    mode_dir()
        .ok()
        .and_then(|dir| fs::read(dir.join("watchdog-intent.json")).ok())
        .and_then(|bytes| serde_json::from_slice::<Value>(&bytes).ok())
        .and_then(|value| value.get("watchdog_desired").and_then(Value::as_bool))
        .unwrap_or(false)
}

fn watchdog_runtime(status: WatchdogStatus, desired: bool) -> String {
    if status.running {
        "running".to_string()
    } else if desired {
        "starting".to_string()
    } else {
        "stopped".to_string()
    }
}

fn watchdog_script() -> Result<PathBuf> {
    if let Some(path) = std::env::var_os("CODEX_X_GATEWAY_WATCHDOG").map(PathBuf::from) {
        if path.is_file() {
            return Ok(path);
        }
    }
    if let Some(profile) = std::env::var_os("USERPROFILE").map(PathBuf::from) {
        let personal = profile
            .join(".codex-x")
            .join("personal-gateway")
            .join("codex_responses_repair_watchdog.ps1");
        if personal.is_file() {
            return Ok(personal);
        }
    }
    Err(CodexxError::Config(
        "WATCHDOG_TASK_MISSING: 找不到外部本地看门狗脚本，请设置 CODEX_X_GATEWAY_WATCHDOG"
            .to_string(),
    ))
}

pub(crate) fn start_watchdog(input: GatewayStartInput) -> Result<GatewayProcessState> {
    if !process_state(input.listen_port).running {
        return Err(CodexxError::Config(
            "WATCHDOG_GATEWAY_REQUIRED: 请先开启网关".to_string(),
        ));
    }
    set_watchdog_intent("gateway", true, input.listen_port, input.upstream.trim())?;
    let task_snapshot = capture_scheduled_task(WATCHDOG_TASK_NAME)?;
    if let Err(error) = configure_watchdog_autostart(&input, true).and_then(|_| {
        if !watchdog_running() {
            run_watchdog_task()?;
        }
        Ok(())
    }) {
        if let Err(rollback_error) = restore_scheduled_task(WATCHDOG_TASK_NAME, &task_snapshot) {
            return Err(CodexxError::Config(format!("{error}; {rollback_error}")));
        }
        return Err(error);
    }
    Ok(process_state(input.listen_port))
}

fn stop_watchdog_runtime() -> Result<()> {
    #[cfg(target_os = "windows")]
    {
        let output = program_command(
            Path::new("schtasks.exe"),
            &["/End", "/TN", WATCHDOG_TASK_NAME],
        )
        .output()
        .map_err(|error| CodexxError::Config(format!("WATCHDOG_TASK_STOP_FAILED: {error}")))?;
        if !output.status.success() && watchdog_task_status(true).running {
            return Err(CodexxError::Config(
                "WATCHDOG_TASK_STOP_FAILED: 无法停止 Windows 看门狗任务".to_string(),
            ));
        }
    }
    let mut slot = watchdog_slot()
        .lock()
        .unwrap_or_else(|error| error.into_inner());
    if let Some(mut child) = slot.take() {
        for _ in 0..40 {
            if child
                .try_wait()
                .map_err(|error| {
                    CodexxError::Config(format!("WATCHDOG_TASK_STOP_FAILED: {error}"))
                })?
                .is_some()
            {
                return Ok(());
            }
            std::thread::sleep(Duration::from_millis(100));
        }
        child
            .kill()
            .map_err(|error| CodexxError::Config(format!("WATCHDOG_TASK_STOP_FAILED: {error}")))?;
        child
            .wait()
            .map_err(|error| CodexxError::Config(format!("WATCHDOG_TASK_STOP_FAILED: {error}")))?;
    }
    Ok(())
}

pub(crate) fn request(input: GatewayRequestInput) -> Result<Value> {
    request_control(&input)
}

/// Application exit does not change the persistent gateway mode.
pub(crate) fn shutdown_on_exit() -> Result<()> {
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::TcpListener;
    use std::sync::atomic::{AtomicU64, Ordering};

    fn isolated_test_path(prefix: &str) -> PathBuf {
        static NEXT_ID: AtomicU64 = AtomicU64::new(1);
        std::env::temp_dir().join(format!(
            "{prefix}-{}-{}",
            std::process::id(),
            NEXT_ID.fetch_add(1, Ordering::Relaxed)
        ))
    }

    #[test]
    fn unavailable_gateway_health_is_not_an_error_after_normal_stop() {
        let (degraded, error) = classify_unavailable_gateway_state(
            false,
            false,
            false,
            None,
            "CONTROL_API_UNAVAILABLE: connection refused".to_string(),
        );

        assert!(!degraded);
        assert!(error.is_none());
    }

    #[test]
    fn unavailable_expected_or_managed_gateway_is_degraded() {
        for (managed, gateway_mode_expected) in [(true, false), (false, true)] {
            let (degraded, error) = classify_unavailable_gateway_state(
                managed,
                gateway_mode_expected,
                false,
                None,
                "CONTROL_API_UNAVAILABLE: connection refused".to_string(),
            );

            assert!(degraded);
            assert_eq!(
                error.as_deref(),
                Some("CONTROL_API_UNAVAILABLE: connection refused")
            );
        }
    }

    #[test]
    fn persisted_degraded_error_takes_priority_over_health_probe_error() {
        let (degraded, error) = classify_unavailable_gateway_state(
            false,
            false,
            true,
            Some("GATEWAY_DEGRADED: recovery failed".to_string()),
            "CONTROL_API_UNAVAILABLE: connection refused".to_string(),
        );

        assert!(degraded);
        assert_eq!(error.as_deref(), Some("GATEWAY_DEGRADED: recovery failed"));
    }

    #[test]
    fn oversized_runtime_state_is_ignored_before_reading_payload() {
        let directory = isolated_test_path("codex-x-gateway-large-runtime");
        fs::create_dir_all(&directory).expect("create test directory");
        let path = directory.join("runtime-state.json");
        let file = fs::File::create(&path).expect("create runtime state");
        file.set_len(MAX_GATEWAY_STATE_FILE_BYTES + 1)
            .expect("grow runtime state");

        assert!(read_optional_runtime_state(&path).is_none());

        let _ = fs::remove_dir_all(directory);
    }

    #[test]
    fn oversized_mode_metadata_is_rejected_before_reading_payload() {
        let directory = isolated_test_path("codex-x-gateway-large-meta");
        fs::create_dir_all(&directory).expect("create test directory");
        let path = directory.join("state.json");
        let file = fs::File::create(&path).expect("create mode metadata");
        file.set_len(MAX_GATEWAY_STATE_FILE_BYTES + 1)
            .expect("grow mode metadata");

        let error = read_bounded_file(&path, MAX_GATEWAY_STATE_FILE_BYTES)
            .expect_err("oversized mode metadata must be rejected");
        assert!(error.to_string().contains("GATEWAY_STATE_TOO_LARGE"));

        let _ = fs::remove_dir_all(directory);
    }

    #[test]
    fn startup_snapshot_rejects_oversized_runtime_state_before_full_read() {
        let directory = isolated_test_path("codex-x-gateway-large-snapshot");
        fs::create_dir_all(&directory).expect("create test directory");
        let path = directory.join("runtime-state.json");
        let file = fs::File::create(&path).expect("create runtime state");
        file.set_len(MAX_GATEWAY_STATE_FILE_BYTES + 1)
            .expect("grow runtime state");

        let error = GatewayStartStateSnapshot::capture(&directory)
            .expect_err("oversized runtime state must stop startup safely");
        assert!(error.to_string().contains("GATEWAY_STATE_TOO_LARGE"));

        let _ = fs::remove_dir_all(directory);
    }

    #[test]
    fn control_url_rejects_invalid_paths_and_zero_port() {
        assert!(control_url(0, "/state").is_err());
        assert!(control_url(8787, "state").is_err());
        assert!(control_url(8787, "/../state").is_err());
        assert_eq!(
            control_url(18787, "/health").unwrap(),
            "http://127.0.0.1:18787/health"
        );
    }

    #[test]
    fn tauri_gateway_request_arguments_require_the_input_wrapper() {
        #[derive(Deserialize)]
        struct GatewayCommandArgs {
            input: GatewayRequestInput,
        }

        let valid = serde_json::from_value::<GatewayCommandArgs>(json!({
            "input": {
                "listenPort": 18787,
                "method": "GET",
                "path": "/observe/state",
                "body": null
            }
        }))
        .expect("wrapped gateway request arguments");
        assert_eq!(valid.input.listen_port, 18787);
        assert_eq!(valid.input.method, "GET");
        assert_eq!(valid.input.path, "/observe/state");
        assert!(valid.input.body.is_none());

        assert!(
            serde_json::from_value::<GatewayCommandArgs>(json!({
                "listenPort": 18787,
                "method": "GET",
                "path": "/observe/state",
                "body": null
            }))
            .is_err(),
            "flattened arguments must fail at the Tauri command boundary"
        );
    }

    #[test]
    fn tauri_gateway_command_payloads_use_camel_case_and_reject_invalid_shapes() {
        #[derive(Deserialize)]
        struct ProcessStateCommandArgs {
            #[serde(rename = "listenPort")]
            listen_port: u16,
        }

        #[derive(Deserialize)]
        struct StartCommandArgs {
            input: GatewayStartInput,
        }

        #[derive(Deserialize)]
        struct RequestCommandArgs {
            input: GatewayRequestInput,
        }

        let process_state = serde_json::from_value::<ProcessStateCommandArgs>(json!({
            "listenPort": 8788
        }))
        .expect("camelCase process state arguments");
        assert_eq!(process_state.listen_port, 8788);

        let start = serde_json::from_value::<StartCommandArgs>(json!({
            "input": {
                "listenHost": "127.0.0.1",
                "listenPort": 8788,
                "upstream": "http://127.0.0.1:19090"
            }
        }))
        .expect("wrapped start arguments");
        assert_eq!(start.input.listen_host, "127.0.0.1");
        assert_eq!(start.input.listen_port, 8788);
        assert!(start.input.config_dir.is_none());

        let request = serde_json::from_value::<RequestCommandArgs>(json!({
            "input": {
                "listenPort": 8788,
                "method": "GET",
                "path": "/observe/state",
                "body": null
            }
        }))
        .expect("wrapped request arguments");
        assert_eq!(request.input.listen_port, 8788);
        assert!(request.input.body.is_none());

        assert!(
            serde_json::from_value::<ProcessStateCommandArgs>(json!({
                "listen_port": 8788
            }))
            .is_err(),
            "snake_case process state arguments must fail"
        );
        assert!(
            serde_json::from_value::<StartCommandArgs>(json!({
                "listenHost": "127.0.0.1",
                "listenPort": 8788,
                "upstream": "http://127.0.0.1:19090"
            }))
            .is_err(),
            "flattened start arguments must fail"
        );
        assert!(
            serde_json::from_value::<RequestCommandArgs>(json!({
                "input": {
                    "listenPort": "8788",
                    "method": "GET",
                    "path": "/observe/state",
                    "body": null
                }
            }))
            .is_err(),
            "invalid request field types must fail"
        );
    }

    #[test]
    fn gateway_request_reaches_only_the_loopback_control_api_with_json_body() {
        let listener = TcpListener::bind(("127.0.0.1", 0)).expect("bind control API test server");
        let port = listener
            .local_addr()
            .expect("control API test address")
            .port();
        let server = std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().expect("accept control API request");
            stream
                .set_read_timeout(Some(Duration::from_secs(2)))
                .expect("set control API read timeout");
            let mut bytes = Vec::new();
            let mut chunk = [0u8; 4096];
            loop {
                let read = stream.read(&mut chunk).expect("read control API request");
                if read == 0 {
                    break;
                }
                bytes.extend_from_slice(&chunk[..read]);
                let header_end = bytes
                    .windows(4)
                    .position(|window| window == b"\r\n\r\n")
                    .map(|index| index + 4);
                let Some(header_end) = header_end else {
                    continue;
                };
                let header_text = String::from_utf8_lossy(&bytes[..header_end]);
                let content_length = header_text
                    .lines()
                    .find_map(|line| {
                        line.strip_prefix("Content-Length:")
                            .or_else(|| line.strip_prefix("content-length:"))
                            .and_then(|value| value.trim().parse::<usize>().ok())
                    })
                    .unwrap_or(0);
                if bytes.len() >= header_end + content_length {
                    break;
                }
            }
            let request = String::from_utf8(bytes).expect("UTF-8 control API request");
            assert!(request.starts_with("PUT /state/provider HTTP/1.1"));
            assert!(request.contains("\"provider_id\":\"synthetic\""));
            assert!(request.contains("\"model\":\"test-model\""));
            stream
                .write_all(
                    b"HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: 11\r\n\r\n{\"ok\":true}",
                )
                .expect("write control API response");
        });

        let result = request(GatewayRequestInput {
            listen_port: port,
            method: "put".to_string(),
            path: "/state/provider".to_string(),
            body: Some(json!({
                "provider_id": "synthetic",
                "model": "test-model"
            })),
        })
        .expect("request loopback control API");

        server.join().expect("join control API test server");
        assert_eq!(result, json!({"ok": true}));
    }

    #[test]
    fn gateway_request_preserves_control_api_error_code_without_leaking_body() {
        let listener = TcpListener::bind(("127.0.0.1", 0)).expect("bind error test server");
        let port = listener.local_addr().expect("error test address").port();
        let server = std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().expect("accept error request");
            let mut buffer = [0u8; 2048];
            let _ = stream.read(&mut buffer);
            stream
                .write_all(
                    b"HTTP/1.1 409 Conflict\r\nContent-Type: application/json\r\nContent-Length: 65\r\n\r\n{\"error\":{\"code\":\"STATE_VERSION_CONFLICT\",\"message\":\"synthetic\"}}",
                )
                .expect("write error response");
        });

        let error = request(GatewayRequestInput {
            listen_port: port,
            method: "POST".to_string(),
            path: "/state/provider".to_string(),
            body: None,
        })
        .expect_err("control API error");

        server.join().expect("join error test server");
        assert!(error.to_string().contains("STATE_VERSION_CONFLICT"));
        assert!(error.to_string().contains("synthetic"));
    }

    #[test]
    fn config_projection_changes_active_routes_and_can_remove_instruction_pointer() {
        let path = PathBuf::from("config.toml");
        let original = br#"model_provider = "custom"
model = "old-model"
model_instructions_file = "./prompt.md"

[model_providers.custom]
name = "Custom"
base_url = "https://provider.example/v1"
wire_api = "responses"
"#;
        let projected = project_config(&path, original, 18787, true).expect("project config");
        let text = String::from_utf8(projected).expect("utf8 config");
        assert!(text.contains("base_url = \"http://127.0.0.1:18787/v1\""));
        assert!(!text.contains("model_instructions_file"));
        assert!(text.contains("model = \"old-model\""));
    }

    #[test]
    fn codex_route_active_follows_the_effective_live_provider_base_url() {
        let directory = isolated_test_path("codex-x-gateway-route");
        let _ = fs::remove_dir_all(&directory);
        ensure_directory(&directory).expect("route directory");
        let config = directory.join("config.toml");
        let meta = GatewayModeMeta {
            version: 1,
            desired_mode: "gateway".to_string(),
            codex_dir: directory.to_string_lossy().into_owned(),
            listen_port: 18787,
            projected_config_sha256: String::new(),
            original_config_file: String::new(),
            original_auth_file: String::new(),
            original_instruction_file: None,
            original_instruction_backup: None,
            original_agents_file: None,
            original_agents_backup: None,
            instruction_mode: None,
        };

        atomic_write(
            &config,
            br#"model_provider = "custom"
base_url = "https://top-level.example/v1"

[model_providers.custom]
base_url = "http://127.0.0.1:18787/v1"
"#,
        )
        .expect("active route config");
        assert!(codex_route_active(Some(&meta), 18787));

        atomic_write(
            &config,
            br#"model_provider = "custom"
base_url = "http://127.0.0.1:18787/v1"

[model_providers.custom]
base_url = "https://provider.example/v1"
"#,
        )
        .expect("external provider config");
        assert!(!codex_route_active(Some(&meta), 18787));

        let _ = fs::remove_dir_all(&directory);
    }

    #[test]
    fn runtime_provider_projection_updates_model_without_touching_unrelated_fields() {
        let path = PathBuf::from("config.toml");
        let original = br#"model_provider = "custom"
model = "old-model"
other = true

[model_providers.custom]
base_url = "https://old.example/v1"
"#;
        let projected = project_runtime_provider(
            &path,
            original,
            &json!({"base_url": "https://new.example/v1", "model": "new-model"}),
        )
        .expect("project provider");
        let text = String::from_utf8(projected).expect("utf8 config");
        assert!(text.contains("base_url = \"https://new.example/v1\""));
        assert!(text.contains("model = \"new-model\""));
        assert!(text.contains("other = true"));
    }

    #[test]
    fn runtime_provider_projection_updates_every_owned_provider_field() {
        let path = PathBuf::from("config.toml");
        let original = br#"model_provider = "custom"
model = "old-model"

[model_providers.custom]
name = "Old"
base_url = "https://old.example/v1"
wire_api = "completions"
requires_openai_auth = true
"#;
        let projected = project_runtime_provider(
            &path,
            original,
            &json!({
                "provider_id": "custom",
                "provider_name": "New",
                "base_url": "https://new.example/v1",
                "wire_api": "responses",
                "requires_openai_auth": false,
                "model": "new-model"
            }),
        )
        .expect("project provider");
        let text = String::from_utf8(projected).expect("utf8 config");
        assert!(text.contains("model_provider = \"custom\""));
        assert!(text.contains("model = \"new-model\""));
        assert!(text.contains("name = \"New\""));
        assert!(text.contains("base_url = \"https://new.example/v1\""));
        assert!(text.contains("wire_api = \"responses\""));
        assert!(text.contains("requires_openai_auth = false"));
    }

    #[test]
    fn runtime_provider_projection_rejects_missing_or_invalid_provider_url() {
        let path = PathBuf::from("config.toml");
        let error = project_runtime_provider(&path, b"model_provider = \"custom\"\n", &json!({}))
            .expect_err("missing base url must fail");
        assert!(error
            .to_string()
            .contains("DIRECT_CONFIG_INVALID_AFTER_WRITE"));

        let error = project_runtime_provider(
            &path,
            b"not = [valid\n",
            &json!({"base_url": "https://new.example"}),
        )
        .expect_err("invalid toml must fail");
        assert!(error
            .to_string()
            .contains("DIRECT_CONFIG_INVALID_AFTER_WRITE"));
    }

    #[test]
    fn runtime_auth_updates_only_the_owned_api_key() {
        let current = br#"{
  "OPENAI_API_KEY": "old-key",
  "tokens": {"access_token": "keep-token"},
  "custom": true
}"#;
        let projected = runtime_auth_bytes(
            &json!({"api_key": "new-key", "requires_openai_auth": false}),
            Some(current),
        )
        .expect("project auth")
        .expect("auth remains present");
        let value: Value = serde_json::from_slice(&projected).expect("valid auth json");
        assert_eq!(value["OPENAI_API_KEY"], "new-key");
        assert_eq!(value["tokens"]["access_token"], "keep-token");
        assert_eq!(value["custom"], true);
    }

    #[test]
    fn runtime_auth_removes_only_the_owned_api_key_for_official_provider() {
        let current = br#"{"OPENAI_API_KEY":"proxy-key","tokens":{"refresh_token":"keep"}}"#;
        let projected = runtime_auth_bytes(
            &json!({"provider_id": "openai-official", "requires_openai_auth": true}),
            Some(current),
        )
        .expect("project auth")
        .expect("official auth remains present");
        let value: Value = serde_json::from_slice(&projected).expect("valid auth json");
        assert!(value.get("OPENAI_API_KEY").is_none());
        assert_eq!(value["tokens"]["refresh_token"], "keep");
    }

    #[test]
    fn runtime_auth_handles_missing_file_and_rejects_malformed_existing_file() {
        let created = runtime_auth_bytes(&json!({"api_key": "created-key"}), None)
            .expect("create auth")
            .expect("auth file should be created");
        let created_value: Value = serde_json::from_slice(&created).expect("valid created auth");
        assert_eq!(created_value["OPENAI_API_KEY"], "created-key");

        let error = runtime_auth_bytes(&json!({"api_key": "new-key"}), Some(b"not-json"))
            .expect_err("malformed auth must fail safely");
        assert!(error
            .to_string()
            .contains("DIRECT_CONFIG_INVALID_AFTER_WRITE"));
    }

    #[test]
    fn direct_config_guard_blocks_only_the_managed_codex_directory() {
        let mode = mode_dir().expect("mode dir");
        ensure_directory(&mode).expect("mode directory");
        let codex_dir = isolated_test_path("codex-x-gateway-guard");
        ensure_directory(&codex_dir).expect("codex directory");
        let meta = GatewayModeMeta {
            version: 1,
            desired_mode: "gateway".to_string(),
            codex_dir: codex_dir.to_string_lossy().into_owned(),
            listen_port: 18787,
            projected_config_sha256: String::new(),
            original_config_file: String::new(),
            original_auth_file: String::new(),
            original_instruction_file: None,
            original_instruction_backup: None,
            original_agents_file: None,
            original_agents_backup: None,
            instruction_mode: None,
        };
        write_mode_meta(&meta).expect("write managed mode");
        let blocked = ensure_direct_config_write_allowed(Some(&codex_dir.to_string_lossy()));
        assert!(blocked
            .expect_err("managed directory must be blocked")
            .to_string()
            .contains("GATEWAY_CONFIG_WRITE_BLOCKED"));

        let other_dir = isolated_test_path("codex-x-gateway-guard-other");
        ensure_directory(&other_dir).expect("other directory");
        ensure_direct_config_write_allowed(Some(&other_dir.to_string_lossy()))
            .expect("other directory remains direct");
        let _ = fs::remove_file(mode_meta_path().expect("mode metadata path"));
        let _ = fs::remove_dir_all(codex_dir);
        let _ = fs::remove_dir_all(other_dir);
    }

    #[test]
    fn persistent_gateway_identity_requires_matching_mode_and_loopback_state() {
        let meta = GatewayModeMeta {
            version: 1,
            desired_mode: "gateway".to_string(),
            codex_dir: "C:/tmp/codex".to_string(),
            listen_port: 18787,
            projected_config_sha256: "hash".to_string(),
            original_config_file: "config".to_string(),
            original_auth_file: "auth".to_string(),
            original_instruction_file: None,
            original_instruction_backup: None,
            original_agents_file: None,
            original_agents_backup: None,
            instruction_mode: None,
        };
        let state = json!({"state":"gateway","listen":"127.0.0.1:18787","process_id":1234});
        assert!(persistently_owned_gateway(Some(&meta), 18787, &state));
        assert!(!persistently_owned_gateway(Some(&meta), 18788, &state));
        assert!(!persistently_owned_gateway(
            Some(&meta),
            18787,
            &json!({"state":"gateway","listen":"127.0.0.1:18787"})
        ));
        assert!(!persistently_owned_gateway(
            Some(&meta),
            18787,
            &json!({"state":"direct","listen":"127.0.0.1:18787","process_id":1234})
        ));
    }

    #[test]
    fn recovery_upstream_prefers_watchdog_intent_then_runtime_provider() {
        let runtime = json!({"provider":{"base_url":"https://runtime.example"}});
        assert_eq!(
            recovery_upstream(None, Some(&runtime)).as_deref(),
            Some("https://runtime.example")
        );
        let intent = json!({"upstream":"https://intent.example"});
        assert_eq!(
            recovery_upstream(Some(&intent), Some(&runtime)).as_deref(),
            Some("https://intent.example")
        );
        assert!(recovery_upstream(None, Some(&json!({"provider":{}}))).is_none());
    }

    #[test]
    fn watchdog_task_xml_carries_intent_and_hidden_restart_settings() {
        let input = GatewayStartInput {
            listen_host: "127.0.0.1".to_string(),
            listen_port: 18787,
            upstream: "https://provider.example/v1?mode=a&value=<test>".to_string(),
            config_dir: None,
        };
        let xml = watchdog_task_xml(
            &input,
            Path::new("C:/Users/Test & QA/.codexx/gateway-mode/watchdog-intent.json"),
            Path::new("C:/Codex-X-Pro/Test & QA/codex_responses_repair_watchdog.ps1"),
        );
        assert!(xml.contains("<Hidden>true</Hidden>"));
        assert!(xml.contains("<Enabled>true</Enabled>"));
        assert!(xml.contains("<LogonType>InteractiveToken</LogonType>"));
        assert!(xml.contains(
            "<RestartOnFailure><Count>999</Count><Interval>PT1M</Interval></RestartOnFailure>"
        ));
        assert!(xml.contains("<Command>powershell.exe</Command>"));
        assert!(xml.contains("-StateFile &quot;C:/Users/Test &amp; QA/.codexx/gateway-mode/watchdog-intent.json&quot;"));
        assert!(xml.contains("-WindowStyle Hidden"));
        assert!(xml.contains("mode=a&amp;value=&lt;test&gt;"));
        assert!(!xml.contains("/TR"));
    }

    #[test]
    fn project_watchdog_task_does_not_reuse_the_personal_gateway_task_name() {
        assert_eq!(WATCHDOG_TASK_NAME, "Codex-X-Pro Local Gateway");
        assert_ne!(WATCHDOG_TASK_NAME, "Codex Responses Repair Gateway");
    }

    #[test]
    fn watchdog_task_xml_has_no_schtasks_action_length_limit() {
        let input = GatewayStartInput {
            listen_host: "127.0.0.1".to_string(),
            listen_port: 18788,
            upstream: format!("https://provider.example/{}", "long-segment/".repeat(40)),
            config_dir: None,
        };
        let xml = watchdog_task_xml(
            &input,
            Path::new("C:/Users/Test/.codexx/gateway-mode/watchdog-intent.json"),
            Path::new(
                "C:/Users/Test/.codex-x/personal-gateway/codex_responses_repair_watchdog.ps1",
            ),
        );
        let arguments = xml
            .split_once("<Arguments>")
            .and_then(|(_, rest)| rest.split_once("</Arguments>"))
            .map(|(arguments, _)| arguments)
            .expect("task arguments");
        assert!(arguments.len() > 261);
        assert!(xml.contains("<Actions Context=\"Author\">"));
    }

    #[test]
    fn watchdog_task_xml_is_written_as_utf16le_with_bom() {
        let bytes = utf16le_with_bom("<?xml version=\"1.0\" encoding=\"UTF-16\"?>");
        assert_eq!(&bytes[..2], &[0xff, 0xfe]);
        assert_eq!(bytes.len() % 2, 0);
        let units = bytes[2..]
            .chunks_exact(2)
            .map(|chunk| u16::from_le_bytes([chunk[0], chunk[1]]))
            .collect::<Vec<_>>();
        assert_eq!(
            String::from_utf16(&units).expect("decode UTF-16LE"),
            "<?xml version=\"1.0\" encoding=\"UTF-16\"?>"
        );
    }

    #[test]
    fn failed_start_snapshot_restores_replaced_and_new_state_files() {
        let directory = isolated_test_path("codex-x-gateway-start-snapshot");
        let _ = fs::remove_dir_all(&directory);
        ensure_directory(&directory).expect("snapshot directory");
        let state_path = directory.join("state.json");
        let runtime_path = directory.join("runtime-state.json");
        let intent_path = directory.join("watchdog-intent.json");
        atomic_write(&state_path, b"old-state").expect("old state");
        atomic_write(&runtime_path, b"old-runtime").expect("old runtime");
        let snapshot = GatewayStartStateSnapshot::capture(&directory).expect("capture snapshot");

        atomic_write(&state_path, b"new-state").expect("new state");
        atomic_write(&runtime_path, b"new-runtime").expect("new runtime");
        atomic_write(&intent_path, b"new-intent").expect("new intent");
        snapshot.restore();

        assert_eq!(fs::read(&state_path).expect("restored state"), b"old-state");
        assert_eq!(
            fs::read(&runtime_path).expect("restored runtime"),
            b"old-runtime"
        );
        assert!(!intent_path.exists());
        let _ = fs::remove_dir_all(&directory);
    }

    #[test]
    fn failed_start_restores_managed_agents_content() {
        let directory = isolated_test_path("codex-x-gateway-agents");
        let _ = fs::remove_dir_all(&directory);
        ensure_directory(&directory).expect("agents directory");
        let path = directory.join("AGENTS.md");
        let original = b"before\nmanaged\nafter\n".to_vec();
        let without_managed = b"before\nafter\n".to_vec();
        atomic_write(&path, &without_managed).expect("projected agents");
        let initial = (
            path.clone(),
            "managed".to_string(),
            original.clone(),
            without_managed,
        );

        restore_agents_after_failed_start(Some(&initial));

        assert_eq!(fs::read(&path).expect("restored agents"), original);
        let _ = fs::remove_dir_all(&directory);
    }

    #[cfg(target_os = "windows")]
    #[test]
    #[ignore = "creates and deletes an isolated current-user Windows scheduled task"]
    fn windows_schtasks_accepts_generated_watchdog_xml() {
        let task_name = format!(
            "Codex-X-Pro Gateway XML Test {} {}",
            std::process::id(),
            isolated_test_path("id")
                .file_name()
                .and_then(|value| value.to_str())
                .unwrap_or("1")
        );
        let directory = isolated_test_path("codex-x-gateway-task-xml");
        ensure_directory(&directory).expect("task XML directory");
        let xml_path = directory.join("task.xml");
        let input = GatewayStartInput {
            listen_host: "127.0.0.1".to_string(),
            listen_port: 18788,
            upstream: format!("https://provider.example/{}", "long-segment/".repeat(40)),
            config_dir: None,
        };
        let xml = watchdog_task_xml_for_name(
            &task_name,
            &input,
            Path::new("C:/Users/Test/.codexx/gateway-mode/watchdog-intent.json"),
            Path::new(
                "C:/Users/Test/.codex-x/personal-gateway/codex_responses_repair_watchdog.ps1",
            ),
        );
        atomic_write(&xml_path, &utf16le_with_bom(&xml)).expect("write task XML");

        let result = (|| -> Result<String> {
            let old_input = GatewayStartInput {
                listen_host: "127.0.0.1".to_string(),
                listen_port: 18787,
                upstream: "https://old-provider.example".to_string(),
                config_dir: None,
            };
            let old_xml = watchdog_task_xml_for_name(
                &task_name,
                &old_input,
                Path::new("C:/Users/Test/.codexx/gateway-mode/old-intent.json"),
                Path::new("C:/Users/Test/.codex-x/personal-gateway/old-watchdog.ps1"),
            );
            let old_xml_path = directory.join("old-task.xml");
            atomic_write(&old_xml_path, &utf16le_with_bom(&old_xml))?;
            create_watchdog_task(&task_name, &old_xml_path)?;
            let snapshot = capture_scheduled_task(&task_name)?;
            if snapshot
                .xml
                .as_deref()
                .is_none_or(|value| !value.contains("old-watchdog.ps1"))
            {
                return Err(CodexxError::Config(
                    "test task snapshot did not capture old action".to_string(),
                ));
            }

            create_watchdog_task(&task_name, &xml_path)?;
            let output = program_command(
                Path::new("schtasks.exe"),
                &["/Query", "/TN", &task_name, "/XML"],
            )
            .output()
            .map_err(|error| CodexxError::Config(format!("query test task: {error}")))?;
            if !output.status.success() {
                return Err(CodexxError::Config(command_output_summary(&output)));
            }
            let new_xml = decode_command_bytes(&output.stdout);
            if !new_xml.contains("codex_responses_repair_watchdog.ps1") {
                return Err(CodexxError::Config(
                    "test task was not replaced".to_string(),
                ));
            }

            restore_scheduled_task_from_directory(&task_name, &snapshot, &directory)?;
            query_scheduled_task_xml(&task_name)?
                .ok_or_else(|| CodexxError::Config("restored test task was not found".to_string()))
        })();

        let _ = program_command(
            Path::new("schtasks.exe"),
            &["/Delete", "/TN", &task_name, "/F"],
        )
        .output();
        let _ = fs::remove_dir_all(&directory);
        let queried_xml = result.expect("create, query, and restore isolated task");
        assert!(queried_xml.contains("<Hidden>true</Hidden>"));
        assert!(queried_xml.contains("old-watchdog.ps1"));
        assert!(queried_xml.contains("old-intent.json"));
        assert!(queried_xml.contains("-WindowStyle Hidden"));
    }

    #[test]
    fn startup_initialization_is_noop_without_gateway_mode_snapshot() {
        let path = mode_meta_path().expect("mode path");
        let _ = fs::remove_file(path);
        initialize_on_startup().expect("missing snapshot should not block startup");
    }

    #[test]
    fn shutdown_on_exit_is_noop_without_gateway_mode_snapshot() {
        let path = mode_meta_path().expect("mode path");
        let _ = fs::remove_file(path);
        shutdown_on_exit().expect("missing snapshot should not block exit");
    }

    #[test]
    fn startup_recovers_gateway_and_adopts_it_after_process_handle_loss() {
        let listener = TcpListener::bind(("127.0.0.1", 0)).expect("reserve port");
        let port = listener.local_addr().expect("local addr").port();
        drop(listener);
        let directory = mode_dir().expect("mode dir");
        let _ = fs::remove_dir_all(&directory);
        ensure_directory(&directory).expect("create mode dir");
        let runtime_path = directory.join("runtime-state.json");
        atomic_write(&runtime_path, serde_json::to_vec_pretty(&json!({
            "provider": {"base_url": "http://127.0.0.1:9", "provider_id": "test", "provider_name": "Test", "model": "test", "wire_api": "responses"},
            "instruction": {"enabled": false, "content": "", "injection_mode": "append"}
        })).expect("runtime json").as_slice()).expect("runtime state");
        let meta = GatewayModeMeta {
            version: 1,
            desired_mode: "gateway".to_string(),
            codex_dir: directory.join("codex-home").to_string_lossy().into_owned(),
            listen_port: port,
            projected_config_sha256: "test".to_string(),
            original_config_file: directory
                .join("original-config.toml")
                .to_string_lossy()
                .into_owned(),
            original_auth_file: directory
                .join("original-auth.json")
                .to_string_lossy()
                .into_owned(),
            original_instruction_file: None,
            original_instruction_backup: None,
            original_agents_file: None,
            original_agents_backup: None,
            instruction_mode: None,
        };
        write_mode_meta(&meta).expect("mode meta");
        set_watchdog_intent("gateway", false, port, "http://127.0.0.1:9").expect("intent");
        let outcome = (|| -> Result<()> {
            initialize_on_startup()?;
            let first = process_state(port);
            if !first.running || !first.managed_by_codex_x || first.process_id.is_none() {
                return Err(CodexxError::Config(
                    "startup did not manage recovered gateway".to_string(),
                ));
            }
            let child = child_slot()
                .lock()
                .unwrap_or_else(|error| error.into_inner())
                .take();
            drop(child);
            let adopted = process_state(port);
            if !adopted.running
                || !adopted.managed_by_codex_x
                || adopted.process_id != first.process_id
            {
                return Err(CodexxError::Config(
                    "persisted gateway was not adopted after handle loss".to_string(),
                ));
            }
            terminate_persisted_gateway(&meta)?;
            if gateway_health(port).is_ok() {
                return Err(CodexxError::Config(
                    "persisted gateway remained healthy after termination".to_string(),
                ));
            }
            Ok(())
        })();
        let _ = terminate_persisted_gateway(&meta);
        let _ = fs::remove_dir_all(&directory);
        outcome.expect("startup recovery lifecycle");
    }
}
