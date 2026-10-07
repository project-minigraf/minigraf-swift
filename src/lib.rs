#![cfg_attr(
    test,
    allow(
        clippy::unwrap_used,
        clippy::expect_used,
        clippy::panic,
        clippy::indexing_slicing,
        clippy::cast_possible_truncation,
        clippy::cast_possible_wrap,
        clippy::cast_sign_loss,
    )
)]

use minigraf::{QueryResult, Value};
use std::sync::{Arc, Mutex, MutexGuard};

uniffi::setup_scaffolding!();

// ─── Error type ──────────────────────────────────────────────────────────────

#[derive(Debug, thiserror::Error, uniffi::Error)]
pub enum MiniGrafError {
    #[error("storage error: {msg}")]
    Storage { msg: String },
    #[error("query error: {msg}")]
    Query { msg: String },
    #[error("parse error: {msg}")]
    Parse { msg: String },
    #[error("unknown error: {msg}")]
    Other { msg: String },
}

// ─── minigraf::MinigrafError → MiniGrafError conversion ──────────────────────

impl From<minigraf::MinigrafError> for MiniGrafError {
    fn from(e: minigraf::MinigrafError) -> Self {
        let msg = e.to_string();
        match e.category() {
            minigraf::ErrorCategory::Parser => MiniGrafError::Parse { msg },
            minigraf::ErrorCategory::Storage | minigraf::ErrorCategory::Wal => {
                MiniGrafError::Storage { msg }
            }
            minigraf::ErrorCategory::Query => MiniGrafError::Query { msg },
            minigraf::ErrorCategory::Api | minigraf::ErrorCategory::Internal | _ => {
                MiniGrafError::Other { msg }
            }
        }
    }
}

impl MiniGrafError {
    /// Append the position of the rejected record to the message.
    fn at_batch_index(self, index: usize) -> Self {
        let suffix = |msg: String| format!("{msg} (batch index {index})");
        match self {
            MiniGrafError::Storage { msg } => MiniGrafError::Storage { msg: suffix(msg) },
            MiniGrafError::Query { msg } => MiniGrafError::Query { msg: suffix(msg) },
            MiniGrafError::Parse { msg } => MiniGrafError::Parse { msg: suffix(msg) },
            MiniGrafError::Other { msg } => MiniGrafError::Other { msg: suffix(msg) },
        }
    }
}

fn lock<T>(m: &Mutex<T>) -> Result<MutexGuard<'_, T>, MiniGrafError> {
    m.lock().map_err(|_| MiniGrafError::Other {
        msg: "mutex poisoned".into(),
    })
}

// Counts cross the FFI as signed integers: Kotlin's unsigned types cannot be
// called from Java. A negative or oversized count is API-017.

fn to_u64(n: i64, what: &str) -> Result<u64, MiniGrafError> {
    u64::try_from(n).map_err(|_| {
        minigraf::MinigrafError::invalid_argument(format!("{what} {n} is negative")).into()
    })
}

fn to_usize(n: i64, what: &str) -> Result<usize, MiniGrafError> {
    usize::try_from(to_u64(n, what)?).map_err(|_| {
        minigraf::MinigrafError::invalid_argument(format!("{what} {n} is too large")).into()
    })
}

/// A transaction counter or timestamp for the FFI. Both stay far below
/// `i64::MAX` (a counter of one per transaction; Unix milliseconds).
fn from_u64(n: u64) -> i64 {
    i64::try_from(n).unwrap_or(i64::MAX)
}

fn parse_uuid(s: &str, what: &str) -> Result<minigraf::EntityId, minigraf::MinigrafError> {
    minigraf::EntityId::parse_str(s).map_err(|_| {
        minigraf::MinigrafError::invalid_argument(format!("{what} {s:?} is not a UUID"))
    })
}

// ─── Open options ────────────────────────────────────────────────────────────

/// WAL durability mode. See the Rust `SyncMode`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, uniffi::Enum)]
pub enum SyncMode {
    /// Flush the WAL after every write (the default).
    Full,
    /// Flush only at checkpoints.
    Normal,
}

/// Options for `MiniGrafDb.open_with_options` and `MiniGrafLogWriter.create`.
/// Every field is optional; an absent field keeps the Rust default.
#[derive(Debug, Clone, Default, uniffi::Record)]
pub struct OpenOptions {
    /// Open read-only: shared lock, nothing written, writes fail with API-014.
    #[uniffi(default)]
    pub read_only: Option<bool>,
    /// Pages (4 KB each) in the page cache. Default 256.
    #[uniffi(default)]
    pub page_cache_size: Option<i64>,
    /// Open even when the filesystem cannot lock files. Default false.
    #[uniffi(default)]
    pub allow_unlocked: Option<bool>,
    /// WAL entries before an automatic checkpoint. Default 1000; the maximum
    /// value also suppresses the checkpoint on close.
    #[uniffi(default)]
    pub wal_checkpoint_threshold: Option<i64>,
    /// Facts a recursive rule may derive per iteration. Default 1,000,000.
    #[uniffi(default)]
    pub max_derived_facts: Option<i64>,
    /// Rows a query may return. Default 1,000,000.
    #[uniffi(default)]
    pub max_results: Option<i64>,
    /// WAL durability. Default `Full`.
    #[uniffi(default)]
    pub synchronous: Option<SyncMode>,
}

impl OpenOptions {
    fn to_core(&self) -> Result<minigraf::OpenOptions, MiniGrafError> {
        let mut o = minigraf::OpenOptions::new();
        if let Some(v) = self.read_only {
            o = o.read_only(v);
        }
        if let Some(v) = self.page_cache_size {
            o = o.page_cache_size(to_usize(v, "page_cache_size")?);
        }
        if let Some(v) = self.allow_unlocked {
            o = o.allow_unlocked(v);
        }
        if let Some(v) = self.wal_checkpoint_threshold {
            o = o.wal_checkpoint_threshold(to_usize(v, "wal_checkpoint_threshold")?);
        }
        if let Some(v) = self.max_derived_facts {
            o = o.max_derived_facts(to_usize(v, "max_derived_facts")?);
        }
        if let Some(v) = self.max_results {
            o = o.max_results(to_usize(v, "max_results")?);
        }
        if let Some(v) = self.synchronous {
            o = o.synchronous(match v {
                SyncMode::Full => minigraf::SyncMode::Full,
                SyncMode::Normal => minigraf::SyncMode::Normal,
            });
        }
        Ok(o)
    }
}

// ─── Values and fact records ─────────────────────────────────────────────────

/// A fact value with its type kept, so a record can be written back exactly.
/// `Ref` holds an entity UUID string.
#[derive(Debug, Clone, PartialEq, uniffi::Enum)]
pub enum MiniGrafValue {
    Text { value: String },
    Int64 { value: i64 },
    Float64 { value: f64 },
    Bool { value: bool },
    Ref { value: String },
    Keyword { value: String },
    Null,
}

impl From<Value> for MiniGrafValue {
    fn from(v: Value) -> Self {
        match v {
            Value::String(value) => MiniGrafValue::Text { value },
            Value::Integer(value) => MiniGrafValue::Int64 { value },
            Value::Float(value) => MiniGrafValue::Float64 { value },
            Value::Boolean(value) => MiniGrafValue::Bool { value },
            Value::Ref(id) => MiniGrafValue::Ref {
                value: id.to_string(),
            },
            Value::Keyword(value) => MiniGrafValue::Keyword { value },
            Value::Null => MiniGrafValue::Null,
        }
    }
}

impl MiniGrafValue {
    fn to_core(&self) -> Result<Value, minigraf::MinigrafError> {
        Ok(match self {
            MiniGrafValue::Text { value } => Value::String(value.clone()),
            MiniGrafValue::Int64 { value } => Value::Integer(*value),
            MiniGrafValue::Float64 { value } => Value::Float(*value),
            MiniGrafValue::Bool { value } => Value::Boolean(*value),
            MiniGrafValue::Ref { value } => Value::Ref(parse_uuid(value, "ref value")?),
            MiniGrafValue::Keyword { value } => Value::Keyword(value.clone()),
            MiniGrafValue::Null => Value::Null,
        })
    }
}

/// One fact-log record: an assertion or a retraction, with its transaction
/// and valid-time bounds. `valid_to` = 9223372036854775807 (i64 max) is forever.
#[derive(Debug, Clone, PartialEq, uniffi::Record)]
pub struct FactRecord {
    /// The entity UUID string.
    pub entity: String,
    pub attribute: String,
    pub value: MiniGrafValue,
    pub tx_count: i64,
    pub tx_id: i64,
    pub valid_from: i64,
    pub valid_to: i64,
    pub asserted: bool,
}

impl From<minigraf::FactRecord> for FactRecord {
    fn from(r: minigraf::FactRecord) -> Self {
        FactRecord {
            entity: r.entity.to_string(),
            attribute: r.attribute,
            value: r.value.into(),
            tx_count: from_u64(r.tx_count),
            tx_id: from_u64(r.tx_id),
            valid_from: r.valid_from,
            valid_to: r.valid_to,
            asserted: r.asserted,
        }
    }
}

impl FactRecord {
    fn to_core(&self) -> Result<minigraf::FactRecord, minigraf::MinigrafError> {
        let count = |n: i64, what: &str| {
            u64::try_from(n).map_err(|_| {
                minigraf::MinigrafError::invalid_argument(format!("{what} {n} is negative"))
            })
        };
        Ok(minigraf::FactRecord {
            entity: parse_uuid(&self.entity, "entity")?,
            attribute: self.attribute.clone(),
            value: self.value.to_core()?,
            tx_count: count(self.tx_count, "tx_count")?,
            tx_id: count(self.tx_id, "tx_id")?,
            valid_from: self.valid_from,
            valid_to: self.valid_to,
            asserted: self.asserted,
        })
    }
}

/// The order of a fact log.
#[derive(Debug, Clone, Copy, PartialEq, Eq, uniffi::Enum)]
pub enum FactOrder {
    /// Ascending `tx_count`, each transaction's records together (the default).
    Tx,
    /// The cheapest order to read, in one scan.
    Storage,
}

/// Which records a fact log returns. Every field is optional; the ones that
/// are set must all match.
#[derive(Debug, Clone, Default, uniffi::Record)]
pub struct FactFilter {
    /// Keep these attributes (combined with `attribute_prefixes`, either matches).
    #[uniffi(default)]
    pub attributes: Option<Vec<String>>,
    /// Keep attributes starting with one of these, such as `":ingestion/"`.
    #[uniffi(default)]
    pub attribute_prefixes: Option<Vec<String>>,
    /// Keep these entity UUID strings.
    #[uniffi(default)]
    pub entities: Option<Vec<String>>,
    /// Lowest `tx_count` kept (inclusive).
    #[uniffi(default)]
    pub tx_from: Option<i64>,
    /// Highest `tx_count` kept (inclusive).
    #[uniffi(default)]
    pub tx_to: Option<i64>,
    #[uniffi(default)]
    pub order: Option<FactOrder>,
    /// In `Tx` order, the most records held in memory at once.
    #[uniffi(default)]
    pub window: Option<i64>,
}

impl FactFilter {
    fn to_core(&self) -> Result<minigraf::FactFilter, MiniGrafError> {
        let mut f = minigraf::FactFilter::new();
        if let Some(attrs) = &self.attributes {
            f = f.attributes(attrs.iter().cloned());
        }
        for prefix in self.attribute_prefixes.iter().flatten() {
            f = f.attribute_prefix(prefix);
        }
        if let Some(entities) = &self.entities {
            let ids = entities
                .iter()
                .map(|e| parse_uuid(e, "entity"))
                .collect::<Result<Vec<_>, _>>()?;
            f = f.entities(ids);
        }
        if self.tx_from.is_some() || self.tx_to.is_some() {
            let lo = self.tx_from.map_or(Ok(0), |n| to_u64(n, "tx_from"))?;
            let hi = self.tx_to.map_or(Ok(u64::MAX), |n| to_u64(n, "tx_to"))?;
            f = f.tx_range(lo..=hi);
        }
        if let Some(order) = self.order {
            f = f.order(match order {
                FactOrder::Tx => minigraf::FactOrder::Tx,
                FactOrder::Storage => minigraf::FactOrder::Storage,
            });
        }
        if let Some(w) = self.window {
            f = f.window(to_usize(w, "window")?);
        }
        Ok(f)
    }
}

// ─── MiniGrafDb ──────────────────────────────────────────────────────────────

#[derive(uniffi::Object)]
pub struct MiniGrafDb {
    inner: Arc<Mutex<minigraf::Minigraf>>,
}

#[uniffi::export]
impl MiniGrafDb {
    #[uniffi::constructor]
    pub fn open(path: String) -> Result<Arc<Self>, MiniGrafError> {
        let db = minigraf::Minigraf::open(&path).map_err(MiniGrafError::from)?;
        Ok(Arc::new(Self {
            inner: Arc::new(Mutex::new(db)),
        }))
    }

    #[uniffi::constructor]
    pub fn open_in_memory() -> Result<Arc<Self>, MiniGrafError> {
        let db = minigraf::Minigraf::in_memory().map_err(MiniGrafError::from)?;
        Ok(Arc::new(Self {
            inner: Arc::new(Mutex::new(db)),
        }))
    }

    pub fn execute(&self, datalog: String) -> Result<String, MiniGrafError> {
        let result = self
            .inner
            .lock()
            .map_err(|_| MiniGrafError::Other {
                msg: "mutex poisoned".into(),
            })?
            .execute(&datalog)
            .map_err(MiniGrafError::from)?;
        Ok(query_result_to_json(result))
    }

    /// Open a file-backed database with `options`.
    #[uniffi::constructor]
    pub fn open_with_options(
        path: String,
        options: OpenOptions,
    ) -> Result<Arc<Self>, MiniGrafError> {
        let db = minigraf::Minigraf::open_with_options(&path, options.to_core()?)
            .map_err(MiniGrafError::from)?;
        Ok(Arc::new(Self {
            inner: Arc::new(Mutex::new(db)),
        }))
    }

    /// Open a cursor over a `(query ...)`. Its answer is fixed when it opens.
    pub fn query(&self, datalog: String) -> Result<Arc<MiniGrafCursor>, MiniGrafError> {
        let cursor = lock(&self.inner)?.query(&datalog)?;
        Ok(Arc::new(MiniGrafCursor {
            vars: cursor.vars().to_vec(),
            inner: Mutex::new(Some(cursor)),
        }))
    }

    /// Stream every fact record that `filter` keeps. Checkpoints wait until
    /// the log is closed or read to the end.
    pub fn fact_log(&self, filter: FactFilter) -> Result<Arc<MiniGrafFactLog>, MiniGrafError> {
        let filter = filter.to_core()?;
        let log = lock(&self.inner)?.fact_log(&filter)?;
        Ok(Arc::new(MiniGrafFactLog {
            inner: Mutex::new(Some(log)),
        }))
    }

    /// The transaction counter that `:as-of N` compares against.
    pub fn current_tx_count(&self) -> Result<i64, MiniGrafError> {
        Ok(from_u64(lock(&self.inner)?.current_tx_count()))
    }

    pub fn checkpoint(&self) -> Result<(), MiniGrafError> {
        self.inner
            .lock()
            .map_err(|_| MiniGrafError::Other {
                msg: "mutex poisoned".into(),
            })?
            .checkpoint()
            .map_err(MiniGrafError::from)
    }
}

// ─── MiniGrafCursor ──────────────────────────────────────────────────────────

/// The rows of one query answer, in batches.
#[derive(uniffi::Object)]
pub struct MiniGrafCursor {
    vars: Vec<String>,
    /// `None` after `close()` or the end.
    inner: Mutex<Option<minigraf::Cursor>>,
}

#[uniffi::export]
impl MiniGrafCursor {
    /// The `:find` variables, in column order.
    pub fn vars(&self) -> Vec<String> {
        self.vars.clone()
    }

    /// The next batch of at most `max_rows` rows (0 counts as 1), as a JSON
    /// array of rows encoded like `execute()`'s `results`; `None` at the end
    /// or after `close()`. A batch is never empty.
    pub fn next_batch(&self, max_rows: i32) -> Result<Option<String>, MiniGrafError> {
        let mut guard = lock(&self.inner)?;
        let Some(cursor) = guard.as_mut() else {
            return Ok(None);
        };
        match cursor.next_batch(to_usize(i64::from(max_rows), "max_rows")?)? {
            Some(batch) => Ok(Some(rows_to_json(batch.rows()))),
            None => {
                *guard = None;
                Ok(None)
            }
        }
    }

    /// Release the cursor. Later `next_batch` calls return `None`.
    pub fn close(&self) {
        if let Ok(mut guard) = self.inner.lock() {
            *guard = None;
        }
    }
}

// ─── MiniGrafFactLog ─────────────────────────────────────────────────────────

/// A forward-only stream of fact records from `MiniGrafDb.fact_log`.
#[derive(uniffi::Object)]
pub struct MiniGrafFactLog {
    /// `None` after `close()` or the end, which releases the database.
    inner: Mutex<Option<minigraf::FactLog>>,
}

#[uniffi::export]
impl MiniGrafFactLog {
    /// The next batch of at most `max_records` records (0 counts as 1);
    /// `None` at the end or after `close()`. A batch is never empty.
    pub fn next_batch(&self, max_records: i32) -> Result<Option<Vec<FactRecord>>, MiniGrafError> {
        let mut guard = lock(&self.inner)?;
        let Some(log) = guard.as_mut() else {
            return Ok(None);
        };
        match log.next_batch(to_usize(i64::from(max_records), "max_records")?) {
            Ok(Some(batch)) => Ok(Some(batch.into_iter().map(FactRecord::from).collect())),
            Ok(None) => {
                *guard = None;
                Ok(None)
            }
            Err(e) => {
                *guard = None;
                Err(e.into())
            }
        }
    }

    /// Stop reading and release the database.
    pub fn close(&self) {
        if let Ok(mut guard) = self.inner.lock() {
            *guard = None;
        }
    }
}

// ─── MiniGrafLogWriter ───────────────────────────────────────────────────────

/// Builds a new database file from fact records, keeping their transaction
/// and valid-time bounds. The file is built at `<path>.partial` and renamed
/// into place by `finish()`.
#[derive(uniffi::Object)]
pub struct MiniGrafLogWriter {
    /// `None` after `finish()` or `close()`.
    inner: Mutex<Option<minigraf::LogWriter>>,
}

fn writer_closed() -> MiniGrafError {
    minigraf::MinigrafError::closed("log writer").into()
}

#[uniffi::export]
impl MiniGrafLogWriter {
    /// Start building a new database at `path` (STG-043 if it exists).
    #[uniffi::constructor]
    pub fn create(path: String, options: OpenOptions) -> Result<Arc<Self>, MiniGrafError> {
        let writer = minigraf::LogWriter::create(&path, options.to_core()?)?;
        Ok(Arc::new(Self {
            inner: Mutex::new(Some(writer)),
        }))
    }

    /// Append one record. A rejected record changes nothing.
    pub fn append(&self, record: FactRecord) -> Result<(), MiniGrafError> {
        let record = record.to_core()?;
        let mut guard = lock(&self.inner)?;
        let writer = guard.as_mut().ok_or_else(writer_closed)?;
        Ok(writer.append(&record)?)
    }

    /// Append `records` in order, stopping at the first rejected one. Its
    /// error ends with `(batch index N)`; the records before it stay appended.
    pub fn append_batch(&self, records: Vec<FactRecord>) -> Result<(), MiniGrafError> {
        let mut guard = lock(&self.inner)?;
        let writer = guard.as_mut().ok_or_else(writer_closed)?;
        for (i, record) in records.iter().enumerate() {
            record
                .to_core()
                .and_then(|r| writer.append(&r))
                .map_err(|e| MiniGrafError::from(e).at_batch_index(i))?;
        }
        Ok(())
    }

    /// Close the open transaction and raise the counter to `tx_count`.
    pub fn advance_tx_count(&self, tx_count: i64) -> Result<(), MiniGrafError> {
        let tx_count = to_u64(tx_count, "tx_count")?;
        let mut guard = lock(&self.inner)?;
        let writer = guard.as_mut().ok_or_else(writer_closed)?;
        Ok(writer.advance_tx_count(tx_count)?)
    }

    /// The highest `tx_count` appended or advanced to.
    pub fn tx_count(&self) -> Result<i64, MiniGrafError> {
        let guard = lock(&self.inner)?;
        Ok(from_u64(
            guard.as_ref().ok_or_else(writer_closed)?.tx_count(),
        ))
    }

    /// Commit every record and rename the file into place. Later calls are
    /// API-018.
    pub fn finish(&self) -> Result<(), MiniGrafError> {
        let writer = lock(&self.inner)?.take().ok_or_else(writer_closed)?;
        Ok(writer.finish()?)
    }

    /// Abandon an unfinished build, deleting `<path>.partial`. A no-op after
    /// `finish()` or `close()`.
    pub fn close(&self) {
        if let Ok(mut guard) = self.inner.lock() {
            guard.take();
        }
    }

    /// `false` once `finish()` or `close()` has been called.
    pub fn is_open(&self) -> bool {
        self.inner.lock().is_ok_and(|g| g.is_some())
    }
}

// ─── JSON serialisation (internal helpers) ───────────────────────────────────

fn rows_to_json(rows: &[Vec<Value>]) -> String {
    let rows: Vec<Vec<serde_json::Value>> = rows
        .iter()
        .map(|row| row.iter().map(value_to_json).collect())
        .collect();
    serde_json::Value::from(rows).to_string()
}

fn value_to_json(v: &Value) -> serde_json::Value {
    use serde_json::Value as JVal;
    match v {
        Value::String(s) => JVal::String(s.clone()),
        Value::Integer(i) => JVal::Number((*i).into()),
        Value::Float(f) => serde_json::Number::from_f64(*f)
            .map(JVal::Number)
            .unwrap_or(JVal::Null),
        Value::Boolean(b) => JVal::Bool(*b),
        Value::Ref(uuid) => JVal::String(uuid.to_string()),
        Value::Keyword(k) => JVal::String(k.clone()),
        Value::Null => JVal::Null,
    }
}

fn query_result_to_json(result: QueryResult) -> String {
    use serde_json::json;
    let val = match result {
        QueryResult::Transacted(tx_id) => {
            json!({"transacted": tx_id})
        }
        QueryResult::Retracted(tx_id) => {
            json!({"retracted": tx_id})
        }
        QueryResult::Ok => json!({"ok": true}),
        QueryResult::QueryResults { vars, results } => {
            let rows: Vec<Vec<serde_json::Value>> = results
                .iter()
                .map(|row| row.iter().map(value_to_json).collect())
                .collect();
            json!({"variables": vars, "results": rows})
        }
    };
    val.to_string()
}

// ─── Tests ───────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn value_to_json_string() {
        let v = Value::String("hello".into());
        let j = value_to_json(&v);
        assert_eq!(j, serde_json::Value::String("hello".into()));
    }

    #[test]
    fn value_to_json_integer() {
        let v = Value::Integer(42);
        let j = value_to_json(&v);
        assert_eq!(j, serde_json::json!(42));
    }

    #[test]
    fn value_to_json_null() {
        let j = value_to_json(&Value::Null);
        assert_eq!(j, serde_json::Value::Null);
    }

    #[test]
    fn query_result_to_json_transacted() {
        let json = query_result_to_json(QueryResult::Transacted(12345));
        let v: serde_json::Value = serde_json::from_str(&json).expect("valid json");
        assert_eq!(v["transacted"], serde_json::json!(12345));
    }

    #[test]
    fn query_result_to_json_query_results() {
        let result = QueryResult::QueryResults {
            vars: vec!["?name".into()],
            results: vec![vec![Value::String("Alice".into())]],
        };
        let json = query_result_to_json(result);
        let v: serde_json::Value = serde_json::from_str(&json).expect("valid json");
        assert_eq!(v["variables"][0], "?name");
        assert_eq!(v["results"][0][0], "Alice");
    }

    #[test]
    fn query_result_to_json_ok() {
        let json = query_result_to_json(QueryResult::Ok);
        let v: serde_json::Value = serde_json::from_str(&json).expect("valid json");
        assert_eq!(v["ok"], serde_json::json!(true));
    }

    #[test]
    fn value_to_json_float_finite() {
        let j = value_to_json(&Value::Float(2.5));
        assert_eq!(j, serde_json::json!(2.5));
    }

    #[test]
    fn value_to_json_float_nan() {
        let j = value_to_json(&Value::Float(f64::NAN));
        assert_eq!(j, serde_json::Value::Null);
    }

    #[test]
    fn value_to_json_float_infinity() {
        let j = value_to_json(&Value::Float(f64::INFINITY));
        assert_eq!(j, serde_json::Value::Null);
    }

    #[test]
    fn value_to_json_boolean() {
        assert_eq!(
            value_to_json(&Value::Boolean(true)),
            serde_json::json!(true)
        );
        assert_eq!(
            value_to_json(&Value::Boolean(false)),
            serde_json::json!(false)
        );
    }

    #[test]
    fn value_to_json_ref() {
        let id = minigraf::EntityId::new_v4();
        let j = value_to_json(&Value::Ref(id));
        assert_eq!(j, serde_json::Value::String(id.to_string()));
    }

    #[test]
    fn value_to_json_keyword() {
        let j = value_to_json(&Value::Keyword(":status/active".into()));
        assert_eq!(j, serde_json::Value::String(":status/active".into()));
    }

    #[test]
    fn query_result_to_json_retracted() {
        let json = query_result_to_json(QueryResult::Retracted(99));
        let v: serde_json::Value = serde_json::from_str(&json).expect("valid json");
        assert_eq!(v["retracted"], serde_json::json!(99));
    }

    #[test]
    fn open_in_memory_succeeds() {
        MiniGrafDb::open_in_memory().expect("open_in_memory");
    }

    #[test]
    fn execute_transact_returns_json() {
        let db = MiniGrafDb::open_in_memory().expect("open");
        let json = db
            .execute(r#"(transact [[:alice :name "Alice"]])"#.into())
            .expect("execute");
        let v: serde_json::Value = serde_json::from_str(&json).expect("valid json");
        assert!(v.get("transacted").is_some(), "expected transacted key");
    }

    #[test]
    fn execute_query_returns_results() {
        let db = MiniGrafDb::open_in_memory().expect("open");
        db.execute(r#"(transact [[:alice :name "Alice"]])"#.into())
            .expect("transact");
        let json = db
            .execute(r#"(query [:find ?n :where [?e :name ?n]])"#.into())
            .expect("query");
        let v: serde_json::Value = serde_json::from_str(&json).expect("valid json");
        assert_eq!(v["variables"][0], "?n");
        assert_eq!(v["results"][0][0], "Alice");
    }

    #[test]
    fn execute_invalid_datalog_returns_parse_error() {
        let db = MiniGrafDb::open_in_memory().expect("open");
        // Illegal characters trigger tokenizer-level parse error
        let result = db.execute("not valid datalog at all !!!".into());
        assert!(
            matches!(result, Err(MiniGrafError::Parse { .. })),
            "expected Parse error for illegal characters"
        );
    }

    #[test]
    fn execute_unknown_command_returns_parse_error() {
        let db = MiniGrafDb::open_in_memory().expect("open");
        // Structurally valid tokens but unknown command — should also route to Parse
        let result = db.execute("(unknown-command [])".into());
        assert!(
            matches!(result, Err(MiniGrafError::Parse { .. })),
            "expected Parse error for unknown command"
        );
    }

    #[test]
    fn open_file_backed_roundtrip() {
        let dir = std::env::temp_dir();
        let path = dir.join("minigraf_ffi_test.graph");
        // Clean up any leftover file from a previous run
        let _ = std::fs::remove_file(&path);
        let path_str = path.to_str().expect("utf8 path").to_string();

        {
            let db = MiniGrafDb::open(path_str.clone()).expect("open");
            db.execute(r#"(transact [[:alice :name "Alice"]])"#.into())
                .expect("transact");
            db.checkpoint().expect("checkpoint");
        }

        // Re-open and verify fact persisted
        let db2 = MiniGrafDb::open(path_str).expect("re-open");
        let json = db2
            .execute(r#"(query [:find ?n :where [?e :name ?n]])"#.into())
            .expect("query");
        let v: serde_json::Value = serde_json::from_str(&json).expect("valid json");
        assert_eq!(v["results"][0][0], "Alice");

        // Clean up
        let _ = std::fs::remove_file(path);
        let wal = dir.join("minigraf_ffi_test.graph.wal");
        let _ = std::fs::remove_file(wal);
    }

    fn err<T, E>(r: Result<T, E>) -> E {
        match r {
            Ok(_) => panic!("expected an error"),
            Err(e) => e,
        }
    }

    fn msg(e: MiniGrafError) -> String {
        match e {
            MiniGrafError::Storage { msg }
            | MiniGrafError::Query { msg }
            | MiniGrafError::Parse { msg }
            | MiniGrafError::Other { msg } => msg,
        }
    }

    #[test]
    fn value_round_trips_through_core() {
        let id = minigraf::EntityId::new_v4();
        for v in [
            Value::String("s".into()),
            Value::Integer(-3),
            Value::Float(2.5),
            Value::Boolean(true),
            Value::Ref(id),
            Value::Keyword(":k/w".into()),
            Value::Null,
        ] {
            let back = MiniGrafValue::from(v.clone()).to_core().expect("to_core");
            assert!(back == v, "value changed in round trip");
        }
    }

    #[test]
    fn bad_uuid_is_api_017() {
        let e = err(MiniGrafValue::Ref {
            value: "nope".into(),
        }
        .to_core());
        assert_eq!(e.code(), "API-017");
        let filter = FactFilter {
            entities: Some(vec!["nope".into()]),
            ..FactFilter::default()
        };
        assert!(msg(err(filter.to_core())).starts_with("[API-017]"));
    }

    #[test]
    fn negative_counts_are_api_017() {
        let options = OpenOptions {
            page_cache_size: Some(-1),
            ..OpenOptions::default()
        };
        assert!(msg(err(options.to_core())).starts_with("[API-017]"));
        let db = MiniGrafDb::open_in_memory().expect("open");
        let cursor = db
            .query("(query [:find ?e :where [?e :n _]])".into())
            .expect("query");
        assert!(msg(err(cursor.next_batch(-1))).starts_with("[API-017]"));
    }

    #[test]
    fn cursor_batches_and_closes() {
        let db = MiniGrafDb::open_in_memory().expect("open");
        for i in 0..5 {
            db.execute(format!("(transact [[:e{i} :n {i}]])"))
                .expect("transact");
        }
        let cursor = db
            .query("(query [:find ?n :where [?e :n ?n]])".into())
            .expect("query");
        assert_eq!(cursor.vars(), vec!["?n".to_string()]);
        let mut rows = 0;
        while let Some(batch) = cursor.next_batch(2).expect("batch") {
            let batch: Vec<Vec<i64>> = serde_json::from_str(&batch).expect("json");
            assert!(!batch.is_empty() && batch.len() <= 2);
            rows += batch.len();
        }
        assert_eq!(rows, 5);
        let cursor = db
            .query("(query [:find ?n :where [?e :n ?n]])".into())
            .expect("query");
        cursor.close();
        assert!(cursor.next_batch(1).expect("closed").is_none());
        let e = err(db.query("(transact [[:a :n 1]])".into()));
        assert!(msg(e).starts_with("[API-012]"));
    }

    #[test]
    fn fact_log_to_log_writer_round_trip() {
        let dir = std::env::temp_dir().join(format!("minigraf_ffi_etl_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("dir");
        let src = MiniGrafDb::open_in_memory().expect("open");
        src.execute(r#"(transact [[:a :name "A"] [:a :n 1]])"#.into())
            .expect("transact");
        src.execute(r#"(retract [[:a :n 1]])"#.into())
            .expect("retract");
        let log = src.fact_log(FactFilter::default()).expect("log");
        let records = log.next_batch(100).expect("batch").expect("records");
        assert!(log.next_batch(100).expect("end").is_none());
        assert_eq!(records.len(), 3);

        let out = dir.join("out.graph").to_str().expect("utf8").to_string();
        let w = MiniGrafLogWriter::create(out.clone(), OpenOptions::default()).expect("create");
        let e = err(w.append_batch(vec![records[2].clone(), records[0].clone()]));
        let e = msg(e);
        assert!(e.starts_with("[API-015]") && e.ends_with("(batch index 1)"));
        w.close();
        assert!(!w.is_open());
        assert!(msg(err(w.finish())).starts_with("[API-018]"));
        assert!(!std::path::Path::new(&out).exists());

        let w = MiniGrafLogWriter::create(out.clone(), OpenOptions::default()).expect("create");
        w.append_batch(records.clone()).expect("append");
        assert_eq!(w.tx_count().expect("tx_count"), 2);
        w.finish().expect("finish");
        assert!(msg(err(w.tx_count())).starts_with("[API-018]"));

        let copy = MiniGrafDb::open_with_options(
            out.clone(),
            OpenOptions {
                read_only: Some(true),
                ..OpenOptions::default()
            },
        )
        .expect("open copy");
        assert_eq!(copy.current_tx_count().expect("count"), 2);
        let again = copy
            .fact_log(FactFilter::default())
            .expect("log")
            .next_batch(100)
            .expect("batch")
            .expect("records");
        assert!(again == records, "records changed in round trip");
        let e = err(copy.execute(r#"(transact [[:b :n 2]])"#.into()));
        assert!(msg(e).starts_with("[API-014]"));
        drop(copy);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn checkpoint_in_memory_succeeds() {
        let db = MiniGrafDb::open_in_memory().expect("open");
        db.checkpoint().expect("checkpoint on in-memory db");
    }
}
