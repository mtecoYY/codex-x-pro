use crate::error::{CodexxError, Result};
use crate::now_rfc3339;
use crate::paths::normalized_path_scope;
use rusqlite::{params, Connection, OptionalExtension};
use std::collections::HashSet;
use std::path::Path;

fn selected_provider_id_on_connection(
    conn: &Connection,
    codex_dir: &Path,
) -> Result<Option<String>> {
    conn.query_row(
        "SELECT provider_id
         FROM active_provider_selections
         WHERE codex_dir = ?1",
        [normalized_path_scope(codex_dir)],
        |row| row.get(0),
    )
    .optional()
    .map_err(|error| CodexxError::Database(error.to_string()))
}

pub(crate) fn remember_active_provider_on_connection(
    conn: &Connection,
    codex_dir: &Path,
    provider_id: &str,
) -> Result<()> {
    let provider_id = provider_id.trim();
    if provider_id.is_empty() {
        return Err(CodexxError::Config("当前供应商 ID 不能为空".to_string()));
    }
    let exists = conn
        .query_row(
            "SELECT EXISTS(SELECT 1 FROM providers WHERE id = ?1)",
            [provider_id],
            |row| row.get::<_, bool>(0),
        )
        .map_err(|error| CodexxError::Database(error.to_string()))?;
    if !exists {
        return Err(CodexxError::Config(format!(
            "无法记录当前供应商，未找到 ID {provider_id}"
        )));
    }
    conn.execute(
        "INSERT INTO active_provider_selections (codex_dir, provider_id, updated_at)
         VALUES (?1, ?2, ?3)
         ON CONFLICT(codex_dir) DO UPDATE SET
            provider_id = excluded.provider_id,
            updated_at = excluded.updated_at",
        params![normalized_path_scope(codex_dir), provider_id, now_rfc3339()],
    )
    .map_err(|error| CodexxError::Database(error.to_string()))?;
    Ok(())
}

pub(crate) fn clear_active_provider_on_connection(
    conn: &Connection,
    codex_dir: &Path,
) -> Result<()> {
    conn.execute(
        "DELETE FROM active_provider_selections WHERE codex_dir = ?1",
        [normalized_path_scope(codex_dir)],
    )
    .map_err(|error| CodexxError::Database(error.to_string()))?;
    Ok(())
}

pub(crate) fn clear_provider_selections_on_connection(
    conn: &Connection,
    provider_id: &str,
) -> Result<()> {
    conn.execute(
        "DELETE FROM active_provider_selections WHERE provider_id = ?1",
        [provider_id],
    )
    .map_err(|error| CodexxError::Database(error.to_string()))?;
    Ok(())
}

pub(crate) fn reconcile_active_provider_on_connection(
    conn: &Connection,
    codex_dir: &Path,
    candidate_ids: &[String],
) -> Result<Option<String>> {
    let candidates = candidate_ids
        .iter()
        .map(|id| id.trim())
        .filter(|id| !id.is_empty())
        .collect::<HashSet<_>>();
    let selected = selected_provider_id_on_connection(conn, codex_dir)?;
    if let Some(selected) = selected.as_deref() {
        if candidates.contains(selected) {
            return Ok(Some(selected.to_string()));
        }
        clear_active_provider_on_connection(conn, codex_dir)?;
    }

    if candidates.len() != 1 {
        return Ok(None);
    }
    let provider_id = candidates
        .into_iter()
        .next()
        .expect("one candidate must be present");
    remember_active_provider_on_connection(conn, codex_dir, provider_id)?;
    Ok(Some(provider_id.to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn test_connection() -> Connection {
        let conn = Connection::open_in_memory().expect("open selection test database");
        conn.execute_batch(
            "CREATE TABLE providers (id TEXT PRIMARY KEY);
             CREATE TABLE active_provider_selections (
                codex_dir TEXT PRIMARY KEY,
                provider_id TEXT NOT NULL,
                updated_at TEXT NOT NULL
             );
             INSERT INTO providers (id) VALUES ('original'), ('copy'), ('other');",
        )
        .expect("create selection test schema");
        conn
    }

    fn scope(name: &str) -> PathBuf {
        std::env::temp_dir().join(format!("codex-x-selection-{name}"))
    }

    #[test]
    fn exact_duplicate_does_not_replace_the_remembered_current_provider() {
        let conn = test_connection();
        let codex_dir = scope("duplicate");
        remember_active_provider_on_connection(&conn, &codex_dir, "original")
            .expect("remember original provider");

        let active = reconcile_active_provider_on_connection(
            &conn,
            &codex_dir,
            &["original".to_string(), "copy".to_string()],
        )
        .expect("reconcile duplicate profiles");

        assert_eq!(active.as_deref(), Some("original"));
    }

    #[test]
    fn selections_are_independent_for_each_codex_directory() {
        let conn = test_connection();
        let first = scope("first");
        let second = scope("second");
        remember_active_provider_on_connection(&conn, &first, "original")
            .expect("remember first selection");
        remember_active_provider_on_connection(&conn, &second, "copy")
            .expect("remember second selection");
        let candidates = ["original".to_string(), "copy".to_string()];

        assert_eq!(
            reconcile_active_provider_on_connection(&conn, &first, &candidates)
                .expect("reconcile first selection")
                .as_deref(),
            Some("original")
        );
        assert_eq!(
            reconcile_active_provider_on_connection(&conn, &second, &candidates)
                .expect("reconcile second selection")
                .as_deref(),
            Some("copy")
        );
    }

    #[test]
    fn stale_selection_falls_back_only_when_the_live_match_is_unique() {
        let conn = test_connection();
        let codex_dir = scope("stale");
        remember_active_provider_on_connection(&conn, &codex_dir, "original")
            .expect("remember stale selection");

        assert_eq!(
            reconcile_active_provider_on_connection(&conn, &codex_dir, &["other".to_string()])
                .expect("reconcile unique replacement")
                .as_deref(),
            Some("other")
        );
        assert_eq!(
            selected_provider_id_on_connection(&conn, &codex_dir)
                .expect("read repaired selection")
                .as_deref(),
            Some("other")
        );

        assert!(
            reconcile_active_provider_on_connection(&conn, &codex_dir, &[])
                .expect("clear unmatched selection")
                .is_none()
        );
        assert!(selected_provider_id_on_connection(&conn, &codex_dir)
            .expect("read cleared selection")
            .is_none());
    }
}
