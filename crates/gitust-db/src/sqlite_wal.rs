use std::ffi::{c_char, c_int, c_void, CStr, CString};
use std::io::ErrorKind;
use std::path::{Path, PathBuf};
use std::ptr;

const SQLITE_OK: c_int = 0;
const SQLITE_ROW: c_int = 100;
const SQLITE_DONE: c_int = 101;

const SQLITE_OPEN_READWRITE: c_int = 0x0000_0002;
const SQLITE_OPEN_CREATE: c_int = 0x0000_0004;
const SQLITE_OPEN_NOMUTEX: c_int = 0x0000_8000;

const SQLITE_INTEGER: c_int = 1;
const SQLITE_TEXT: c_int = 3;
const SQLITE_NULL: c_int = 5;

#[link(name = "sqlite3")]
unsafe extern "C" {
    fn sqlite3_open_v2(
        filename: *const c_char,
        pp_db: *mut *mut c_void,
        flags: c_int,
        z_vfs: *const c_char,
    ) -> c_int;
    fn sqlite3_close_v2(db: *mut c_void) -> c_int;
    fn sqlite3_exec(
        db: *mut c_void,
        sql: *const c_char,
        callback: *const c_void,
        arg: *mut c_void,
        errmsg: *mut *mut c_char,
    ) -> c_int;
    fn sqlite3_free(ptr: *mut c_void);
    fn sqlite3_prepare_v2(
        db: *mut c_void,
        z_sql: *const c_char,
        n_byte: c_int,
        pp_stmt: *mut *mut c_void,
        pz_tail: *mut *const c_char,
    ) -> c_int;
    fn sqlite3_bind_text(
        stmt: *mut c_void,
        idx: c_int,
        val: *const c_char,
        n_byte: c_int,
        destructor: *const c_void,
    ) -> c_int;
    fn sqlite3_bind_int64(stmt: *mut c_void, idx: c_int, val: i64) -> c_int;
    fn sqlite3_bind_null(stmt: *mut c_void, idx: c_int) -> c_int;
    fn sqlite3_step(stmt: *mut c_void) -> c_int;
    fn sqlite3_column_count(stmt: *mut c_void) -> c_int;
    fn sqlite3_column_type(stmt: *mut c_void, i_col: c_int) -> c_int;
    fn sqlite3_column_text(stmt: *mut c_void, i_col: c_int) -> *const u8;
    fn sqlite3_column_bytes(stmt: *mut c_void, i_col: c_int) -> c_int;
    fn sqlite3_column_int64(stmt: *mut c_void, i_col: c_int) -> i64;
    fn sqlite3_finalize(stmt: *mut c_void) -> c_int;
    fn sqlite3_errmsg(db: *mut c_void) -> *const c_char;
    fn sqlite3_busy_timeout(db: *mut c_void, ms: c_int) -> c_int;
}

fn sqlite_transient() -> *const c_void {
    (-1isize) as *const c_void
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SqlValue {
    Null,
    Integer(i64),
    Text(String),
}

impl SqlValue {
    pub fn as_i64(&self) -> Option<i64> {
        match self {
            Self::Integer(v) => Some(*v),
            _ => None,
        }
    }

    pub fn as_str(&self) -> Option<&str> {
        match self {
            Self::Text(s) => Some(s.as_str()),
            _ => None,
        }
    }
}

pub struct SqliteWalDb {
    db: *mut c_void,
    path: PathBuf,
}

impl Drop for SqliteWalDb {
    fn drop(&mut self) {
        if !self.db.is_null() {
            // SAFETY: `self.db` is a valid sqlite3 connection handle owned exclusively by `Self`.
            unsafe {
                sqlite3_close_v2(self.db);
            }
            self.db = ptr::null_mut();
        }
    }
}

fn sidecar_path(base: &Path, suffix: &str) -> PathBuf {
    let mut os = base.as_os_str().to_os_string();
    os.push(suffix);
    PathBuf::from(os)
}

fn reject_symlink(path: &Path) -> Result<bool, String> {
    match std::fs::symlink_metadata(path) {
        Ok(meta) if meta.file_type().is_symlink() => Err(format!(
            "refusing to open symlinked SQLite path: {:?}",
            path
        )),
        Ok(meta) if !meta.file_type().is_file() => {
            Err(format!("SQLite path is not a regular file: {:?}", path))
        }
        Ok(_) => Ok(true),
        Err(e) if e.kind() == ErrorKind::NotFound => Ok(false),
        Err(e) => Err(format!("failed to inspect SQLite path {:?}: {e}", path)),
    }
}

impl SqliteWalDb {
    pub fn open(path: &Path) -> Result<Self, String> {
        reject_symlink(path)?;
        reject_symlink(&sidecar_path(path, "-wal"))?;
        reject_symlink(&sidecar_path(path, "-shm"))?;

        if let Some(parent) = path.parent().filter(|p| !p.as_os_str().is_empty()) {
            std::fs::create_dir_all(parent)
                .map_err(|e| format!("failed to create SQLite parent directory: {e}"))?;
        }

        let path_str = path
            .to_str()
            .ok_or_else(|| format!("SQLite path is not valid UTF-8: {:?}", path))?;
        let c_path = CString::new(path_str)
            .map_err(|_| format!("SQLite path contains interior NUL byte: {:?}", path))?;

        let mut raw_db: *mut c_void = ptr::null_mut();
        let flags = SQLITE_OPEN_READWRITE | SQLITE_OPEN_CREATE | SQLITE_OPEN_NOMUTEX;
        // SAFETY: `c_path` is a valid NUL-terminated string and `raw_db` receives the handle.
        let rc = unsafe { sqlite3_open_v2(c_path.as_ptr(), &mut raw_db, flags, ptr::null()) };
        if rc != SQLITE_OK || raw_db.is_null() {
            let err = if raw_db.is_null() {
                format!("sqlite3_open_v2 failed with code {rc}")
            } else {
                let msg = unsafe { db_error_message(raw_db) };
                unsafe {
                    sqlite3_close_v2(raw_db);
                }
                msg
            };
            return Err(err);
        }

        let conn = Self {
            db: raw_db,
            path: path.to_path_buf(),
        };
        conn.enforce_permissions()?;
        // SAFETY: `conn.db` is valid and open.
        let timeout_rc = unsafe { sqlite3_busy_timeout(conn.db, 5000) };
        if timeout_rc != SQLITE_OK {
            return Err(conn.last_error());
        }
        conn.execute_batch(
            "PRAGMA journal_mode = WAL;
             PRAGMA synchronous = NORMAL;
             PRAGMA foreign_keys = ON;",
        )?;
        let mode = conn.journal_mode()?;
        if !mode.eq_ignore_ascii_case("wal") {
            return Err(format!("expected SQLite WAL journal_mode, got '{mode}'"));
        }
        conn.enforce_permissions()?;
        Ok(conn)
    }

    pub fn enforce_permissions(&self) -> Result<(), String> {
        for candidate in [
            self.path.clone(),
            sidecar_path(&self.path, "-wal"),
            sidecar_path(&self.path, "-shm"),
        ] {
            let exists = reject_symlink(&candidate)?;
            #[cfg(unix)]
            if exists {
                use std::os::unix::fs::PermissionsExt;
                let perms = std::fs::Permissions::from_mode(0o600);
                std::fs::set_permissions(&candidate, perms).map_err(|e| {
                    format!("failed to set 0600 permissions on {:?}: {e}", candidate)
                })?;
            }
        }
        Ok(())
    }

    pub fn journal_mode(&self) -> Result<String, String> {
        let rows = self.query_rows("PRAGMA journal_mode;", &[])?;
        let mode = rows
            .first()
            .and_then(|r| r.first())
            .and_then(SqlValue::as_str)
            .ok_or_else(|| "PRAGMA journal_mode returned no value".to_string())?;
        Ok(mode.to_ascii_lowercase())
    }

    pub fn execute_batch(&self, sql: &str) -> Result<(), String> {
        let c_sql =
            CString::new(sql).map_err(|_| "SQL batch contains interior NUL byte".to_string())?;
        let mut errmsg: *mut c_char = ptr::null_mut();
        // SAFETY: `self.db` is valid, `c_sql` is NUL-terminated, `errmsg` is freed via `sqlite3_free`.
        let rc = unsafe {
            sqlite3_exec(
                self.db,
                c_sql.as_ptr(),
                ptr::null(),
                ptr::null_mut(),
                &mut errmsg,
            )
        };
        if rc != SQLITE_OK {
            let msg = if !errmsg.is_null() {
                // SAFETY: `errmsg` was allocated by SQLite and is NUL-terminated.
                let text = unsafe { CStr::from_ptr(errmsg).to_string_lossy().into_owned() };
                unsafe { sqlite3_free(errmsg.cast::<c_void>()) };
                text
            } else {
                self.last_error()
            };
            return Err(format!("sqlite3_exec failed ({rc}): {msg}"));
        }
        Ok(())
    }

    pub fn execute(&self, sql: &str, params: &[SqlValue]) -> Result<(), String> {
        let mut stmt = PreparedStmt::prepare(self, sql)?;
        stmt.bind_all(params)?;
        let rc = stmt.step()?;
        if rc != SQLITE_DONE && rc != SQLITE_ROW {
            return Err(format!(
                "sqlite3_step unexpected code {rc}: {}",
                self.last_error()
            ));
        }
        Ok(())
    }

    pub fn query_rows(&self, sql: &str, params: &[SqlValue]) -> Result<Vec<Vec<SqlValue>>, String> {
        let mut stmt = PreparedStmt::prepare(self, sql)?;
        stmt.bind_all(params)?;
        let mut rows = Vec::new();
        loop {
            let rc = stmt.step()?;
            if rc == SQLITE_DONE {
                break;
            }
            if rc != SQLITE_ROW {
                return Err(format!(
                    "sqlite3_step unexpected code {rc}: {}",
                    self.last_error()
                ));
            }
            rows.push(stmt.read_current_row()?);
        }
        Ok(rows)
    }

    pub fn with_transaction<T, F>(&self, f: F) -> Result<T, String>
    where
        F: FnOnce(&Self) -> Result<T, String>,
    {
        self.execute_batch("BEGIN IMMEDIATE TRANSACTION;")?;
        match f(self) {
            Ok(val) => {
                if let Err(commit_err) = self.execute_batch("COMMIT;") {
                    if let Err(rb_err) = self.execute_batch("ROLLBACK;") {
                        return Err(format!("{commit_err}; rollback also failed: {rb_err}"));
                    }
                    return Err(commit_err);
                }
                self.enforce_permissions()?;
                Ok(val)
            }
            Err(err) => {
                if let Err(rb_err) = self.execute_batch("ROLLBACK;") {
                    return Err(format!("{err}; rollback also failed: {rb_err}"));
                }
                Err(err)
            }
        }
    }

    fn last_error(&self) -> String {
        // SAFETY: `self.db` is a valid sqlite3 pointer.
        unsafe { db_error_message(self.db) }
    }
}

unsafe fn db_error_message(db: *mut c_void) -> String {
    let ptr = unsafe { sqlite3_errmsg(db) };
    if ptr.is_null() {
        "unknown sqlite3 error".to_string()
    } else {
        unsafe { CStr::from_ptr(ptr).to_string_lossy().into_owned() }
    }
}

struct PreparedStmt<'a> {
    conn: &'a SqliteWalDb,
    stmt: *mut c_void,
}

impl<'a> Drop for PreparedStmt<'a> {
    fn drop(&mut self) {
        if !self.stmt.is_null() {
            // SAFETY: `self.stmt` is a valid statement handle.
            unsafe {
                sqlite3_finalize(self.stmt);
            }
            self.stmt = ptr::null_mut();
        }
    }
}

impl<'a> PreparedStmt<'a> {
    fn prepare(conn: &'a SqliteWalDb, sql: &str) -> Result<Self, String> {
        let c_sql =
            CString::new(sql).map_err(|_| "SQL query contains interior NUL byte".to_string())?;
        let mut stmt: *mut c_void = ptr::null_mut();
        // SAFETY: `conn.db` and `c_sql` are valid; `stmt` receives the prepared handle.
        let rc =
            unsafe { sqlite3_prepare_v2(conn.db, c_sql.as_ptr(), -1, &mut stmt, ptr::null_mut()) };
        if rc != SQLITE_OK || stmt.is_null() {
            return Err(format!(
                "sqlite3_prepare_v2 failed ({rc}): {}",
                conn.last_error()
            ));
        }
        Ok(Self { conn, stmt })
    }

    fn bind_all(&mut self, params: &[SqlValue]) -> Result<(), String> {
        for (idx_zero, param) in params.iter().enumerate() {
            let idx = c_int::try_from(idx_zero + 1)
                .map_err(|_| "SQLite bind index overflow".to_string())?;
            let rc = match param {
                SqlValue::Null => unsafe { sqlite3_bind_null(self.stmt, idx) },
                SqlValue::Integer(val) => unsafe { sqlite3_bind_int64(self.stmt, idx, *val) },
                SqlValue::Text(text) => {
                    if text.contains('\0') {
                        return Err("SQLite text parameter contains interior NUL byte".to_string());
                    }
                    let len = c_int::try_from(text.len())
                        .map_err(|_| "SQLite text parameter exceeds c_int::MAX".to_string())?;
                    unsafe {
                        sqlite3_bind_text(
                            self.stmt,
                            idx,
                            text.as_ptr().cast::<c_char>(),
                            len,
                            sqlite_transient(),
                        )
                    }
                }
            };
            if rc != SQLITE_OK {
                return Err(format!(
                    "sqlite3_bind failed at index {idx} ({rc}): {}",
                    self.conn.last_error()
                ));
            }
        }
        Ok(())
    }

    fn step(&mut self) -> Result<c_int, String> {
        // SAFETY: `self.stmt` is valid.
        let rc = unsafe { sqlite3_step(self.stmt) };
        if rc == SQLITE_ROW || rc == SQLITE_DONE {
            Ok(rc)
        } else {
            Err(format!(
                "sqlite3_step failed ({rc}): {}",
                self.conn.last_error()
            ))
        }
    }

    fn read_current_row(&self) -> Result<Vec<SqlValue>, String> {
        // SAFETY: `self.stmt` is positioned on a valid row (`SQLITE_ROW`).
        let col_count = unsafe { sqlite3_column_count(self.stmt) };
        let cap =
            usize::try_from(col_count).map_err(|_| "negative SQLite column count".to_string())?;
        let mut row = Vec::with_capacity(cap);
        for col in 0..col_count {
            let col_type = unsafe { sqlite3_column_type(self.stmt, col) };
            let val = match col_type {
                SQLITE_NULL => SqlValue::Null,
                SQLITE_INTEGER => {
                    let v = unsafe { sqlite3_column_int64(self.stmt, col) };
                    SqlValue::Integer(v)
                }
                SQLITE_TEXT => {
                    let ptr = unsafe { sqlite3_column_text(self.stmt, col) };
                    let len = unsafe { sqlite3_column_bytes(self.stmt, col) };
                    if ptr.is_null() || len <= 0 {
                        SqlValue::Text(String::new())
                    } else {
                        let byte_len = usize::try_from(len)
                            .map_err(|_| "negative SQLite column length".to_string())?;
                        let bytes = unsafe { std::slice::from_raw_parts(ptr, byte_len) };
                        let s = std::str::from_utf8(bytes)
                            .map_err(|e| format!("invalid UTF-8 in SQLite TEXT column: {e}"))?;
                        SqlValue::Text(s.to_owned())
                    }
                }
                other => {
                    return Err(format!("unsupported SQLite column type code: {other}"));
                }
            };
            row.push(val);
        }
        Ok(row)
    }
}
