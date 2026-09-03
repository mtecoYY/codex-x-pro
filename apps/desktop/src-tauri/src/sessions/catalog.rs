use crate::error::{CodexxError, Result};
use crate::sqlite_utils::{sqlite_has_table, table_column_set};
use rusqlite::{params_from_iter, types::Value as SqlValue, Connection, OpenFlags};
use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

const CATALOG_TABLE: &str = "local_thread_catalog";

#[derive(Debug, Clone)]
pub(super) struct CatalogRepairThread {
    id: String,
    display_title: String,
    source_created_at: f64,
    source_updated_at: f64,
    source_recency_at: f64,
    cwd: String,
    source_kind: String,
    source_detail: String,
    model_provider: String,
    git_branch: Option<String>,
    thread_source: Option<String>,
}

#[derive(Debug, Default)]
pub(super) struct CatalogSyncScan {
    pub(super) sources: HashMap<String, CatalogRepairThread>,
    pub(super) mismatched_thread_ids: HashSet<String>,
    pub(super) mismatched_rows: usize,
    pub(super) missing_rows: usize,
}

impl CatalogSyncScan {
    pub(super) fn total_updates(&self) -> usize {
        self.mismatched_rows + self.missing_rows
    }
}

#[derive(Debug, Default)]
pub(super) struct CatalogUpdateCounts {
    pub(super) provider_rows: usize,
    pub(super) inserted_rows: usize,
}

fn database_error(error: rusqlite::Error) -> CodexxError {
    CodexxError::Database(error.to_string())
}

fn text_expr(columns: &HashSet<String>, column: &str, fallback: &str) -> String {
    if columns.contains(column) {
        format!("COALESCE({column}, {fallback})")
    } else {
        fallback.to_string()
    }
}

fn coalesce_text_expr(columns: &HashSet<String>, candidates: &[&str], fallback: &str) -> String {
    let mut parts = candidates
        .iter()
        .filter(|column| columns.contains(**column))
        .map(|column| format!("NULLIF({column}, '')"))
        .collect::<Vec<_>>();
    parts.push(fallback.to_string());
    if parts.len() == 1 {
        parts.remove(0)
    } else {
        format!("COALESCE({})", parts.join(", "))
    }
}

fn timestamp_expr(columns: &HashSet<String>, ms_column: &str, seconds_column: &str) -> String {
    if columns.contains(ms_column) {
        format!("COALESCE({ms_column} / 1000.0, 0)")
    } else if columns.contains(seconds_column) {
        format!(
            "CASE WHEN COALESCE({seconds_column}, 0) > 9999999999 \
             THEN {seconds_column} / 1000.0 ELSE COALESCE({seconds_column}, 0) END"
        )
    } else {
        "0".to_string()
    }
}

fn collect_catalog_repair_threads(
    paths: &[PathBuf],
    target_provider: &str,
    syncable_thread_ids: &HashSet<String>,
) -> Result<HashMap<String, CatalogRepairThread>> {
    let mut threads = HashMap::new();
    for path in paths {
        let conn = Connection::open_with_flags(
            path,
            OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX,
        )
        .map_err(database_error)?;
        if !sqlite_has_table(&conn, "threads")? {
            continue;
        }
        let columns = table_column_set(&conn, "threads")?;
        if !columns.contains("id") {
            continue;
        }
        let display_title = coalesce_text_expr(
            &columns,
            &["name", "title", "preview", "first_user_message"],
            "id",
        );
        let source_created_at = timestamp_expr(&columns, "created_at_ms", "created_at");
        let source_updated_at = timestamp_expr(&columns, "updated_at_ms", "updated_at");
        let source_recency_at = timestamp_expr(&columns, "recency_at_ms", "recency_at");
        let source_recency_at = if source_recency_at == "0" {
            source_updated_at.clone()
        } else {
            format!("COALESCE(NULLIF({source_recency_at}, 0), {source_updated_at})")
        };
        let cwd = text_expr(&columns, "cwd", "''");
        let source_kind = coalesce_text_expr(&columns, &["source"], "'cli'");
        let source_detail = text_expr(&columns, "rollout_path", "''");
        let git_branch = text_expr(&columns, "git_branch", "NULL");
        let thread_source = text_expr(&columns, "thread_source", "NULL");
        let archived_filter = if columns.contains("archived") {
            " AND COALESCE(archived, 0) = 0"
        } else {
            ""
        };
        let sql = format!(
            "SELECT id, {display_title}, {source_created_at}, {source_updated_at}, \
             {source_recency_at}, {cwd}, {source_kind}, {source_detail}, {git_branch}, \
             {thread_source} FROM threads WHERE COALESCE(id, '') <> ''{archived_filter}"
        );
        let mut statement = conn.prepare(&sql).map_err(database_error)?;
        let rows = statement
            .query_map([], |row| {
                Ok(CatalogRepairThread {
                    id: row.get(0)?,
                    display_title: row.get::<_, String>(1).unwrap_or_default(),
                    source_created_at: row.get::<_, f64>(2).unwrap_or_default(),
                    source_updated_at: row.get::<_, f64>(3).unwrap_or_default(),
                    source_recency_at: row.get::<_, f64>(4).unwrap_or_default(),
                    cwd: row.get::<_, String>(5).unwrap_or_default(),
                    source_kind: row
                        .get::<_, String>(6)
                        .unwrap_or_else(|_| "cli".to_string()),
                    source_detail: row.get::<_, String>(7).unwrap_or_default(),
                    model_provider: target_provider.to_string(),
                    git_branch: row.get::<_, Option<String>>(8).unwrap_or(None),
                    thread_source: row.get::<_, Option<String>>(9).unwrap_or(None),
                })
            })
            .map_err(database_error)?;
        for row in rows {
            let thread = row.map_err(database_error)?;
            if !syncable_thread_ids.contains(&thread.id) {
                continue;
            }
            let replace = threads
                .get(&thread.id)
                .map(|current: &CatalogRepairThread| {
                    thread.source_updated_at > current.source_updated_at
                })
                .unwrap_or(true);
            if replace {
                threads.insert(thread.id.clone(), thread);
            }
        }
    }
    Ok(threads)
}

fn catalog_supports_repair(columns: &HashSet<String>) -> bool {
    [
        "host_id",
        "thread_id",
        "display_title",
        "source_created_at",
        "source_updated_at",
        "cwd",
        "source_kind",
        "model_provider",
        "observation_sequence",
    ]
    .iter()
    .all(|column| columns.contains(*column))
}

fn local_catalog_host_id(conn: &Connection) -> Result<String> {
    if !sqlite_has_table(conn, "local_thread_catalog_hosts")? {
        return Ok("local".to_string());
    }
    let columns = table_column_set(conn, "local_thread_catalog_hosts")?;
    if !columns.contains("host_id") {
        return Ok("local".to_string());
    }
    let order = if columns.contains("host_kind") {
        "CASE WHEN host_id = 'local' THEN 0 WHEN host_kind = 'local' THEN 1 ELSE 2 END, host_id"
    } else {
        "CASE WHEN host_id = 'local' THEN 0 ELSE 1 END, host_id"
    };
    match conn.query_row(
        &format!("SELECT host_id FROM local_thread_catalog_hosts ORDER BY {order} LIMIT 1"),
        [],
        |row| row.get::<_, String>(0),
    ) {
        Ok(host_id) if !host_id.trim().is_empty() => Ok(host_id),
        Ok(_) | Err(rusqlite::Error::QueryReturnedNoRows) => Ok("local".to_string()),
        Err(error) => Err(database_error(error)),
    }
}

fn catalog_existing_thread_ids(
    conn: &Connection,
    columns: &HashSet<String>,
    host_id: &str,
) -> Result<HashSet<String>> {
    let visible_filter = if columns.contains("missing_candidate") {
        " AND COALESCE(missing_candidate, 0) = 0"
    } else {
        ""
    };
    let mut statement = conn
        .prepare(&format!(
            "SELECT thread_id FROM local_thread_catalog WHERE host_id = ?1{visible_filter}"
        ))
        .map_err(database_error)?;
    let rows = statement
        .query_map([host_id], |row| row.get::<_, String>(0))
        .map_err(database_error)?;
    let mut ids = HashSet::new();
    for row in rows {
        ids.insert(row.map_err(database_error)?);
    }
    Ok(ids)
}

pub(super) fn scan_catalog_sync(
    thread_paths: &[PathBuf],
    catalog_paths: &[PathBuf],
    target_provider: &str,
    syncable_thread_ids: &HashSet<String>,
) -> Result<CatalogSyncScan> {
    let sources =
        collect_catalog_repair_threads(thread_paths, target_provider, syncable_thread_ids)?;
    let mut scan = CatalogSyncScan {
        sources,
        ..CatalogSyncScan::default()
    };
    for path in catalog_paths {
        let conn = Connection::open_with_flags(
            path,
            OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX,
        )
        .map_err(database_error)?;
        if !sqlite_has_table(&conn, CATALOG_TABLE)? {
            continue;
        }
        let columns = table_column_set(&conn, CATALOG_TABLE)?;
        if columns.contains("model_provider") && columns.contains("thread_id") {
            let mut statement = conn
                .prepare(
                    "SELECT thread_id FROM local_thread_catalog \
                     WHERE COALESCE(model_provider, '') <> ?1",
                )
                .map_err(database_error)?;
            let rows = statement
                .query_map([target_provider], |row| row.get::<_, String>(0))
                .map_err(database_error)?;
            for row in rows {
                let id = row.map_err(database_error)?;
                if syncable_thread_ids.contains(&id) {
                    scan.mismatched_rows += 1;
                    scan.mismatched_thread_ids.insert(id);
                }
            }
        }
        if !catalog_supports_repair(&columns) {
            continue;
        }
        let host_id = local_catalog_host_id(&conn)?;
        let existing_ids = catalog_existing_thread_ids(&conn, &columns, &host_id)?;
        for id in scan.sources.keys() {
            if !existing_ids.contains(id) {
                scan.missing_rows += 1;
                scan.mismatched_thread_ids.insert(id.clone());
            }
        }
    }
    Ok(scan)
}

pub(super) fn catalog_columns(conn: &Connection) -> Result<HashSet<String>> {
    if sqlite_has_table(conn, CATALOG_TABLE)? {
        table_column_set(conn, CATALOG_TABLE)
    } else {
        Ok(HashSet::new())
    }
}

pub(super) fn create_catalog_rollback_tables(conn: &Connection) -> Result<()> {
    conn.execute_batch(
        "CREATE TEMP TABLE codexx_catalog_provider_rollback (
	            host_id TEXT NOT NULL,
	            thread_id TEXT NOT NULL,
	            model_provider TEXT,
	            missing_candidate INTEGER,
	            PRIMARY KEY (host_id, thread_id)
         );
         CREATE TEMP TABLE codexx_catalog_insert_rollback (
            host_id TEXT NOT NULL,
            thread_id TEXT NOT NULL,
            PRIMARY KEY (host_id, thread_id)
         );
         CREATE TEMP TABLE codexx_catalog_metadata_rollback (
            id INTEGER PRIMARY KEY,
            catalog_revision INTEGER
         );
         CREATE TEMP TABLE codexx_catalog_metadata_insert_rollback (
            id INTEGER PRIMARY KEY
         );
         CREATE TEMP TABLE codexx_catalog_sync_rollback (
            host_id TEXT PRIMARY KEY,
            watermark_updated_at REAL,
            initial_build_complete INTEGER,
            observation_sequence INTEGER,
            last_full_reconciled_at INTEGER
         );
         CREATE TEMP TABLE codexx_catalog_sync_insert_rollback (
            host_id TEXT PRIMARY KEY
         );",
    )
    .map_err(database_error)
}

fn local_catalog_max_observation_sequence(conn: &Connection) -> Result<i64> {
    conn.query_row(
        "SELECT COALESCE(MAX(observation_sequence), 0) FROM local_thread_catalog",
        [],
        |row| row.get(0),
    )
    .map_err(database_error)
}

fn local_catalog_insert_columns(columns: &HashSet<String>) -> Vec<&'static str> {
    let mut names = vec![
        "host_id",
        "thread_id",
        "display_title",
        "source_created_at",
        "source_updated_at",
        "cwd",
        "source_kind",
        "model_provider",
        "observation_sequence",
    ];
    for optional in [
        "source_detail",
        "missing_candidate",
        "git_branch",
        "thread_source",
        "source_recency_at",
    ] {
        if columns.contains(optional) {
            names.push(optional);
        }
    }
    names
}

fn local_catalog_insert_values(
    columns: &[&str],
    host_id: &str,
    thread: &CatalogRepairThread,
    observation_sequence: i64,
) -> Vec<SqlValue> {
    columns
        .iter()
        .map(|column| match *column {
            "host_id" => SqlValue::Text(host_id.to_string()),
            "thread_id" => SqlValue::Text(thread.id.clone()),
            "display_title" => SqlValue::Text(thread.display_title.clone()),
            "source_created_at" => SqlValue::Real(thread.source_created_at),
            "source_updated_at" => SqlValue::Real(thread.source_updated_at),
            "source_recency_at" => SqlValue::Real(thread.source_recency_at),
            "cwd" => SqlValue::Text(thread.cwd.clone()),
            "source_kind" => SqlValue::Text(thread.source_kind.clone()),
            "source_detail" => SqlValue::Text(thread.source_detail.clone()),
            "model_provider" => SqlValue::Text(thread.model_provider.clone()),
            "git_branch" => thread
                .git_branch
                .clone()
                .map(SqlValue::Text)
                .unwrap_or(SqlValue::Null),
            "thread_source" => thread
                .thread_source
                .clone()
                .map(SqlValue::Text)
                .unwrap_or(SqlValue::Null),
            "observation_sequence" => SqlValue::Integer(observation_sequence),
            "missing_candidate" => SqlValue::Integer(0),
            _ => SqlValue::Null,
        })
        .collect()
}

fn update_catalog_metadata(conn: &Connection, inserted: usize) -> Result<()> {
    if !sqlite_has_table(conn, "local_thread_catalog_metadata")? {
        return Ok(());
    }
    let columns = table_column_set(conn, "local_thread_catalog_metadata")?;
    if !columns.contains("catalog_revision") {
        return Ok(());
    }
    if !columns.contains("id") {
        return Err(CodexxError::Database(
            "local_thread_catalog_metadata 缺少 id 字段".to_string(),
        ));
    }
    conn.execute(
        "INSERT OR IGNORE INTO temp.codexx_catalog_metadata_rollback (id, catalog_revision) \
         SELECT id, catalog_revision FROM local_thread_catalog_metadata",
        [],
    )
    .map_err(database_error)?;
    let affected = conn
        .execute(
            "UPDATE local_thread_catalog_metadata \
             SET catalog_revision = catalog_revision + ?1",
            [inserted as i64],
        )
        .map_err(database_error)?;
    if affected == 0 {
        conn.execute(
            "INSERT INTO local_thread_catalog_metadata (id, catalog_revision) VALUES (1, ?1)",
            [inserted as i64],
        )
        .map_err(database_error)?;
        conn.execute(
            "INSERT INTO temp.codexx_catalog_metadata_insert_rollback (id) VALUES (1)",
            [],
        )
        .map_err(database_error)?;
    }
    Ok(())
}

fn now_secs() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_secs() as i64)
        .unwrap_or_default()
}

fn update_catalog_sync_state(
    conn: &Connection,
    host_id: &str,
    observation_sequence: i64,
    max_source_updated_at: f64,
) -> Result<()> {
    if !sqlite_has_table(conn, "local_thread_catalog_sync_state")? {
        return Ok(());
    }
    let columns = table_column_set(conn, "local_thread_catalog_sync_state")?;
    if !columns.contains("host_id") {
        return Err(CodexxError::Database(
            "local_thread_catalog_sync_state 缺少 host_id 字段".to_string(),
        ));
    }
    let snapshot_expr = |column: &str| {
        if columns.contains(column) {
            column.to_string()
        } else {
            "NULL".to_string()
        }
    };
    let snapshot_sql = format!(
        "INSERT OR IGNORE INTO temp.codexx_catalog_sync_rollback (\
            host_id, watermark_updated_at, initial_build_complete, observation_sequence, \
            last_full_reconciled_at) \
         SELECT host_id, {}, {}, {}, {} FROM local_thread_catalog_sync_state WHERE host_id = ?1",
        snapshot_expr("watermark_updated_at"),
        snapshot_expr("initial_build_complete"),
        snapshot_expr("observation_sequence"),
        snapshot_expr("last_full_reconciled_at"),
    );
    conn.execute(&snapshot_sql, [host_id])
        .map_err(database_error)?;

    let mut assignments = Vec::new();
    let mut values = Vec::new();
    if columns.contains("initial_build_complete") {
        assignments.push("initial_build_complete = 1");
    }
    if columns.contains("observation_sequence") {
        assignments.push("observation_sequence = MAX(COALESCE(observation_sequence, 0), ?)");
        values.push(SqlValue::Integer(observation_sequence));
    }
    if columns.contains("watermark_updated_at") {
        assignments.push("watermark_updated_at = MAX(COALESCE(watermark_updated_at, 0), ?)");
        values.push(SqlValue::Real(max_source_updated_at));
    }
    if columns.contains("last_full_reconciled_at") {
        assignments.push("last_full_reconciled_at = MAX(COALESCE(last_full_reconciled_at, 0), ?)");
        values.push(SqlValue::Integer(now_secs()));
    }
    if assignments.is_empty() {
        return Ok(());
    }
    let update_sql = format!(
        "UPDATE local_thread_catalog_sync_state SET {} WHERE host_id = ?",
        assignments.join(", ")
    );
    let mut update_values = values.clone();
    update_values.push(SqlValue::Text(host_id.to_string()));
    let affected = conn
        .execute(&update_sql, params_from_iter(update_values))
        .map_err(database_error)?;
    if affected > 0 {
        return Ok(());
    }

    let mut insert_columns = vec!["host_id"];
    let mut insert_values = vec![SqlValue::Text(host_id.to_string())];
    if columns.contains("watermark_updated_at") {
        insert_columns.push("watermark_updated_at");
        insert_values.push(SqlValue::Real(max_source_updated_at));
    }
    if columns.contains("initial_build_complete") {
        insert_columns.push("initial_build_complete");
        insert_values.push(SqlValue::Integer(1));
    }
    if columns.contains("observation_sequence") {
        insert_columns.push("observation_sequence");
        insert_values.push(SqlValue::Integer(observation_sequence));
    }
    if columns.contains("last_full_reconciled_at") {
        insert_columns.push("last_full_reconciled_at");
        insert_values.push(SqlValue::Integer(now_secs()));
    }
    let placeholders = std::iter::repeat_n("?", insert_columns.len())
        .collect::<Vec<_>>()
        .join(", ");
    let insert_sql = format!(
        "INSERT INTO local_thread_catalog_sync_state ({}) VALUES ({})",
        insert_columns.join(", "),
        placeholders
    );
    conn.execute(&insert_sql, params_from_iter(insert_values))
        .map_err(database_error)?;
    conn.execute(
        "INSERT INTO temp.codexx_catalog_sync_insert_rollback (host_id) VALUES (?1)",
        [host_id],
    )
    .map_err(database_error)?;
    Ok(())
}

fn snapshot_catalog_provider_changes(
    conn: &Connection,
    columns: &HashSet<String>,
    target_provider: &str,
    provider_thread_ids: &HashSet<String>,
) -> Result<()> {
    if !columns.contains("model_provider") || !columns.contains("thread_id") {
        return Ok(());
    }
    let host_expr = if columns.contains("host_id") {
        "host_id"
    } else {
        "''"
    };
    let missing_expr = if columns.contains("missing_candidate") {
        "missing_candidate"
    } else {
        "NULL"
    };
    let sql = format!(
        "INSERT INTO temp.codexx_catalog_provider_rollback \
            (host_id, thread_id, model_provider, missing_candidate) \
         SELECT {host_expr}, thread_id, model_provider, {missing_expr} \
         FROM local_thread_catalog WHERE COALESCE(model_provider, '') <> ?1 \
           AND thread_id = ?2"
    );
    for thread_id in provider_thread_ids {
        conn.execute(&sql, (target_provider, thread_id))
            .map_err(database_error)?;
    }
    Ok(())
}

fn snapshot_hidden_catalog_sources(
    conn: &Connection,
    columns: &HashSet<String>,
    host_id: &str,
    sources: &HashMap<String, CatalogRepairThread>,
) -> Result<()> {
    if !columns.contains("missing_candidate") {
        return Ok(());
    }
    for thread_id in sources.keys() {
        conn.execute(
            "INSERT OR IGNORE INTO temp.codexx_catalog_provider_rollback \
                (host_id, thread_id, model_provider, missing_candidate) \
             SELECT host_id, thread_id, model_provider, missing_candidate \
             FROM local_thread_catalog \
             WHERE host_id = ?1 AND thread_id = ?2 \
               AND COALESCE(missing_candidate, 0) <> 0",
            (host_id, thread_id),
        )
        .map_err(database_error)?;
    }
    Ok(())
}

fn reactivate_catalog_sources(
    conn: &Connection,
    columns: &HashSet<String>,
    host_id: &str,
    sources: &HashMap<String, CatalogRepairThread>,
) -> Result<()> {
    if !columns.contains("missing_candidate") {
        return Ok(());
    }
    for thread_id in sources.keys() {
        conn.execute(
            "UPDATE local_thread_catalog SET missing_candidate = 0 \
             WHERE host_id = ?1 AND thread_id = ?2 \
               AND COALESCE(missing_candidate, 0) <> 0",
            (host_id, thread_id),
        )
        .map_err(database_error)?;
    }
    Ok(())
}

fn insert_missing_catalog_sources(
    conn: &Connection,
    columns: &HashSet<String>,
    host_id: &str,
    sources: &HashMap<String, CatalogRepairThread>,
) -> Result<(usize, i64, f64)> {
    let mut observation_sequence = local_catalog_max_observation_sequence(conn)?;
    let insert_columns = local_catalog_insert_columns(columns);
    let placeholders = std::iter::repeat_n("?", insert_columns.len())
        .collect::<Vec<_>>()
        .join(", ");
    let insert_sql = format!(
        "INSERT OR IGNORE INTO local_thread_catalog ({}) VALUES ({})",
        insert_columns.join(", "),
        placeholders
    );
    let mut sorted_sources = sources.values().collect::<Vec<_>>();
    sorted_sources.sort_by(|left, right| left.id.cmp(&right.id));
    let mut inserted_rows = 0;
    let mut max_source_updated_at = 0.0_f64;
    for thread in sorted_sources {
        observation_sequence += 1;
        let values =
            local_catalog_insert_values(&insert_columns, host_id, thread, observation_sequence);
        let affected = conn
            .execute(&insert_sql, params_from_iter(values))
            .map_err(database_error)?;
        if affected == 0 {
            continue;
        }
        conn.execute(
            "INSERT INTO temp.codexx_catalog_insert_rollback (host_id, thread_id) \
             VALUES (?1, ?2)",
            (host_id, &thread.id),
        )
        .map_err(database_error)?;
        inserted_rows += affected;
        max_source_updated_at = max_source_updated_at.max(thread.source_updated_at);
    }
    Ok((inserted_rows, observation_sequence, max_source_updated_at))
}

pub(super) fn apply_catalog_updates(
    conn: &Connection,
    columns: &HashSet<String>,
    target_provider: &str,
    sources: &HashMap<String, CatalogRepairThread>,
    provider_thread_ids: &HashSet<String>,
) -> Result<CatalogUpdateCounts> {
    let mut counts = CatalogUpdateCounts::default();
    let repair_host = if !sources.is_empty() && catalog_supports_repair(columns) {
        Some(local_catalog_host_id(conn)?)
    } else {
        None
    };
    snapshot_catalog_provider_changes(conn, columns, target_provider, provider_thread_ids)?;
    if let Some(host_id) = repair_host.as_deref() {
        snapshot_hidden_catalog_sources(conn, columns, host_id, sources)?;
    }
    if columns.contains("model_provider") && columns.contains("thread_id") {
        for thread_id in provider_thread_ids {
            conn.execute(
                "UPDATE local_thread_catalog SET model_provider = ?1 \
                 WHERE thread_id = ?2 AND COALESCE(model_provider, '') <> ?1",
                (target_provider, thread_id),
            )
            .map_err(database_error)?;
        }
    }
    if let Some(host_id) = repair_host.as_deref() {
        reactivate_catalog_sources(conn, columns, host_id, sources)?;
        let (inserted, observation_sequence, max_source_updated_at) =
            insert_missing_catalog_sources(conn, columns, host_id, sources)?;
        counts.inserted_rows = inserted;
        if inserted > 0 {
            update_catalog_sync_state(conn, host_id, observation_sequence, max_source_updated_at)?;
        }
    }
    counts.provider_rows = conn
        .query_row(
            "SELECT COUNT(*) FROM temp.codexx_catalog_provider_rollback",
            [],
            |row| row.get(0),
        )
        .map_err(database_error)?;
    if counts.provider_rows + counts.inserted_rows > 0 {
        update_catalog_metadata(conn, 1)?;
    }
    Ok(counts)
}

fn restore_catalog_sync_state(conn: &Connection) -> Result<()> {
    if !sqlite_has_table(conn, "local_thread_catalog_sync_state")? {
        return Ok(());
    }
    let columns = table_column_set(conn, "local_thread_catalog_sync_state")?;
    if !columns.contains("host_id") {
        return Ok(());
    }
    conn.execute(
        "DELETE FROM local_thread_catalog_sync_state WHERE host_id IN (\
            SELECT host_id FROM temp.codexx_catalog_sync_insert_rollback\
         )",
        [],
    )
    .map_err(database_error)?;
    let mut assignments = Vec::new();
    for column in [
        "watermark_updated_at",
        "initial_build_complete",
        "observation_sequence",
        "last_full_reconciled_at",
    ] {
        if columns.contains(column) {
            assignments.push(format!(
                "{column} = (SELECT rollback.{column} FROM \
                 temp.codexx_catalog_sync_rollback AS rollback \
                 WHERE rollback.host_id = local_thread_catalog_sync_state.host_id)"
            ));
        }
    }
    if !assignments.is_empty() {
        conn.execute(
            &format!(
                "UPDATE local_thread_catalog_sync_state SET {} WHERE host_id IN (\
                    SELECT host_id FROM temp.codexx_catalog_sync_rollback\
                 )",
                assignments.join(", ")
            ),
            [],
        )
        .map_err(database_error)?;
    }
    Ok(())
}

fn restore_catalog_metadata(conn: &Connection) -> Result<()> {
    if !sqlite_has_table(conn, "local_thread_catalog_metadata")? {
        return Ok(());
    }
    let columns = table_column_set(conn, "local_thread_catalog_metadata")?;
    if !columns.contains("id") || !columns.contains("catalog_revision") {
        return Ok(());
    }
    conn.execute(
        "DELETE FROM local_thread_catalog_metadata WHERE id IN (\
            SELECT id FROM temp.codexx_catalog_metadata_insert_rollback\
         )",
        [],
    )
    .map_err(database_error)?;
    conn.execute(
        "UPDATE local_thread_catalog_metadata SET catalog_revision = (\
            SELECT rollback.catalog_revision \
            FROM temp.codexx_catalog_metadata_rollback AS rollback \
            WHERE rollback.id = local_thread_catalog_metadata.id\
         ) WHERE id IN (SELECT id FROM temp.codexx_catalog_metadata_rollback)",
        [],
    )
    .map_err(database_error)?;
    Ok(())
}

pub(super) fn restore_catalog_updates(conn: &Connection, columns: &HashSet<String>) -> Result<()> {
    restore_catalog_sync_state(conn)?;
    restore_catalog_metadata(conn)?;
    if catalog_supports_repair(columns) {
        conn.execute(
            "DELETE FROM local_thread_catalog WHERE (host_id, thread_id) IN (\
                SELECT host_id, thread_id FROM temp.codexx_catalog_insert_rollback\
             )",
            [],
        )
        .map_err(database_error)?;
    }
    if columns.contains("thread_id")
        && (columns.contains("model_provider") || columns.contains("missing_candidate"))
    {
        let row_match = if columns.contains("host_id") {
            "rollback.host_id = local_thread_catalog.host_id AND \
             rollback.thread_id = local_thread_catalog.thread_id"
        } else {
            "rollback.thread_id = local_thread_catalog.thread_id"
        };
        let changed_rows = if columns.contains("host_id") {
            "(host_id, thread_id) IN (SELECT host_id, thread_id FROM \
             temp.codexx_catalog_provider_rollback)"
        } else {
            "thread_id IN (SELECT thread_id FROM temp.codexx_catalog_provider_rollback)"
        };
        let mut assignments = Vec::new();
        if columns.contains("model_provider") {
            assignments.push(format!(
                "model_provider = (SELECT rollback.model_provider FROM \
                 temp.codexx_catalog_provider_rollback AS rollback WHERE {row_match})"
            ));
        }
        if columns.contains("missing_candidate") {
            assignments.push(format!(
                "missing_candidate = (SELECT rollback.missing_candidate FROM \
                 temp.codexx_catalog_provider_rollback AS rollback WHERE {row_match})"
            ));
        }
        conn.execute(
            &format!(
                "UPDATE local_thread_catalog SET {} WHERE {changed_rows}",
                assignments.join(", ")
            ),
            [],
        )
        .map_err(database_error)?;
    }
    conn.execute_batch(
        "DROP TABLE temp.codexx_catalog_sync_insert_rollback;
         DROP TABLE temp.codexx_catalog_sync_rollback;
         DROP TABLE temp.codexx_catalog_metadata_insert_rollback;
         DROP TABLE temp.codexx_catalog_metadata_rollback;
         DROP TABLE temp.codexx_catalog_insert_rollback;
         DROP TABLE temp.codexx_catalog_provider_rollback;",
    )
    .map_err(database_error)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::sync::atomic::{AtomicU64, Ordering};

    fn temp_dir(label: &str) -> PathBuf {
        static COUNTER: AtomicU64 = AtomicU64::new(0);
        let path = std::env::temp_dir().join(format!(
            "codex-x-catalog-{label}-{}-{}",
            std::process::id(),
            COUNTER.fetch_add(1, Ordering::Relaxed)
        ));
        let _ = fs::remove_dir_all(&path);
        fs::create_dir_all(&path).expect("create catalog test directory");
        path
    }

    fn create_catalog_schema(conn: &Connection) {
        conn.execute_batch(
            "CREATE TABLE local_thread_catalog (
                host_id TEXT NOT NULL,
                thread_id TEXT NOT NULL,
                display_title TEXT NOT NULL,
                source_created_at REAL NOT NULL,
                source_updated_at REAL NOT NULL,
                cwd TEXT NOT NULL,
                source_kind TEXT NOT NULL,
                source_detail TEXT,
                model_provider TEXT NOT NULL,
                git_branch TEXT,
                observation_sequence INTEGER NOT NULL,
                missing_candidate INTEGER NOT NULL DEFAULT 0,
                thread_source TEXT,
                source_recency_at REAL NOT NULL DEFAULT 0,
                PRIMARY KEY (host_id, thread_id)
             );
             CREATE TABLE local_thread_catalog_hosts (
                host_id TEXT PRIMARY KEY,
                host_kind TEXT NOT NULL
             );
             INSERT INTO local_thread_catalog_hosts VALUES ('local', 'local');
             CREATE TABLE local_thread_catalog_metadata (
                id INTEGER PRIMARY KEY,
                catalog_revision INTEGER NOT NULL DEFAULT 0
             );
             INSERT INTO local_thread_catalog_metadata VALUES (1, 7);
             CREATE TABLE local_thread_catalog_sync_state (
                host_id TEXT PRIMARY KEY,
                watermark_updated_at REAL,
                initial_build_complete INTEGER NOT NULL DEFAULT 0,
                observation_sequence INTEGER NOT NULL DEFAULT 0,
                last_full_reconciled_at INTEGER
             );
             INSERT INTO local_thread_catalog_sync_state VALUES ('local', 100, 1, 1, 100);",
        )
        .expect("create catalog schema");
    }

    fn insert_catalog_row(conn: &Connection, id: &str, provider: &str, missing: i64) {
        conn.execute(
            "INSERT INTO local_thread_catalog (
                host_id, thread_id, display_title, source_created_at, source_updated_at,
                cwd, source_kind, source_detail, model_provider, git_branch,
                observation_sequence, missing_candidate, thread_source, source_recency_at
             ) VALUES ('local', ?1, ?1, 100, 100, '/tmp/project', 'cli', '', ?2,
                NULL, 1, ?3, 'user', 100)",
            (id, provider, missing),
        )
        .expect("insert catalog row");
    }

    fn repair_source(id: &str, provider: &str) -> CatalogRepairThread {
        CatalogRepairThread {
            id: id.to_string(),
            display_title: id.to_string(),
            source_created_at: 100.0,
            source_updated_at: 100.0,
            source_recency_at: 100.0,
            cwd: "/tmp/project".to_string(),
            source_kind: "cli".to_string(),
            source_detail: String::new(),
            model_provider: provider.to_string(),
            git_branch: None,
            thread_source: Some("user".to_string()),
        }
    }

    fn catalog_state(conn: &Connection, id: &str) -> (String, i64, i64) {
        conn.query_row(
            "SELECT model_provider, missing_candidate,
                (SELECT catalog_revision FROM local_thread_catalog_metadata WHERE id = 1)
             FROM local_thread_catalog WHERE host_id = 'local' AND thread_id = ?1",
            [id],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )
        .expect("read catalog state")
    }

    fn apply_and_commit(
        conn: &Connection,
        target_provider: &str,
        sources: &HashMap<String, CatalogRepairThread>,
        provider_thread_ids: &HashSet<String>,
    ) -> CatalogUpdateCounts {
        conn.execute_batch("BEGIN IMMEDIATE")
            .expect("begin catalog update");
        create_catalog_rollback_tables(conn).expect("create rollback tables");
        let columns = catalog_columns(conn).expect("read catalog columns");
        let counts = apply_catalog_updates(
            conn,
            &columns,
            target_provider,
            sources,
            provider_thread_ids,
        )
        .expect("apply catalog update");
        conn.execute_batch("COMMIT").expect("commit catalog update");
        counts
    }

    fn restore_and_commit(conn: &Connection) {
        conn.execute_batch("BEGIN IMMEDIATE")
            .expect("begin catalog restore");
        let columns = catalog_columns(conn).expect("read catalog columns");
        restore_catalog_updates(conn, &columns).expect("restore catalog update");
        conn.execute_batch("COMMIT")
            .expect("commit catalog restore");
    }

    #[test]
    fn hidden_source_is_drift_and_visibility_revision_roll_back_together() {
        let root = temp_dir("hidden-source");
        let thread_path = root.join("state_5.sqlite");
        let catalog_path = root.join("sqlite/codex-dev.db");
        fs::create_dir_all(catalog_path.parent().expect("catalog parent"))
            .expect("create catalog parent");
        let thread = Connection::open(&thread_path).expect("create thread database");
        thread
            .execute_batch(
                "CREATE TABLE threads (
                    id TEXT PRIMARY KEY,
                    model_provider TEXT NOT NULL,
                    title TEXT
                 );
                 INSERT INTO threads VALUES ('thread-hidden', 'custom', 'Hidden');",
            )
            .expect("create source thread");
        drop(thread);
        let catalog = Connection::open(&catalog_path).expect("create catalog database");
        create_catalog_schema(&catalog);
        insert_catalog_row(&catalog, "thread-hidden", "custom", 1);
        drop(catalog);

        let syncable_thread_ids = HashSet::from(["thread-hidden".to_string()]);
        let scan = scan_catalog_sync(
            &[thread_path],
            std::slice::from_ref(&catalog_path),
            "custom",
            &syncable_thread_ids,
        )
        .expect("scan hidden catalog row");
        assert_eq!(scan.mismatched_rows, 0);
        assert_eq!(scan.missing_rows, 1);
        assert!(scan.mismatched_thread_ids.contains("thread-hidden"));

        let catalog = Connection::open(&catalog_path).expect("open catalog for update");
        let counts = apply_and_commit(&catalog, "custom", &scan.sources, &syncable_thread_ids);
        assert_eq!(counts.provider_rows, 1);
        assert_eq!(counts.inserted_rows, 0);
        assert_eq!(
            catalog_state(&catalog, "thread-hidden"),
            ("custom".to_string(), 0, 8)
        );

        restore_and_commit(&catalog);
        assert_eq!(
            catalog_state(&catalog, "thread-hidden"),
            ("custom".to_string(), 1, 7)
        );
        drop(catalog);
        fs::remove_dir_all(root).expect("remove catalog test directory");
    }

    #[test]
    fn provider_only_change_bumps_revision_and_rolls_back() {
        let conn = Connection::open_in_memory().expect("open in-memory catalog");
        create_catalog_schema(&conn);
        insert_catalog_row(&conn, "thread-provider", "openai", 0);
        let provider_thread_ids = HashSet::from(["thread-provider".to_string()]);

        let counts = apply_and_commit(&conn, "custom", &HashMap::new(), &provider_thread_ids);
        assert_eq!(counts.provider_rows, 1);
        assert_eq!(counts.inserted_rows, 0);
        assert_eq!(
            catalog_state(&conn, "thread-provider"),
            ("custom".to_string(), 0, 8)
        );

        restore_and_commit(&conn);
        assert_eq!(
            catalog_state(&conn, "thread-provider"),
            ("openai".to_string(), 0, 7)
        );
    }

    #[test]
    fn local_catalog_host_is_preferred_over_lexicographically_first_remote() {
        let conn = Connection::open_in_memory().expect("open host catalog");
        conn.execute_batch(
            "CREATE TABLE local_thread_catalog_hosts (
                host_id TEXT PRIMARY KEY,
                host_kind TEXT NOT NULL
             );
             INSERT INTO local_thread_catalog_hosts VALUES ('a-remote', 'ssh');
             INSERT INTO local_thread_catalog_hosts VALUES ('z-local-kind', 'local');
             INSERT INTO local_thread_catalog_hosts VALUES ('local', 'remote-control');",
        )
        .expect("seed catalog hosts");
        assert_eq!(
            local_catalog_host_id(&conn).expect("select local host"),
            "local"
        );

        conn.execute(
            "DELETE FROM local_thread_catalog_hosts WHERE host_id = 'local'",
            [],
        )
        .expect("remove literal local host");
        assert_eq!(
            local_catalog_host_id(&conn).expect("select local-kind host"),
            "z-local-kind"
        );
    }

    #[test]
    fn provider_and_visibility_change_share_one_revision_and_restore_snapshot() {
        let conn = Connection::open_in_memory().expect("open combined catalog");
        create_catalog_schema(&conn);
        insert_catalog_row(&conn, "thread-combined", "openai", 1);
        let sources = HashMap::from([(
            "thread-combined".to_string(),
            repair_source("thread-combined", "custom"),
        )]);
        let provider_thread_ids = HashSet::from(["thread-combined".to_string()]);

        let counts = apply_and_commit(&conn, "custom", &sources, &provider_thread_ids);
        assert_eq!(counts.provider_rows, 1);
        assert_eq!(counts.inserted_rows, 0);
        assert_eq!(
            catalog_state(&conn, "thread-combined"),
            ("custom".to_string(), 0, 8)
        );

        restore_and_commit(&conn);
        assert_eq!(
            catalog_state(&conn, "thread-combined"),
            ("openai".to_string(), 1, 7)
        );
    }
}
