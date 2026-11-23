//! Direct bindings to the system SQLite: `libsqlite3` on macOS and Linux (where the system web
//! view needs it too) and `winsqlite3.dll` on Windows. Nothing is compiled in and no crate sits
//! between; these are the typed calls `database` makes. Each connection is opened for one task
//! and closed after it, so the app never holds a database open.

use std::ffi::{c_char, c_int, c_void, CStr, CString};
use std::path::Path;
use std::time::{Duration, Instant};

#[repr(C)]
struct Sqlite3 {
    _private: [u8; 0],
}

#[repr(C)]
struct Sqlite3Statement {
    _private: [u8; 0],
}

const OK: c_int = 0;
const INTERRUPT: c_int = 9;
const ROW: c_int = 100;
const DONE: c_int = 101;

const OPEN_READONLY: c_int = 0x1;
const OPEN_READWRITE: c_int = 0x2;
const OPEN_CREATE: c_int = 0x4;

const LIMIT_ATTACHED: c_int = 7;

const INTEGER: c_int = 1;
const FLOAT: c_int = 2;
const TEXT: c_int = 3;
const BLOB: c_int = 4;

/// `SQLITE_TRANSIENT`: SQLite copies what it is bound before the call returns.
const TRANSIENT: isize = -1;
/// How many VM steps a statement runs between deadline checks.
const STEPS_BETWEEN_LOOKS: c_int = 4096;
/// How long a statement waits for another program's write to finish.
const WAIT_FOR_A_WRITER_MS: c_int = 2000;

#[cfg_attr(target_os = "windows", link(name = "winsqlite3"))]
#[cfg_attr(not(target_os = "windows"), link(name = "sqlite3"))]
extern "system" {
    fn sqlite3_open_v2(
        name: *const c_char,
        out: *mut *mut Sqlite3,
        flags: c_int,
        vfs: *const c_char,
    ) -> c_int;
    fn sqlite3_close(db: *mut Sqlite3) -> c_int;
    fn sqlite3_errmsg(db: *mut Sqlite3) -> *const c_char;
    fn sqlite3_busy_timeout(db: *mut Sqlite3, ms: c_int) -> c_int;
    fn sqlite3_limit(db: *mut Sqlite3, limit: c_int, value: c_int) -> c_int;
    fn sqlite3_progress_handler(
        db: *mut Sqlite3,
        steps: c_int,
        handler: Option<unsafe extern "system" fn(*mut c_void) -> c_int>,
        argument: *mut c_void,
    );
    fn sqlite3_changes(db: *mut Sqlite3) -> c_int;
    fn sqlite3_last_insert_rowid(db: *mut Sqlite3) -> i64;
    fn sqlite3_prepare_v2(
        db: *mut Sqlite3,
        sql: *const c_char,
        bytes: c_int,
        out: *mut *mut Sqlite3Statement,
        tail: *mut *const c_char,
    ) -> c_int;
    fn sqlite3_finalize(statement: *mut Sqlite3Statement) -> c_int;
    fn sqlite3_step(statement: *mut Sqlite3Statement) -> c_int;
    fn sqlite3_stmt_readonly(statement: *mut Sqlite3Statement) -> c_int;
    fn sqlite3_bind_parameter_count(statement: *mut Sqlite3Statement) -> c_int;
    fn sqlite3_bind_null(statement: *mut Sqlite3Statement, index: c_int) -> c_int;
    fn sqlite3_bind_int64(statement: *mut Sqlite3Statement, index: c_int, value: i64) -> c_int;
    fn sqlite3_bind_double(statement: *mut Sqlite3Statement, index: c_int, value: f64) -> c_int;
    fn sqlite3_bind_text(
        statement: *mut Sqlite3Statement,
        index: c_int,
        text: *const c_char,
        bytes: c_int,
        destructor: isize,
    ) -> c_int;
    fn sqlite3_column_count(statement: *mut Sqlite3Statement) -> c_int;
    fn sqlite3_column_name(statement: *mut Sqlite3Statement, column: c_int) -> *const c_char;
    fn sqlite3_column_type(statement: *mut Sqlite3Statement, column: c_int) -> c_int;
    fn sqlite3_column_int64(statement: *mut Sqlite3Statement, column: c_int) -> i64;
    fn sqlite3_column_double(statement: *mut Sqlite3Statement, column: c_int) -> f64;
    fn sqlite3_column_text(statement: *mut Sqlite3Statement, column: c_int) -> *const u8;
    fn sqlite3_column_blob(statement: *mut Sqlite3Statement, column: c_int) -> *const u8;
    fn sqlite3_column_bytes(statement: *mut Sqlite3Statement, column: c_int) -> c_int;
}

/// Why a SQLite call failed, in SQLite's own words.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Failure {
    /// The statement passed the connection's deadline and was interrupted.
    TookTooLong,
    Said(String),
}

impl std::fmt::Display for Failure {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::TookTooLong => formatter.write_str("the query took too long and was stopped"),
            Self::Said(words) => formatter.write_str(words),
        }
    }
}

/// What a connection may do to its file.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Access {
    /// Reads an existing file. No statement can change it.
    Reads,
    /// Reads and writes an existing file.
    Writes,
    /// Reads and writes the file, creating it if missing.
    Creates,
}

/// A value SQLite stores or binds.
#[derive(Clone, Debug, PartialEq)]
pub enum Value {
    Null,
    Integer(i64),
    Real(f64),
    Text(String),
    Blob(Vec<u8>),
}

/// One database file, open for one task. It cannot reach any other file: attaching is
/// disabled, which blocks both `ATTACH` and `VACUUM INTO`.
pub struct Connection {
    handle: *mut Sqlite3,
    access: Access,
    /// When its statements are interrupted. Boxed because SQLite holds its address.
    deadline: Box<Instant>,
}

/// SQLite's progress handler while a statement runs; returning non-zero interrupts it.
unsafe extern "system" fn out_of_patience(deadline: *mut c_void) -> c_int {
    // SAFETY: this is the connection's own deadline, which lives as long as the connection.
    let deadline = unsafe { &*deadline.cast::<Instant>() };
    c_int::from(Instant::now() > *deadline)
}

/// The file's name as SQLite takes it: UTF-8, NUL-terminated.
fn name_of(path: &Path) -> Result<CString, Failure> {
    #[cfg(unix)]
    let bytes = std::os::unix::ffi::OsStrExt::as_bytes(path.as_os_str()).to_vec();
    #[cfg(not(unix))]
    let bytes = path
        .to_str()
        .ok_or_else(|| Failure::Said("the file's name is not text".to_owned()))?
        .as_bytes()
        .to_vec();
    CString::new(bytes).map_err(|_| Failure::Said("the file's name holds a NUL".to_owned()))
}

impl Connection {
    /// Opens the database at `path`. Every statement is interrupted once `patience` has elapsed
    /// from now. A read-only connection still opens the file for writing when it can, so SQLite
    /// can roll back a hot journal left by a crashed writer and read a WAL file beside it; a file
    /// that cannot be written is opened read-only.
    pub fn open(path: &Path, access: Access, patience: Duration) -> Result<Self, Failure> {
        let name = name_of(path)?;
        let flags = match access {
            Access::Reads | Access::Writes => OPEN_READWRITE,
            Access::Creates => OPEN_READWRITE | OPEN_CREATE,
        };
        let handle = Self::handle(&name, flags).or_else(|failure| match access {
            Access::Reads => Self::handle(&name, OPEN_READONLY),
            Access::Writes | Access::Creates => Err(failure),
        })?;
        let mut connection = Self {
            handle,
            access,
            deadline: Box::new(Instant::now() + patience),
        };
        let deadline: *mut Instant = &mut *connection.deadline;
        // SAFETY: a live connection; the deadline outlives it (dropped after `sqlite3_close`).
        unsafe {
            sqlite3_busy_timeout(handle, WAIT_FOR_A_WRITER_MS);
            sqlite3_limit(handle, LIMIT_ATTACHED, 0);
            sqlite3_progress_handler(
                handle,
                STEPS_BETWEEN_LOOKS,
                Some(out_of_patience),
                deadline.cast(),
            );
        }
        if access == Access::Reads {
            connection.run("PRAGMA query_only = 1")?;
        }
        Ok(connection)
    }

    fn handle(name: &CStr, flags: c_int) -> Result<*mut Sqlite3, Failure> {
        let mut handle = std::ptr::null_mut();
        // SAFETY: a NUL-terminated name and storage for the handle.
        let code = unsafe { sqlite3_open_v2(name.as_ptr(), &mut handle, flags, std::ptr::null()) };
        if code == OK {
            return Ok(handle);
        }
        // SAFETY: SQLite returns a handle to read the error from and close, or null.
        let failure = unsafe { said(handle) };
        // SAFETY: closing whatever `sqlite3_open_v2` returned, null included, is allowed.
        unsafe { sqlite3_close(handle) };
        Err(failure)
    }

    fn failure(&self, code: c_int) -> Failure {
        if code == INTERRUPT {
            return Failure::TookTooLong;
        }
        // SAFETY: a live connection.
        unsafe { said(self.handle) }
    }

    /// Prepares the single statement in `sql`. Several statements are refused, since the later
    /// ones would run unseen.
    pub fn prepare(&self, sql: &str) -> Result<Statement<'_>, Failure> {
        let bytes = c_int::try_from(sql.len())
            .map_err(|_| Failure::Said("the statement is too long".to_owned()))?;
        let mut handle = std::ptr::null_mut();
        let mut tail = std::ptr::null();
        // SAFETY: a live connection, `bytes` bytes of text, and storage for both outputs.
        let code = unsafe {
            sqlite3_prepare_v2(
                self.handle,
                sql.as_ptr().cast(),
                bytes,
                &mut handle,
                &mut tail,
            )
        };
        if code != OK {
            return Err(self.failure(code));
        }
        let statement = Statement {
            handle,
            connection: self,
        };
        // SAFETY: `tail` points into `sql`, at or after its start.
        let read = unsafe { tail.cast::<u8>().offset_from(sql.as_ptr()) };
        let rest = &sql.as_bytes()[usize::try_from(read).unwrap_or(sql.len())..];
        if !rest
            .iter()
            .all(|byte| byte.is_ascii_whitespace() || *byte == b';')
        {
            return Err(Failure::Said("one statement at a time".to_owned()));
        }
        if handle.is_null() {
            return Err(Failure::Said("there is no statement to run".to_owned()));
        }
        if self.access == Access::Reads && !statement.only_reads() {
            return Err(Failure::Said(
                "this page reads the database and may not change it".to_owned(),
            ));
        }
        Ok(statement)
    }

    /// Runs `sql`, a statement with no parameters whose rows are ignored.
    pub fn run(&self, sql: &str) -> Result<(), Failure> {
        let mut statement = self.prepare(sql)?;
        while statement.step()? {}
        Ok(())
    }

    /// Rows changed by the most recent statement that changed any.
    pub fn changes(&self) -> i64 {
        // SAFETY: a live connection.
        i64::from(unsafe { sqlite3_changes(self.handle) })
    }

    /// The rowid of the last inserted row.
    pub fn last_insert_rowid(&self) -> i64 {
        // SAFETY: a live connection.
        unsafe { sqlite3_last_insert_rowid(self.handle) }
    }
}

impl Drop for Connection {
    fn drop(&mut self) {
        // SAFETY: a live connection whose statements have all been finalized (each borrows it).
        unsafe { sqlite3_close(self.handle) };
    }
}

/// The last error SQLite reported on `handle`.
///
/// # Safety
/// `handle` is a connection SQLite gave, or null.
unsafe fn said(handle: *mut Sqlite3) -> Failure {
    if handle.is_null() {
        return Failure::Said("out of memory".to_owned());
    }
    // SAFETY: a live connection; its message is a NUL-terminated string it keeps.
    let words = unsafe { CStr::from_ptr(sqlite3_errmsg(handle)) };
    Failure::Said(words.to_string_lossy().into_owned())
}

/// One statement of a connection.
pub struct Statement<'connection> {
    handle: *mut Sqlite3Statement,
    connection: &'connection Connection,
}

impl Statement<'_> {
    /// Whether running it leaves the database unchanged.
    pub fn only_reads(&self) -> bool {
        // SAFETY: a live statement.
        unsafe { sqlite3_stmt_readonly(self.handle) != 0 }
    }

    /// Binds `values` to the statement's `?` parameters in order. The count must match.
    pub fn bind(&mut self, values: &[Value]) -> Result<(), Failure> {
        // SAFETY: a live statement.
        let asked = unsafe { sqlite3_bind_parameter_count(self.handle) };
        if usize::try_from(asked) != Ok(values.len()) {
            return Err(Failure::Said(format!(
                "the statement takes {asked} values and was given {}",
                values.len()
            )));
        }
        for (index, value) in (1..).zip(values) {
            // SAFETY: a live statement and an index it has; text is copied before the call
            // returns.
            let code = unsafe {
                match value {
                    Value::Null => sqlite3_bind_null(self.handle, index),
                    Value::Integer(number) => sqlite3_bind_int64(self.handle, index, *number),
                    Value::Real(number) => sqlite3_bind_double(self.handle, index, *number),
                    Value::Text(text) => {
                        let Ok(bytes) = c_int::try_from(text.len()) else {
                            return Err(Failure::Said("a value is too long".to_owned()));
                        };
                        sqlite3_bind_text(
                            self.handle,
                            index,
                            text.as_ptr().cast(),
                            bytes,
                            TRANSIENT,
                        )
                    }
                    Value::Blob(_) => {
                        return Err(Failure::Said("a value cannot be bytes".to_owned()));
                    }
                }
            };
            if code != OK {
                return Err(self.connection.failure(code));
            }
        }
        Ok(())
    }

    /// Steps to the next row. Returns true when there is a row to read and false when done.
    pub fn step(&mut self) -> Result<bool, Failure> {
        // SAFETY: a live statement.
        match unsafe { sqlite3_step(self.handle) } {
            ROW => Ok(true),
            DONE => Ok(false),
            code => Err(self.connection.failure(code)),
        }
    }

    pub fn column_count(&self) -> usize {
        // SAFETY: a live statement.
        usize::try_from(unsafe { sqlite3_column_count(self.handle) }).unwrap_or(0)
    }

    /// The result column names.
    pub fn column_names(&self) -> Vec<String> {
        (0..self.column_count())
            .map(|column| {
                // SAFETY: a live statement and a column it has; the name is a NUL-terminated
                // string SQLite keeps until the statement is finalized.
                let name = unsafe { sqlite3_column_name(self.handle, column as c_int) };
                if name.is_null() {
                    return String::new();
                }
                // SAFETY: as above.
                unsafe { CStr::from_ptr(name) }
                    .to_string_lossy()
                    .into_owned()
            })
            .collect()
    }

    /// Column `column` of the row the last `step` reached.
    pub fn value(&self, column: usize) -> Value {
        let column = column as c_int;
        // SAFETY: a live statement on a row, and a column it has. The bytes SQLite lends are
        // copied before the next call on the statement.
        unsafe {
            match sqlite3_column_type(self.handle, column) {
                INTEGER => Value::Integer(sqlite3_column_int64(self.handle, column)),
                FLOAT => Value::Real(sqlite3_column_double(self.handle, column)),
                TEXT => {
                    let text = sqlite3_column_text(self.handle, column);
                    Value::Text(String::from_utf8_lossy(self.lent(text, column)).into_owned())
                }
                BLOB => {
                    let bytes = sqlite3_column_blob(self.handle, column);
                    Value::Blob(self.lent(bytes, column).to_vec())
                }
                _ => Value::Null,
            }
        }
    }

    /// The bytes SQLite lends at `start` for `column`; empty for an empty value.
    ///
    /// # Safety
    /// `start` is what SQLite just gave for `column` of the current row.
    unsafe fn lent(&self, start: *const u8, column: c_int) -> &[u8] {
        // SAFETY: a live statement; the length is of the value `start` points at.
        let length = usize::try_from(unsafe { sqlite3_column_bytes(self.handle, column) });
        match length {
            // SAFETY: SQLite lends `length` bytes at a non-null `start`.
            Ok(length) if !start.is_null() => unsafe { std::slice::from_raw_parts(start, length) },
            _ => &[],
        }
    }

    /// The whole row the last `step` reached.
    pub fn row(&self) -> Vec<Value> {
        (0..self.column_count())
            .map(|column| self.value(column))
            .collect()
    }
}

impl Drop for Statement<'_> {
    fn drop(&mut self) {
        // SAFETY: this statement is owned here; finalizing null is a no-op.
        unsafe { sqlite3_finalize(self.handle) };
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const PATIENT: Duration = Duration::from_secs(30);

    fn made(folder: &Path, name: &str, statements: &[&str]) -> std::path::PathBuf {
        let file = folder.join(name);
        let connection = Connection::open(&file, Access::Creates, PATIENT).expect("open");
        for statement in statements {
            connection.run(statement).expect("run");
        }
        file
    }

    #[test]
    fn values_go_in_and_come_back_as_what_they_are() {
        let folder = tempfile::tempdir().expect("folder");
        let file = made(
            folder.path(),
            "a.sqlite",
            &["CREATE TABLE t (i, r, s, b, n)"],
        );
        let connection = Connection::open(&file, Access::Writes, PATIENT).expect("open");
        let mut put = connection
            .prepare("INSERT INTO t VALUES (?, ?, ?, x'00ff', ?)")
            .expect("prepare");
        put.bind(&[
            Value::Integer(-7),
            Value::Real(1.5),
            Value::Text("né\0e".to_owned()),
            Value::Null,
        ])
        .expect("bind");
        assert!(!put.step().expect("step"));
        drop(put);
        assert_eq!(
            (connection.changes(), connection.last_insert_rowid()),
            (1, 1)
        );
        let mut read = connection.prepare("SELECT * FROM t").expect("prepare");
        assert_eq!(read.column_names(), ["i", "r", "s", "b", "n"]);
        assert!(read.step().expect("step"));
        assert_eq!(
            read.row(),
            [
                Value::Integer(-7),
                Value::Real(1.5),
                Value::Text("né\0e".to_owned()),
                Value::Blob(vec![0, 255]),
                Value::Null,
            ]
        );
        assert!(!read.step().expect("step"));
    }

    #[test]
    fn a_statement_says_why_it_fails_and_takes_exactly_its_values_one_at_a_time() {
        let folder = tempfile::tempdir().expect("folder");
        let file = made(folder.path(), "a.sqlite", &["CREATE TABLE t (i)"]);
        let connection = Connection::open(&file, Access::Writes, PATIENT).expect("open");
        assert_eq!(
            connection.prepare("SELECT * FROM missing").err(),
            Some(Failure::Said("no such table: missing".to_owned()))
        );
        assert_eq!(
            connection.prepare("SELECT 1; DROP TABLE t").err(),
            Some(Failure::Said("one statement at a time".to_owned()))
        );
        assert!(connection.prepare("SELECT 1; \n").is_ok());
        assert!(connection.prepare("  ").is_err());
        let mut asks = connection.prepare("SELECT ?, ?").expect("prepare");
        assert_eq!(
            asks.bind(&[Value::Null]).err(),
            Some(Failure::Said(
                "the statement takes 2 values and was given 1".to_owned()
            ))
        );
    }

    #[test]
    fn a_reader_changes_nothing_and_a_missing_file_is_made_only_when_asked() {
        let folder = tempfile::tempdir().expect("folder");
        let file = made(folder.path(), "a.sqlite", &["CREATE TABLE t (i)"]);
        let reader = Connection::open(&file, Access::Reads, PATIENT).expect("open");
        // Turning `query_only` off still does not allow writes.
        reader.run("PRAGMA query_only = 0").expect("a setting");
        for change in [
            "INSERT INTO t VALUES (1)",
            "DROP TABLE t",
            "PRAGMA journal_mode = WAL",
            "BEGIN IMMEDIATE",
        ] {
            assert!(reader.prepare(change).is_err(), "{change}");
        }
        assert!(reader.prepare("SELECT count(*) FROM t").is_ok());
        let missing = folder.path().join("none.sqlite");
        assert!(Connection::open(&missing, Access::Reads, PATIENT).is_err());
        assert!(Connection::open(&missing, Access::Writes, PATIENT).is_err());
        assert!(!missing.exists());
        assert!(Connection::open(&missing, Access::Creates, PATIENT).is_ok());
    }

    #[test]
    fn a_connection_reaches_no_other_file() {
        let folder = tempfile::tempdir().expect("folder");
        let file = made(folder.path(), "a.sqlite", &["CREATE TABLE t (i)"]);
        let other = folder.path().join("other.sqlite");
        let copy = folder.path().join("copy.sqlite");
        let connection = Connection::open(&file, Access::Writes, PATIENT).expect("open");
        let attach = format!("ATTACH DATABASE '{}' AS other", other.display());
        assert!(connection.run(&attach).is_err());
        let vacuum = format!("VACUUM INTO '{}'", copy.display());
        assert!(connection.run(&vacuum).is_err());
        assert!(!other.exists() && !copy.exists());
    }

    #[test]
    fn a_statement_that_runs_past_the_patience_is_stopped() {
        let folder = tempfile::tempdir().expect("folder");
        let file = made(folder.path(), "a.sqlite", &[]);
        let connection =
            Connection::open(&file, Access::Reads, Duration::from_millis(50)).expect("open");
        let endless = "WITH RECURSIVE n(i) AS (SELECT 1 UNION ALL SELECT i + 1 FROM n) \
                       SELECT count(*) FROM n";
        let mut statement = connection.prepare(endless).expect("prepare");
        assert_eq!(statement.step().err(), Some(Failure::TookTooLong));
    }

    #[test]
    fn a_file_that_is_not_a_database_is_refused() {
        let folder = tempfile::tempdir().expect("folder");
        let file = folder.path().join("notes.sqlite");
        std::fs::write(
            &file,
            "these are words, a good many of them, and no database",
        )
        .unwrap();
        let connection = Connection::open(&file, Access::Reads, PATIENT);
        // SQLite reads the file's header at the first statement, not on opening.
        let refused = match connection {
            Ok(open) => open.run("SELECT 1 FROM sqlite_master").is_err(),
            Err(_) => true,
        };
        assert!(refused);
    }
}
