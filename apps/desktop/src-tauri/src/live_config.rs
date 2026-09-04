use crate::error::{CodexxError, Result};
use crate::file_io::{atomic_write_checked, ensure_directory, io_err};
use std::fs;
use std::io::Write;
use std::path::Path;

pub(crate) struct LiveConfigLock {
    file: fs::File,
}

impl Drop for LiveConfigLock {
    fn drop(&mut self) {
        let _ = self.file.unlock();
    }
}

pub(crate) fn acquire_live_config_lock(codex_dir: &Path) -> Result<LiveConfigLock> {
    let tmp_dir = codex_dir.join("tmp");
    ensure_directory(&tmp_dir)?;
    let path = tmp_dir.join("codex-x-live-config.lock");
    if path.is_dir() {
        return Err(CodexxError::Config(format!(
            "Codex live 配置锁被同名目录占用: {}",
            path.display()
        )));
    }
    let mut file = fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .read(true)
        .write(true)
        .open(&path)
        .map_err(|error| io_err(&path, error))?;
    file.try_lock().map_err(|_| {
        CodexxError::Config(format!(
            "另一个 Codex-X-Pro 正在修改 Codex live 配置，请稍后重试: {}",
            path.display()
        ))
    })?;
    file.set_len(0).map_err(|error| io_err(&path, error))?;
    writeln!(file, "pid={}", std::process::id()).map_err(|error| io_err(&path, error))?;
    file.sync_all().map_err(|error| io_err(&path, error))?;
    Ok(LiveConfigLock { file })
}

pub(crate) fn read_file_snapshot(path: &Path) -> Result<Option<Vec<u8>>> {
    match fs::read(path) {
        Ok(bytes) => Ok(Some(bytes)),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(io_err(path, error)),
    }
}

pub(crate) fn text_from_snapshot(path: &Path, snapshot: Option<&[u8]>) -> Result<String> {
    String::from_utf8(snapshot.unwrap_or_default().to_vec())
        .map_err(|_| CodexxError::Config(format!("{} 不是有效的 UTF-8 文本", path.display())))
}

pub(crate) fn ensure_file_snapshot_unchanged(path: &Path, expected: Option<&[u8]>) -> Result<()> {
    if read_file_snapshot(path)?.as_deref() == expected {
        return Ok(());
    }
    Err(CodexxError::Config(format!(
        "{} 已被其他程序修改，本次写入已取消，请刷新后重试",
        path.display()
    )))
}

pub(crate) fn atomic_write_if_unchanged(
    path: &Path,
    expected: Option<&[u8]>,
    replacement: &[u8],
) -> Result<()> {
    atomic_write_checked(path, replacement, || {
        ensure_file_snapshot_unchanged(path, expected)
    })
}

pub(crate) fn remove_file_if_unchanged(path: &Path, expected: Option<&[u8]>) -> Result<()> {
    ensure_file_snapshot_unchanged(path, expected)?;
    match fs::remove_file(path) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(io_err(path, error)),
    }
}

pub(crate) fn restore_file_snapshot_if_unchanged(
    path: &Path,
    expected_current: Option<&[u8]>,
    snapshot: Option<&[u8]>,
) -> Result<()> {
    match snapshot {
        Some(bytes) => atomic_write_if_unchanged(path, expected_current, bytes),
        None => remove_file_if_unchanged(path, expected_current),
    }
}

pub(crate) struct AppliedFileChange {
    path: std::path::PathBuf,
    before: Option<Vec<u8>>,
    after: Option<Vec<u8>>,
    changed: bool,
}

impl AppliedFileChange {
    pub(crate) fn rollback(&self) -> Result<()> {
        if !self.changed {
            return Ok(());
        }
        restore_file_snapshot_if_unchanged(
            &self.path,
            self.after.as_deref(),
            self.before.as_deref(),
        )
    }
}

pub(crate) fn apply_file_change(
    path: &Path,
    before: Option<Vec<u8>>,
    after: Option<Vec<u8>>,
) -> Result<AppliedFileChange> {
    let changed = before != after;
    if changed {
        match after.as_deref() {
            Some(bytes) => atomic_write_if_unchanged(path, before.as_deref(), bytes)?,
            None => remove_file_if_unchanged(path, before.as_deref())?,
        }
    }
    Ok(AppliedFileChange {
        path: path.to_path_buf(),
        before,
        after,
        changed,
    })
}

pub(crate) fn rollback_file_changes(changes: &[AppliedFileChange]) -> Result<()> {
    for change in changes.iter().filter(|change| change.changed) {
        ensure_file_snapshot_unchanged(&change.path, change.after.as_deref())?;
    }

    let mut failures = Vec::new();
    for change in changes.iter().rev() {
        if let Err(error) = change.rollback() {
            failures.push(error.to_string());
        }
    }
    if failures.is_empty() {
        Ok(())
    } else {
        Err(CodexxError::Config(failures.join("；")))
    }
}

pub(crate) fn fail_with_file_rollback<T>(
    error: CodexxError,
    changes: &[AppliedFileChange],
) -> Result<T> {
    match rollback_file_changes(changes) {
        Ok(()) => Err(error),
        Err(rollback_error) => Err(CodexxError::Config(format!(
            "{error}；文件回滚失败：{rollback_error}"
        ))),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU64, Ordering};

    fn temp_dir(name: &str) -> std::path::PathBuf {
        static COUNTER: AtomicU64 = AtomicU64::new(0);
        let path = std::env::temp_dir().join(format!(
            "codex-x-live-config-{name}-{}-{}",
            std::process::id(),
            COUNTER.fetch_add(1, Ordering::Relaxed),
        ));
        let _ = fs::remove_dir_all(&path);
        fs::create_dir_all(&path).expect("create test directory");
        path
    }

    #[test]
    fn lock_rejects_a_second_writer_and_releases_on_drop() {
        let dir = temp_dir("lock");
        let first = acquire_live_config_lock(&dir).expect("acquire first lock");
        let error = acquire_live_config_lock(&dir)
            .err()
            .expect("second lock must fail");
        assert!(error.to_string().contains("另一个 Codex-X-Pro"));
        drop(first);
        acquire_live_config_lock(&dir).expect("lock is released on drop");
        fs::remove_dir_all(dir).expect("remove test directory");
    }

    #[test]
    fn stale_checked_write_preserves_the_external_value() {
        let dir = temp_dir("stale");
        let path = dir.join("config.toml");
        fs::write(&path, b"old").expect("seed file");
        let old = read_file_snapshot(&path).expect("capture file");
        fs::write(&path, b"external").expect("simulate external writer");

        atomic_write_if_unchanged(&path, old.as_deref(), b"codex-x")
            .expect_err("stale write must fail");

        assert_eq!(fs::read(&path).expect("read file"), b"external");
        fs::remove_dir_all(dir).expect("remove test directory");
    }
}
