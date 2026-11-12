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
use std::collections::HashMap;
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
    /// A call the API refuses: a bad argument, a closed object, a bind
    /// mistake. The message starts with its `API-` code.
    #[error("API error: {msg}")]
    Api { msg: String },
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
            minigraf::ErrorCategory::Api => MiniGrafError::Api { msg },
            minigraf::ErrorCategory::Internal | _ => MiniGrafError::Other { msg },
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
            MiniGrafError::Api { msg } => MiniGrafError::Api { msg: suffix(msg) },
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

/// A limit: `i64::MAX` stands for core's `usize::MAX` (no limit), which is
/// out of reach of a signed count; any other negative count is API-017.
fn to_limit(n: i64, what: &str) -> Result<usize, MiniGrafError> {
    if n == i64::MAX {
        Ok(usize::MAX)
    } else {
        to_usize(n, what)
    }
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

/// A query keyword that `OpenOptions.allow_overrides` may let query text
/// use. See the Rust `Overridable`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, uniffi::Enum)]
pub enum Overridable {
    /// `:max-derived-facts` (in the defaults).
    MaxDerivedFacts,
    /// `:max-results` (in the defaults).
    MaxResults,
    /// `:query-memory-limit` (not in the defaults).
    QueryMemoryLimit,
    /// `:max-iterations` (in the defaults).
    MaxIterations,
}

impl Overridable {
    fn to_core(self) -> minigraf::Overridable {
        match self {
            Overridable::MaxDerivedFacts => minigraf::Overridable::MaxDerivedFacts,
            Overridable::MaxResults => minigraf::Overridable::MaxResults,
            Overridable::QueryMemoryLimit => minigraf::Overridable::QueryMemoryLimit,
            Overridable::MaxIterations => minigraf::Overridable::MaxIterations,
        }
    }

    fn from_core(o: minigraf::Overridable) -> Option<Self> {
        match o {
            minigraf::Overridable::MaxDerivedFacts => Some(Overridable::MaxDerivedFacts),
            minigraf::Overridable::MaxResults => Some(Overridable::MaxResults),
            minigraf::Overridable::QueryMemoryLimit => Some(Overridable::QueryMemoryLimit),
            minigraf::Overridable::MaxIterations => Some(Overridable::MaxIterations),
            _ => None,
        }
    }
}

/// The override list that applies when `OpenOptions.allow_overrides` is
/// absent: the Rust `Overridable::DEFAULTS`.
#[uniffi::export]
pub fn overridable_defaults() -> Vec<Overridable> {
    minigraf::Overridable::DEFAULTS
        .iter()
        .copied()
        .filter_map(Overridable::from_core)
        .collect()
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
    /// WAL entries before an automatic checkpoint. Default 1000. The maximum
    /// (i64::MAX, 9223372036854775807) means never: no automatic checkpoint
    /// and no checkpoint when the handle closes; call `checkpoint()` yourself.
    #[uniffi(default)]
    pub wal_checkpoint_threshold: Option<i64>,
    /// Facts a recursive rule may derive per iteration. Default 1,000,000.
    #[uniffi(default)]
    pub max_derived_facts: Option<i64>,
    /// Deprecated alias of `max_derived_facts`; the lower of the two applies.
    #[uniffi(default)]
    pub max_results: Option<i64>,
    /// WAL durability. Default `Full`.
    #[uniffi(default)]
    pub synchronous: Option<SyncMode>,
    /// The most bytes one query may hold; a query that would hold more fails
    /// with QRY-012. The maximum (i64::MAX, 9223372036854775807) means no
    /// limit, the default.
    #[uniffi(default)]
    pub query_memory_limit: Option<i64>,
    /// The query keywords query text may use to override these settings.
    /// Absent: `overridable_defaults()`. A list replaces the defaults; an
    /// empty one refuses every override keyword (API-025).
    #[uniffi(default)]
    pub allow_overrides: Option<Vec<Overridable>>,
    /// Facts one transaction stages before a file database writes them in
    /// chunks. Default 65,536; clamped to 1..=1,000,000.
    #[uniffi(default)]
    pub transaction_chunk_facts: Option<i64>,
    /// Rounds one stratum of a rule query may run; more fail with INT-020.
    /// Default 1,000,000. The maximum (i64::MAX) means no limit.
    #[uniffi(default)]
    pub max_iterations: Option<i64>,
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
            // i64::MAX stands for core's usize::MAX sentinel, which is out of
            // reach of a signed count.
            let n = if v == i64::MAX {
                usize::MAX
            } else {
                to_usize(v, "wal_checkpoint_threshold")?
            };
            o = o.wal_checkpoint_threshold(n);
        }
        if let Some(v) = self.max_derived_facts {
            o = o.max_derived_facts(to_usize(v, "max_derived_facts")?);
        }
        if let Some(v) = self.max_results {
            #[allow(deprecated)]
            {
                o = o.max_results(to_usize(v, "max_results")?);
            }
        }
        if let Some(v) = self.synchronous {
            o = o.synchronous(match v {
                SyncMode::Full => minigraf::SyncMode::Full,
                SyncMode::Normal => minigraf::SyncMode::Normal,
            });
        }
        if let Some(v) = self.query_memory_limit {
            // i64::MAX stands for core's usize::MAX (no limit), as for
            // wal_checkpoint_threshold.
            let n = if v == i64::MAX {
                usize::MAX
            } else {
                to_usize(v, "query_memory_limit")?
            };
            o = o.query_memory_limit(n);
        }
        if let Some(list) = &self.allow_overrides {
            o = o.allow_overrides(list.iter().map(|v| v.to_core()));
        }
        if let Some(v) = self.transaction_chunk_facts {
            o = o.transaction_chunk_facts(to_usize(v, "transaction_chunk_facts")?);
        }
        if let Some(v) = self.max_iterations {
            o = o.max_iterations(to_limit(v, "max_iterations")?);
        }
        Ok(o)
    }
}

// ─── Values and fact records ─────────────────────────────────────────────────

/// A fact value with its type kept, so a record can be written back exactly.
/// `Ref` holds an entity UUID string. `Set` holds the elements of a set
/// value in canonical order; an element that is itself a `Set` is API-021.
#[derive(Debug, Clone, PartialEq, uniffi::Enum)]
pub enum MiniGrafValue {
    Text { value: String },
    Int64 { value: i64 },
    Float64 { value: f64 },
    Bool { value: bool },
    Ref { value: String },
    Keyword { value: String },
    Null,
    Set { value: Vec<MiniGrafValue> },
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
            Value::Set(set) => MiniGrafValue::Set {
                value: set.iter().cloned().map(MiniGrafValue::from).collect(),
            },
            // A value kind newer than this binding: its debug form, until
            // the binding learns it.
            other => MiniGrafValue::Text {
                value: format!("{other:?}"),
            },
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
            MiniGrafValue::Set { value } => Value::Set(minigraf::ValueSet::new(
                value
                    .iter()
                    .map(MiniGrafValue::to_core)
                    .collect::<Result<Vec<_>, _>>()?,
            )?),
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

// ─── Bind values ─────────────────────────────────────────────────────────────

/// The value of one `$slot` of a prepared query.
///
/// `Value` fills an entity position with a `Ref`, and a value, expression or
/// rule-argument position. `Entity` fills an entity position, and the others
/// as a `Ref`. `TxCount` and `Timestamp` fill `:as-of`; `Timestamp` and
/// `AnyValidTime` fill `:valid-at`. A bind value of the wrong kind for its
/// slot is API-024.
#[derive(Debug, Clone, PartialEq, uniffi::Enum)]
pub enum MiniGrafBindValue {
    Value {
        value: MiniGrafValue,
    },
    /// An entity UUID string.
    Entity {
        uuid: String,
    },
    /// The counter `:as-of N` compares against; negative is API-017.
    TxCount {
        value: i64,
    },
    /// Unix milliseconds.
    Timestamp {
        value: i64,
    },
    AnyValidTime,
}

impl MiniGrafBindValue {
    fn to_core(&self) -> Result<minigraf::BindValue, MiniGrafError> {
        Ok(match self {
            MiniGrafBindValue::Value { value } => minigraf::BindValue::Val(value.to_core()?),
            MiniGrafBindValue::Entity { uuid } => {
                minigraf::BindValue::Entity(parse_uuid(uuid, "entity")?)
            }
            MiniGrafBindValue::TxCount { value } => {
                minigraf::BindValue::TxCount(to_u64(*value, "tx count")?)
            }
            MiniGrafBindValue::Timestamp { value } => minigraf::BindValue::Timestamp(*value),
            MiniGrafBindValue::AnyValidTime => minigraf::BindValue::AnyValidTime,
        })
    }
}

/// `bindings` as core bind values, named without their `$`.
fn core_bindings(
    bindings: &HashMap<String, MiniGrafBindValue>,
) -> Result<Vec<(&str, minigraf::BindValue)>, MiniGrafError> {
    bindings
        .iter()
        .map(|(name, value)| Ok((name.as_str(), value.to_core()?)))
        .collect()
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
        let result = self.db()?.execute(&datalog)?;
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
        let cursor = self.db()?.query(&datalog)?;
        Ok(Arc::new(MiniGrafCursor {
            vars: cursor.vars().to_vec(),
            inner: Mutex::new(Some(cursor)),
        }))
    }

    /// Stream every fact record that `filter` keeps. Checkpoints wait until
    /// the log is closed or read to the end.
    pub fn fact_log(&self, filter: FactFilter) -> Result<Arc<MiniGrafFactLog>, MiniGrafError> {
        let filter = filter.to_core()?;
        let log = self.db()?.fact_log(&filter)?;
        Ok(Arc::new(MiniGrafFactLog {
            inner: Mutex::new(Some(log)),
        }))
    }

    /// Parse and validate a `(query ...)` once, for running many times with
    /// different bind values in its `$name` slots. A transact, retract or
    /// rule is API-005/006/007; a slot in an attribute position is API-023.
    pub fn prepare(&self, datalog: String) -> Result<Arc<MiniGrafPreparedQuery>, MiniGrafError> {
        let inner = self.db()?.prepare(&datalog)?;
        Ok(Arc::new(MiniGrafPreparedQuery {
            _db: self.inner.clone(),
            inner,
        }))
    }

    /// The transaction counter that `:as-of N` compares against.
    pub fn current_tx_count(&self) -> Result<i64, MiniGrafError> {
        Ok(from_u64(self.db()?.current_tx_count()))
    }

    pub fn checkpoint(&self) -> Result<(), MiniGrafError> {
        Ok(self.db()?.checkpoint()?)
    }

    /// Register `predicate` under `name`, called in Datalog as
    /// `[(name ?v)]`. A predicate that throws stops the query with QRY-014.
    /// Queries already open keep the functions they opened with.
    pub fn register_predicate(
        &self,
        name: String,
        predicate: Arc<dyn MiniGrafPredicate>,
    ) -> Result<(), MiniGrafError> {
        Ok(self.db()?.try_register_predicate(&name, move |v| {
            Ok(predicate.test(MiniGrafValue::from(v.clone()))?)
        })?)
    }

    /// Register `aggregate` under `name`, used in `:find` as `(name ?v)` and
    /// as a window function with `:over`. Each group or window partition
    /// gets its own accumulator. A throw stops the query with QRY-014.
    pub fn register_aggregate(
        &self,
        name: String,
        aggregate: Arc<dyn MiniGrafAggregate>,
    ) -> Result<(), MiniGrafError> {
        Ok(self.db()?.try_register_aggregate(
            &name,
            move || Ok(aggregate.accumulator()?),
            |acc: &mut Arc<dyn MiniGrafAccumulator>, v| {
                Ok(acc.step(MiniGrafValue::from(v.clone()))?)
            },
            |acc, count| {
                // A count of values held in memory stays far below i64::MAX.
                let count = i64::try_from(count).unwrap_or(i64::MAX);
                acc.finish(count)?
                    .to_core()
                    .map_err(|e| minigraf::UdfError::new(e.to_string()))
            },
        )?)
    }
}

impl MiniGrafDb {
    /// The database, cloned out of the handle so that no binding lock is
    /// held while the core runs: a user function may call back into this
    /// handle.
    fn db(&self) -> Result<minigraf::Minigraf, MiniGrafError> {
        Ok(lock(&self.inner)?.clone())
    }
}

// ─── User functions ──────────────────────────────────────────────────────────

/// Why a user function failed. A callback that throws anything else is
/// mapped to this. The query stops with QRY-014, which carries `msg`.
#[derive(Debug, thiserror::Error, uniffi::Error)]
pub enum MiniGrafUdfError {
    #[error("{msg}")]
    Failed { msg: String },
}

/// Any other exception. Kotlin sends its stack trace, so only the first
/// line is kept: the exception's class and message.
impl From<uniffi::UnexpectedUniFFICallbackError> for MiniGrafUdfError {
    fn from(e: uniffi::UnexpectedUniFFICallbackError) -> Self {
        let msg = e.reason.lines().next().unwrap_or_default().to_string();
        MiniGrafUdfError::Failed { msg }
    }
}

impl From<MiniGrafUdfError> for minigraf::UdfError {
    fn from(e: MiniGrafUdfError) -> Self {
        let MiniGrafUdfError::Failed { msg } = e;
        minigraf::UdfError::new(msg)
    }
}

/// A user predicate. It may be called from any thread that queries the
/// database, concurrently, so it must be thread-safe.
#[uniffi::export(with_foreign)]
pub trait MiniGrafPredicate: Send + Sync {
    fn test(&self, value: MiniGrafValue) -> Result<bool, MiniGrafUdfError>;
}

/// A user aggregate: a factory of accumulators.
#[uniffi::export(with_foreign)]
pub trait MiniGrafAggregate: Send + Sync {
    /// A new, empty accumulator: one per group, or per window partition.
    fn accumulator(&self) -> Result<Arc<dyn MiniGrafAccumulator>, MiniGrafUdfError>;
}

/// The running state of one aggregate. One query uses it, on one thread at
/// a time.
#[uniffi::export(with_foreign)]
pub trait MiniGrafAccumulator: Send + Sync {
    fn step(&self, value: MiniGrafValue) -> Result<(), MiniGrafUdfError>;
    /// `count`: values stepped so far.
    fn finish(&self, count: i64) -> Result<MiniGrafValue, MiniGrafUdfError>;
}

// ─── MiniGrafPreparedQuery ───────────────────────────────────────────────────

/// A query parsed once by `MiniGrafDb.prepare`, run with bind values. It
/// keeps its database open until both are freed, and each run reads the
/// database as it is then.
#[derive(uniffi::Object)]
pub struct MiniGrafPreparedQuery {
    /// Keeps the database (and its close-time checkpoint) alive.
    _db: Arc<Mutex<minigraf::Minigraf>>,
    inner: minigraf::PreparedQuery,
}

#[uniffi::export]
impl MiniGrafPreparedQuery {
    /// Run the query with `bindings` (slot name without its `$` to bind
    /// value) and return its result as JSON, encoded like
    /// `MiniGrafDb.execute`. Names the query does not use are ignored. A
    /// missing slot is API-022.
    pub fn execute(
        &self,
        bindings: HashMap<String, MiniGrafBindValue>,
    ) -> Result<String, MiniGrafError> {
        let result = self.inner.execute(&core_bindings(&bindings)?)?;
        Ok(query_result_to_json(result))
    }

    /// Run the query with `bindings` and return a cursor over its rows. The
    /// answer is fixed when the cursor opens.
    pub fn query(
        &self,
        bindings: HashMap<String, MiniGrafBindValue>,
    ) -> Result<Arc<MiniGrafCursor>, MiniGrafError> {
        let cursor = self.inner.query(&core_bindings(&bindings)?)?;
        Ok(Arc::new(MiniGrafCursor {
            vars: cursor.vars().to_vec(),
            inner: Mutex::new(Some(cursor)),
        }))
    }

    /// A new prepared query that may derive at most `n` facts; this one is
    /// unchanged. An allowed `:max-derived-facts` or `:max-results` in the
    /// query still overrides it. `i64::MAX` means no limit; negative is
    /// API-017. Not gated by `OpenOptions.allow_overrides`.
    pub fn with_max_derived_facts(
        &self,
        n: i64,
    ) -> Result<Arc<MiniGrafPreparedQuery>, MiniGrafError> {
        let n = to_limit(n, "max_derived_facts")?;
        Ok(self.derive(|q| q.with_max_derived_facts(n)))
    }

    /// A new prepared query that may hold at most `bytes` bytes (QRY-012
    /// beyond); this one is unchanged. `0` lets only queries that hold no
    /// state run. Takes its argument as `with_max_derived_facts` does.
    pub fn with_query_memory_limit(
        &self,
        bytes: i64,
    ) -> Result<Arc<MiniGrafPreparedQuery>, MiniGrafError> {
        let bytes = to_limit(bytes, "query_memory_limit")?;
        Ok(self.derive(|q| q.with_query_memory_limit(bytes)))
    }

    /// A new prepared query whose rule strata may run at most `n` rounds
    /// (INT-020 beyond); this one is unchanged. An allowed `:max-iterations`
    /// in the query still overrides it. Takes its argument as
    /// `with_max_derived_facts` does.
    pub fn with_max_iterations(&self, n: i64) -> Result<Arc<MiniGrafPreparedQuery>, MiniGrafError> {
        let n = to_limit(n, "max_iterations")?;
        Ok(self.derive(|q| q.with_max_iterations(n)))
    }
}

impl MiniGrafPreparedQuery {
    /// A copy with `set` applied, keeping the same database open.
    fn derive(
        &self,
        set: impl FnOnce(minigraf::PreparedQuery) -> minigraf::PreparedQuery,
    ) -> Arc<MiniGrafPreparedQuery> {
        Arc::new(MiniGrafPreparedQuery {
            _db: self._db.clone(),
            inner: set(self.inner.clone()),
        })
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

// ─── Write transactions ──────────────────────────────────────────────────────

/// Options for `MiniGrafDb.begin_write_with`. Every field is optional; an
/// absent field is the database's value. See the Rust `TxOptions`.
#[derive(Debug, Clone, Default, uniffi::Record)]
pub struct TxOptions {
    /// This transaction's WAL durability. Absent: the database's
    /// `OpenOptions.synchronous`. No effect in memory or on a transaction
    /// that becomes large, which commits as a checkpoint.
    #[uniffi(default)]
    pub synchronous: Option<SyncMode>,
    /// Facts this transaction stages before a file database writes them in
    /// chunks. Absent: the database's `transaction_chunk_facts`. Clamped to
    /// 1..=1,000,000; negative is API-017.
    #[uniffi(default)]
    pub chunk_facts: Option<i64>,
}

impl TxOptions {
    fn to_core(&self) -> Result<minigraf::TxOptions, MiniGrafError> {
        let mut o = minigraf::TxOptions::new();
        if let Some(v) = self.synchronous {
            o = o.synchronous(match v {
                SyncMode::Full => minigraf::SyncMode::Full,
                SyncMode::Normal => minigraf::SyncMode::Normal,
            });
        }
        if let Some(v) = self.chunk_facts {
            o = o.chunk_facts(to_usize(v, "chunk_facts")?);
        }
        Ok(o)
    }
}

#[uniffi::export]
impl MiniGrafDb {
    /// Begin a write transaction with the database's options. See
    /// `MiniGrafWriteTransaction`.
    pub fn begin_write(&self) -> Result<Arc<MiniGrafWriteTransaction>, MiniGrafError> {
        self.begin_write_with(TxOptions::default())
    }

    /// Begin a write transaction with `options`. It holds the database's
    /// write lock until it commits, rolls back or is freed: a write or a
    /// begin from another thread waits until then (this one included, while
    /// another transaction is open), and a query from another thread reads
    /// the committed state. On this thread, `execute`, `query`, `fact_log`,
    /// `prepare`, `checkpoint`, a begin and a prepared query's runs fail
    /// with INT-001 until it finishes. API-014 on a read-only database.
    pub fn begin_write_with(
        &self,
        options: TxOptions,
    ) -> Result<Arc<MiniGrafWriteTransaction>, MiniGrafError> {
        let options = options.to_core()?;
        // No binding lock is held while the begin waits for the write lock.
        let tx = self.db()?.begin_write_owned_with(options)?;
        Ok(Arc::new(MiniGrafWriteTransaction {
            inner: Mutex::new(Some(tx)),
            running: Mutex::new(None),
        }))
    }
}

/// A write transaction from `MiniGrafDb.begin_write`.
///
/// `execute` runs one statement inside it: a `transact`/`retract` is staged
/// and returns `{"ok":true}`, a `query` reads the committed facts plus the
/// staged ones, and a `rule` is registered at once (a rollback keeps it). A
/// statement that fails stages nothing and leaves the transaction open.
/// `commit` applies everything as one transaction; `rollback` discards it.
///
/// Once it committed (or failed to) or rolled back, it is finished: `execute` and `commit` fail with API-018, and `rollback` and
/// freeing it do nothing. Freed while open, it rolls back. Each call runs
/// whole: one used from two threads at once runs their calls one after the
/// other. It may be used and finished on any thread; INT-001 stays on the
/// thread that began it. It keeps its database open until it is freed.
///
/// A user function cannot call the transaction whose statement it runs in:
/// there `execute`, `commit` and `rollback` fail with INT-001 and change
/// nothing, and `is_open` is true.
#[derive(uniffi::Object)]
pub struct MiniGrafWriteTransaction {
    /// `None` once finished.
    inner: Mutex<Option<minigraf::OwnedWriteTransaction>>,
    /// The thread running a statement of this transaction, which holds
    /// `inner` meanwhile. A call from that thread comes from a user function
    /// of the statement and would wait for `inner` forever (minigraf#886).
    running: Mutex<Option<std::thread::ThreadId>>,
}

/// Clears `MiniGrafWriteTransaction::running` when the statement ends, also
/// by a panic.
struct Running<'a>(&'a Mutex<Option<std::thread::ThreadId>>);

impl Drop for Running<'_> {
    fn drop(&mut self) {
        *self.0.lock().unwrap_or_else(|e| e.into_inner()) = None;
    }
}

impl MiniGrafWriteTransaction {
    /// Whether this thread is running a statement of this transaction. Only
    /// the thread that set `running` can read its own id there, so the answer
    /// cannot change under the caller.
    fn in_own_statement(&self) -> bool {
        *self.running.lock().unwrap_or_else(|e| e.into_inner()) == Some(std::thread::current().id())
    }

    /// INT-001 for a call from a user function of this transaction's running
    /// statement. Checked before `inner` is locked: another thread's call
    /// passes and waits for the statement.
    fn refuse_own_statement(&self) -> Result<(), MiniGrafError> {
        if self.in_own_statement() {
            return Err(MiniGrafError::Other {
                msg: "[INT-001] a write transaction is already in progress on this thread; \
                      a user function cannot call the transaction it runs in"
                    .into(),
            });
        }
        Ok(())
    }
}

#[uniffi::export]
impl MiniGrafWriteTransaction {
    /// Run one Datalog command inside the transaction. Returns the JSON
    /// `MiniGrafDb.execute` returns for the same core result.
    pub fn execute(&self, datalog: String) -> Result<String, MiniGrafError> {
        self.refuse_own_statement()?;
        let mut guard = lock(&self.inner)?;
        let tx = guard.as_mut().ok_or_else(finished)?;
        // Declared after `guard`, so cleared before `inner` is released.
        *self.running.lock().unwrap_or_else(|e| e.into_inner()) = Some(std::thread::current().id());
        let _running = Running(&self.running);
        Ok(query_result_to_json(tx.execute(&datalog)?))
    }

    /// Commit every staged change as one transaction. A commit that fails
    /// rolls the transaction back; either way it is finished.
    pub fn commit(&self) -> Result<(), MiniGrafError> {
        self.refuse_own_statement()?;
        let tx = lock(&self.inner)?.take().ok_or_else(finished)?;
        Ok(tx.commit()?)
    }

    /// Discard every staged change. Does nothing on a finished transaction.
    pub fn rollback(&self) -> Result<(), MiniGrafError> {
        self.refuse_own_statement()?;
        if let Some(tx) = lock(&self.inner)?.take() {
            tx.rollback();
        }
        Ok(())
    }

    /// Whether the transaction is still open.
    pub fn is_open(&self) -> bool {
        self.in_own_statement() || self.inner.lock().is_ok_and(|g| g.is_some())
    }
}

/// API-018: a call on a finished transaction.
fn finished() -> MiniGrafError {
    minigraf::MinigrafError::closed("write transaction").into()
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
        Value::Set(set) => JVal::Array(set.iter().map(value_to_json).collect()),
        // A value kind newer than this binding: its debug form.
        other => JVal::String(format!("{other:?}")),
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
        let id = minigraf::EntityId::from_u128(0x6f1c_2a3b_4d5e_4f60_8172_839a_b4c5_d6e7);
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
            | MiniGrafError::Api { msg }
            | MiniGrafError::Other { msg } => msg,
        }
    }

    #[test]
    fn value_round_trips_through_core() {
        let id = minigraf::EntityId::from_u128(0x6f1c_2a3b_4d5e_4f60_8172_839a_b4c5_d6e7);
        for v in [
            Value::String("s".into()),
            Value::Integer(-3),
            Value::Float(2.5),
            Value::Boolean(true),
            Value::Ref(id),
            Value::Keyword(":k/w".into()),
            Value::Null,
            Value::Set(
                minigraf::ValueSet::new([
                    Value::Integer(1),
                    Value::Keyword(":k/w".into()),
                    Value::Ref(id),
                ])
                .expect("set"),
            ),
            Value::Set(minigraf::ValueSet::default()),
        ] {
            let back = MiniGrafValue::from(v.clone()).to_core().expect("to_core");
            assert!(back == v, "value changed in round trip");
        }
    }

    #[test]
    fn set_elements_dedup_and_nesting_is_api_021() {
        let one = || MiniGrafValue::Int64 { value: 1 };
        let back = MiniGrafValue::Set {
            value: vec![one(), MiniGrafValue::Text { value: "a".into() }, one()],
        }
        .to_core()
        .expect("to_core");
        assert_eq!(
            MiniGrafValue::from(back),
            MiniGrafValue::Set {
                value: vec![MiniGrafValue::Text { value: "a".into() }, one()]
            },
            "duplicates dropped, canonical order"
        );
        let e = err(MiniGrafValue::Set {
            value: vec![MiniGrafValue::Set { value: vec![] }],
        }
        .to_core());
        assert_eq!(e.code(), "API-021");
    }

    #[test]
    fn set_is_a_json_array() {
        let set =
            minigraf::ValueSet::new([Value::Integer(2), Value::String("a".into())]).expect("set");
        assert_eq!(value_to_json(&Value::Set(set)), serde_json::json!(["a", 2]));
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
    fn max_wal_checkpoint_threshold_suppresses_the_close_checkpoint() {
        let dir = std::env::temp_dir().join(format!("minigraf_ffi_never_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("dir");
        let path = dir.join("db.graph");
        let wal = dir.join("db.graph.wal");
        let options = |threshold| OpenOptions {
            wal_checkpoint_threshold: Some(threshold),
            ..OpenOptions::default()
        };

        let db =
            MiniGrafDb::open_with_options(path.to_str().expect("utf8").into(), options(i64::MAX))
                .expect("open");
        db.execute("(transact [[:a :n 1]])".into())
            .expect("transact");
        drop(db);
        assert!(
            wal.exists(),
            "i64::MAX must suppress the close-time checkpoint"
        );

        // One below the maximum is an ordinary threshold: closing checkpoints.
        let path2 = dir.join("db2.graph");
        let wal2 = dir.join("db2.graph.wal");
        let db = MiniGrafDb::open_with_options(
            path2.to_str().expect("utf8").into(),
            options(i64::MAX - 1),
        )
        .expect("open second");
        db.execute("(transact [[:a :n 1]])".into())
            .expect("transact");
        assert!(wal2.exists(), "the write is in the WAL before close");
        drop(db);
        assert!(!wal2.exists(), "an ordinary threshold checkpoints on close");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn query_memory_limit_is_qry_012() {
        let options = |limit| OpenOptions {
            query_memory_limit: Some(limit),
            ..OpenOptions::default()
        };
        assert!(msg(err(options(-1).to_core())).starts_with("[API-017]"));

        let dir = std::env::temp_dir().join(format!("minigraf_ffi_qml_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("dir");
        // A cartesian product holds one side in full.
        let product = "(query [:find ?a ?b :where [?a :n _] [?b :m _]])";
        for (name, limit, fails) in [("small", 64, true), ("max", i64::MAX, false)] {
            let path = dir.join(format!("{name}.graph"));
            let db =
                MiniGrafDb::open_with_options(path.to_str().expect("utf8").into(), options(limit))
                    .expect("open");
            db.execute("(transact [[:a :n 1] [:b :m 2]])".into())
                .expect("transact");
            let r = db.execute(product.into());
            if fails {
                let e = err(r);
                assert!(matches!(e, MiniGrafError::Query { .. }), "a query error");
                assert!(msg(e).starts_with("[QRY-012]"));
                let cursor = db.query(product.into()).expect("query");
                assert!(msg(err(cursor.next_batch(10))).starts_with("[QRY-012]"));
            } else {
                r.expect("no limit");
            }
        }
        let _ = std::fs::remove_dir_all(&dir);
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

    // ─── Prepared queries (#181) ─────────────────────────────────────────

    fn binds(pairs: &[(&str, MiniGrafBindValue)]) -> HashMap<String, MiniGrafBindValue> {
        pairs
            .iter()
            .map(|(k, v)| (k.to_string(), v.clone()))
            .collect()
    }

    fn int(n: i64) -> MiniGrafBindValue {
        MiniGrafBindValue::Value {
            value: MiniGrafValue::Int64 { value: n },
        }
    }

    fn result_rows(json: &str) -> usize {
        let v: serde_json::Value = serde_json::from_str(json).expect("json");
        v["results"].as_array().expect("results").len()
    }

    fn prepared_db() -> Arc<MiniGrafDb> {
        let db = MiniGrafDb::open_in_memory().expect("open");
        db.execute(
            r#"(transact [[:a :n 1] [:b :n 2] [:c :n 3]
                          [:a :s "x"] [:a :f 2.5] [:a :t true] [:a :k :kw] [:a :z nil]])"#
                .into(),
        )
        .expect("transact");
        db
    }

    #[test]
    fn prepared_query_runs_again_with_new_bindings() {
        let db = prepared_db();
        let pq = db
            .prepare("(query [:find ?e :where [?e :n $n]])".into())
            .expect("prepare");
        for n in 1..=3 {
            let r = pq.execute(binds(&[("n", int(n))])).expect("execute");
            assert_eq!(result_rows(&r), 1, "one entity per value");
        }
        let r = pq.execute(binds(&[("n", int(9))])).expect("execute");
        assert_eq!(result_rows(&r), 0, "no entity has 9");
    }

    #[test]
    fn every_bind_value_variant_binds() {
        let db = prepared_db();
        let a = minigraf::EntityId::new_v5(&minigraf::EntityId::NAMESPACE_OID, b":a").to_string();
        let value = |value| MiniGrafBindValue::Value { value };
        for (q, bind) in [
            ("(query [:find ?e :where [?e :n $v]])", int(1)),
            (
                "(query [:find ?e :where [?e :s $v]])",
                value(MiniGrafValue::Text { value: "x".into() }),
            ),
            (
                "(query [:find ?e :where [?e :f $v]])",
                value(MiniGrafValue::Float64 { value: 2.5 }),
            ),
            (
                "(query [:find ?e :where [?e :t $v]])",
                value(MiniGrafValue::Bool { value: true }),
            ),
            (
                "(query [:find ?e :where [?e :k $v]])",
                value(MiniGrafValue::Keyword {
                    value: ":kw".into(),
                }),
            ),
            (
                "(query [:find ?e :where [?e :z $v]])",
                value(MiniGrafValue::Null),
            ),
            (
                "(query [:find ?x :where [$v :n ?x]])",
                value(MiniGrafValue::Ref { value: a.clone() }),
            ),
            (
                "(query [:find ?x :where [$v :n ?x]])",
                MiniGrafBindValue::Entity { uuid: a.clone() },
            ),
            (
                "(query [:find ?x :as-of $v :where [?e :n ?x]])",
                MiniGrafBindValue::TxCount { value: 1 },
            ),
            (
                "(query [:find ?x :as-of $v :where [?e :n ?x]])",
                MiniGrafBindValue::Timestamp { value: i64::MAX },
            ),
            (
                "(query [:find ?x :valid-at $v :where [:a :n ?x]])",
                MiniGrafBindValue::AnyValidTime,
            ),
        ] {
            let pq = db.prepare(q.into()).expect("prepare");
            let r = pq.execute(binds(&[("v", bind)])).expect("execute");
            assert!(result_rows(&r) > 0, "the bind value matches");
        }
    }

    #[test]
    fn prepared_cursor_matches_execute() {
        let db = prepared_db();
        let pq = db
            .prepare("(query [:find ?e ?n :where [?e :n ?n] [(>= ?n $min)]])".into())
            .expect("prepare");
        let all = result_rows(&pq.execute(binds(&[("min", int(2))])).expect("execute"));
        let cursor = pq.query(binds(&[("min", int(2))])).expect("query");
        assert_eq!(cursor.vars(), vec!["?e".to_string(), "?n".to_string()]);
        let mut seen = 0;
        while let Some(batch) = cursor.next_batch(1).expect("batch") {
            seen += serde_json::from_str::<Vec<serde_json::Value>>(&batch)
                .expect("rows")
                .len();
        }
        assert_eq!(seen, all);
        assert_eq!(seen, 2);
    }

    #[test]
    fn prepared_bind_mistakes_are_api_errors() {
        let db = prepared_db();
        let api = |e: MiniGrafError, code: &str| {
            assert!(matches!(e, MiniGrafError::Api { .. }), "an API error");
            assert!(msg(e).starts_with(code), "the expected code");
        };
        let pq = db
            .prepare("(query [:find ?e :where [?e :n $n]])".into())
            .expect("prepare");
        api(err(pq.execute(binds(&[]))), "[API-022]");
        api(err(pq.query(binds(&[]))), "[API-022]");
        for wrong in [
            pq.execute(binds(&[("n", MiniGrafBindValue::TxCount { value: 1 })]))
                .map(|_| ()),
            pq.query(binds(&[("n", MiniGrafBindValue::TxCount { value: 1 })]))
                .map(|_| ()),
        ] {
            api(err(wrong), "[API-024]");
        }
        api(
            err(pq.query(binds(&[(
                "n",
                MiniGrafBindValue::Value {
                    value: MiniGrafValue::Ref {
                        value: "not-a-uuid".into(),
                    },
                },
            )]))),
            "[API-017]",
        );
        api(
            err(db.prepare("(query [:find ?v :where [?e $a ?v]])".into())),
            "[API-023]",
        );
        api(
            err(db.prepare("(transact [[:a :n 1]])".into())),
            "[API-005]",
        );

        let entity = db
            .prepare("(query [:find ?x :where [$e :n ?x]])".into())
            .expect("prepare");
        api(
            err(entity.execute(binds(&[(
                "e",
                MiniGrafBindValue::Entity {
                    uuid: "not-a-uuid".into(),
                },
            )]))),
            "[API-017]",
        );
        let as_of = db
            .prepare("(query [:find ?x :as-of $t :where [?e :n ?x]])".into())
            .expect("prepare");
        api(
            err(as_of.execute(binds(&[("t", MiniGrafBindValue::TxCount { value: -1 })]))),
            "[API-017]",
        );
    }

    #[test]
    fn prepared_query_keeps_its_database_alive() {
        let db = prepared_db();
        let pq = db
            .prepare("(query [:find ?e :where [?e :n $n]])".into())
            .expect("prepare");
        drop(db);
        let r = pq.execute(binds(&[("n", int(1))])).expect("execute");
        assert_eq!(result_rows(&r), 1);
    }

    #[test]
    fn prepared_query_sees_later_writes() {
        let db = prepared_db();
        let pq = db
            .prepare("(query [:find ?e :where [?e :n $n]])".into())
            .expect("prepare");
        assert_eq!(
            result_rows(&pq.execute(binds(&[("n", int(4))])).expect("execute")),
            0
        );
        db.execute("(transact [[:d :n 4]])".into())
            .expect("transact");
        assert_eq!(
            result_rows(&pq.execute(binds(&[("n", int(4))])).expect("execute")),
            1
        );
    }

    // ─── User functions (#180) ───────────────────────────────────────────

    struct IsEven;
    impl MiniGrafPredicate for IsEven {
        fn test(&self, value: MiniGrafValue) -> Result<bool, MiniGrafUdfError> {
            match value {
                MiniGrafValue::Int64 { value } => Ok(value % 2 == 0),
                _ => Err(MiniGrafUdfError::Failed {
                    msg: "not an integer".into(),
                }),
            }
        }
    }

    /// Sums integers; fails in `step` on `fail_on`, and `finish` returns
    /// `out` when set.
    struct Total {
        fail_on: Option<i64>,
        out: Option<MiniGrafValue>,
        steps: Arc<std::sync::atomic::AtomicUsize>,
    }
    struct TotalAcc {
        fail_on: Option<i64>,
        out: Option<MiniGrafValue>,
        steps: Arc<std::sync::atomic::AtomicUsize>,
        sum: Mutex<i64>,
    }
    impl MiniGrafAggregate for Total {
        fn accumulator(&self) -> Result<Arc<dyn MiniGrafAccumulator>, MiniGrafUdfError> {
            Ok(Arc::new(TotalAcc {
                fail_on: self.fail_on,
                out: self.out.clone(),
                steps: self.steps.clone(),
                sum: Mutex::new(0),
            }))
        }
    }
    impl MiniGrafAccumulator for TotalAcc {
        fn step(&self, value: MiniGrafValue) -> Result<(), MiniGrafUdfError> {
            self.steps.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            let MiniGrafValue::Int64 { value } = value else {
                return Err(MiniGrafUdfError::Failed {
                    msg: "not an integer".into(),
                });
            };
            if Some(value) == self.fail_on {
                return Err(MiniGrafUdfError::Failed {
                    msg: format!("refused {value}"),
                });
            }
            *self.sum.lock().unwrap() += value;
            Ok(())
        }
        fn finish(&self, count: i64) -> Result<MiniGrafValue, MiniGrafUdfError> {
            if let Some(out) = &self.out {
                return Ok(out.clone());
            }
            Ok(MiniGrafValue::Text {
                value: format!("{}/{count}", *self.sum.lock().unwrap()),
            })
        }
    }

    fn total(fail_on: Option<i64>, out: Option<MiniGrafValue>) -> Arc<Total> {
        Arc::new(Total {
            fail_on,
            out,
            steps: Arc::default(),
        })
    }

    fn udf_db() -> Arc<MiniGrafDb> {
        let db = MiniGrafDb::open_in_memory().expect("open");
        db.execute(
            "(transact [[:a :n 1] [:b :n 2] [:c :n 3] [:d :n 4] [:a :g 1] [:b :g 1] [:c :g 2] [:d :g 2]])"
                .into(),
        )
        .expect("transact");
        db
    }

    fn results(json: &str) -> Vec<serde_json::Value> {
        let v: serde_json::Value = serde_json::from_str(json).expect("json");
        v["results"].as_array().expect("results").clone()
    }

    #[test]
    fn predicate_filters_rows() {
        let db = udf_db();
        db.register_predicate("even?".into(), Arc::new(IsEven))
            .expect("register");
        let mut rows = results(
            &db.execute("(query [:find ?n :where [?e :n ?n] [(even? ?n)]])".into())
                .expect("query"),
        );
        rows.sort_by_key(|r| r[0].as_i64());
        assert_eq!(rows, vec![serde_json::json!([2]), serde_json::json!([4])]);
    }

    #[test]
    fn failing_predicate_is_qry_014() {
        let db = udf_db();
        db.execute(r#"(transact [[:e :n "x"]])"#.into())
            .expect("transact");
        db.register_predicate("even?".into(), Arc::new(IsEven))
            .expect("register");
        let e = err(db.execute("(query [:find ?n :where [?e :n ?n] [(even? ?n)]])".into()));
        let MiniGrafError::Query { msg } = e else {
            panic!("expected a query error");
        };
        assert!(msg.contains("QRY-014") && msg.contains("even?") && msg.contains("not an integer"));
    }

    #[test]
    fn aggregate_in_find_and_over() {
        let db = udf_db();
        db.register_aggregate("total".into(), total(None, None))
            .expect("register");
        let rows = results(
            &db.execute("(query [:find (total ?n) :where [?e :n ?n]])".into())
                .expect("find"),
        );
        assert_eq!(rows, vec![serde_json::json!(["10/4"])]);
        let mut rows = results(
            &db.execute(
                "(query [:find ?n (total ?n :over (:partition-by ?g :order-by ?n)) :where [?e :n ?n] [?e :g ?g]])"
                    .into(),
            )
            .expect("over"),
        );
        rows.sort_by_key(|r| r[0].as_i64());
        assert_eq!(
            rows,
            vec![
                serde_json::json!([1, "1/1"]),
                serde_json::json!([2, "3/2"]),
                serde_json::json!([3, "3/1"]),
                serde_json::json!([4, "7/2"]),
            ]
        );
    }

    #[test]
    fn failing_step_is_qry_014_and_stops() {
        let db = udf_db();
        let agg = total(Some(2), None);
        db.register_aggregate("total".into(), agg.clone())
            .expect("register");
        let e = err(db.execute("(query [:find (total ?n) :where [?e :n ?n]])".into()));
        let MiniGrafError::Query { msg } = e else {
            panic!("expected a query error");
        };
        assert!(msg.contains("QRY-014") && msg.contains("refused 2"));
        assert!(
            agg.steps.load(std::sync::atomic::Ordering::SeqCst) <= 4,
            "no step after the whole input"
        );
    }

    #[test]
    fn bad_ref_from_finish_is_qry_014() {
        let db = udf_db();
        let bad = MiniGrafValue::Ref {
            value: "nope".into(),
        };
        db.register_aggregate("total".into(), total(None, Some(bad)))
            .expect("register");
        let e = err(db.execute("(query [:find (total ?n) :where [?e :n ?n]])".into()));
        let MiniGrafError::Query { msg } = e else {
            panic!("expected a query error");
        };
        assert!(msg.contains("QRY-014") && msg.contains(r#"ref value "nope" is not a UUID"#));
    }

    #[test]
    fn finish_ref_reaches_the_result() {
        let db = udf_db();
        let id = "6f1c2a3b-4d5e-4f60-8172-839ab4c5d6e7".to_string();
        db.register_aggregate(
            "total".into(),
            total(None, Some(MiniGrafValue::Ref { value: id.clone() })),
        )
        .expect("register");
        let rows = results(
            &db.execute("(query [:find (total ?n) :where [?e :n ?n]])".into())
                .expect("find"),
        );
        assert_eq!(rows, vec![serde_json::json!([id])]);
    }

    /// Queries the database it is registered on from inside the call.
    struct Reenter(std::sync::Weak<MiniGrafDb>, std::sync::atomic::AtomicBool);
    impl MiniGrafPredicate for Reenter {
        fn test(&self, value: MiniGrafValue) -> Result<bool, MiniGrafUdfError> {
            let db = self.0.upgrade().ok_or(MiniGrafUdfError::Failed {
                msg: "db gone".into(),
            })?;
            let MiniGrafValue::Int64 { value } = value else {
                return Ok(false);
            };
            let fail = |e: MiniGrafError| MiniGrafUdfError::Failed { msg: e.to_string() };
            let json = db
                .execute(format!(
                    "(query [:find ?e :where [?e :n {value}] [?e :g 2]])"
                ))
                .map_err(fail)?;
            let cursor = db
                .query("(query [:find ?n :where [?e :n ?n]])".into())
                .map_err(fail)?;
            let _ = cursor.next_batch(10).map_err(fail)?;
            if !self.1.swap(true, std::sync::atomic::Ordering::SeqCst) {
                db.register_predicate("late?".into(), Arc::new(IsEven))
                    .map_err(fail)?;
            }
            Ok(!results(&json).is_empty())
        }
    }

    #[test]
    fn predicate_can_query_its_own_database() {
        let db = udf_db();
        db.register_predicate(
            "g2?".into(),
            Arc::new(Reenter(Arc::downgrade(&db), Default::default())),
        )
        .expect("register");
        let (tx, rx) = std::sync::mpsc::channel();
        let db2 = db.clone();
        std::thread::spawn(move || {
            let r = db2.execute("(query [:find ?n :where [?e :n ?n] [(g2? ?n)]])".into());
            let _ = tx.send(r.map(|json| results(&json)));
        });
        let mut rows = rx
            .recv_timeout(std::time::Duration::from_secs(30))
            .expect("no deadlock")
            .expect("query");
        rows.sort_by_key(|r| r[0].as_i64());
        assert_eq!(rows, vec![serde_json::json!([3]), serde_json::json!([4])]);
        let later = db
            .execute("(query [:find ?n :where [?e :n ?n] [(late? ?n)]])".into())
            .expect("registered from inside the call");
        assert_eq!(results(&later).len(), 2);
    }

    #[test]
    fn registering_a_name_twice_is_int_040() {
        let db = udf_db();
        db.register_predicate("even?".into(), Arc::new(IsEven))
            .expect("register");
        let e = err(db.register_predicate("even?".into(), Arc::new(IsEven)));
        assert!(msg(e).starts_with("[INT-040]"));
        db.register_aggregate("total".into(), total(None, None))
            .expect("aggregates have their own names");
        let e = err(db.register_aggregate("total".into(), total(None, None)));
        assert!(msg(e).starts_with("[INT-040]"));
    }

    #[test]
    fn unexpected_callback_error_becomes_failed() {
        let e = MiniGrafUdfError::from(uniffi::UnexpectedUniFFICallbackError {
            reason: "boom".into(),
        });
        assert_eq!(e.to_string(), "boom");
        let e = MiniGrafUdfError::from(uniffi::UnexpectedUniFFICallbackError {
            reason: "java.lang.IllegalStateException: no\n\tat Foo.test(Foo.kt:3)\n".into(),
        });
        assert_eq!(e.to_string(), "java.lang.IllegalStateException: no");
    }

    // ─── Overrides, open options and prepared setters (minigraf#872) ─────────

    const REACH: &str = "(query [:find ?x ?y :where (reach ?x ?y)])";
    const GROUPED: &str = "(query [:find ?v (count ?e) :where [?e :v ?v]])";

    fn temp_dir(name: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!("minigraf_ffi_{name}_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("dir");
        dir
    }

    fn graph(dir: &std::path::Path, name: &str) -> String {
        dir.join(format!("{name}.graph"))
            .to_str()
            .expect("utf8")
            .into()
    }

    /// A 4-edge chain (10 `reach` pairs) and 3 grouped values.
    fn load_reach(db: &MiniGrafDb) {
        db.execute(
            "(transact [[:a :edge :b] [:b :edge :c] [:c :edge :d] [:d :edge :e] \
             [:p :v 1] [:q :v 1] [:r :v 2]])"
                .into(),
        )
        .expect("transact");
        db.execute("(rule [(reach ?x ?y) [?x :edge ?y]])".into())
            .expect("rule");
        db.execute("(rule [(reach ?x ?y) [?x :edge ?z] (reach ?z ?y)])".into())
            .expect("rule");
    }

    fn reach_db(dir: &std::path::Path, name: &str, options: OpenOptions) -> Arc<MiniGrafDb> {
        let db = MiniGrafDb::open_with_options(graph(dir, name), options).expect("open");
        load_reach(&db);
        db
    }

    fn with_overrides(list: Option<Vec<Overridable>>) -> OpenOptions {
        OpenOptions {
            allow_overrides: list,
            ..OpenOptions::default()
        }
    }

    fn code_of<T>(r: Result<T, MiniGrafError>) -> String {
        msg(err(r))[1..8].to_string()
    }

    #[test]
    fn overridable_defaults_are_the_core_defaults() {
        assert_eq!(
            overridable_defaults(),
            vec![
                Overridable::MaxDerivedFacts,
                Overridable::MaxResults,
                Overridable::MaxIterations
            ]
        );
    }

    #[test]
    fn allow_overrides_replaces_the_defaults() {
        let dir = temp_dir("overrides");
        let derived = "(query [:find ?x ?y :where (reach ?x ?y) :max-derived-facts 100])";
        let memory = "(query [:find ?x ?y :where (reach ?x ?y) :query-memory-limit 1000000])";
        let cases = [
            ("absent", None, true, false),
            ("defaults", Some(overridable_defaults()), true, false),
            ("empty", Some(vec![]), false, false),
            (
                "memory",
                Some(vec![
                    Overridable::QueryMemoryLimit,
                    Overridable::QueryMemoryLimit,
                ]),
                false,
                true,
            ),
            (
                "all",
                Some(vec![
                    Overridable::MaxDerivedFacts,
                    Overridable::MaxResults,
                    Overridable::QueryMemoryLimit,
                    Overridable::MaxIterations,
                ]),
                true,
                true,
            ),
        ];
        for (name, list, derived_ok, memory_ok) in cases {
            let db = reach_db(&dir, name, with_overrides(list));
            for (q, ok) in [(derived, derived_ok), (memory, memory_ok)] {
                if ok {
                    assert_eq!(results(&db.execute(q.into()).expect(name)).len(), 10);
                    db.query(q.into()).expect(name);
                    db.prepare(q.into()).expect(name);
                } else {
                    assert_eq!(code_of(db.execute(q.into())), "API-025", "{name}");
                    assert_eq!(code_of(db.query(q.into())), "API-025", "{name}");
                    assert_eq!(code_of(db.prepare(q.into())), "API-025", "{name}");
                }
            }
        }
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn transaction_chunk_facts_writes_a_large_transaction() {
        let dir = temp_dir("chunk");
        let path = graph(&dir, "chunk");
        let options = |n| OpenOptions {
            transaction_chunk_facts: Some(n),
            ..OpenOptions::default()
        };
        assert_eq!(code_of(options(-1).to_core()), "API-017");
        let db = MiniGrafDb::open_with_options(path.clone(), options(2)).expect("open");
        let facts: String = (0..10).map(|i| format!("[:e{i} :n {i}]")).collect();
        db.execute(format!("(transact [{facts}])"))
            .expect("transact");
        drop(db);
        let db = MiniGrafDb::open(path).expect("reopen");
        let rows = db
            .execute("(query [:find ?n :where [?e :n ?n]])".into())
            .expect("query");
        assert_eq!(results(&rows).len(), 10);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn max_iterations_option_limits_rule_rounds() {
        let dir = temp_dir("iterations");
        let options = |n| OpenOptions {
            max_iterations: Some(n),
            ..OpenOptions::default()
        };
        assert_eq!(code_of(options(-1).to_core()), "API-017");
        let db = reach_db(&dir, "default", OpenOptions::default());
        assert_eq!(
            results(&db.execute(REACH.into()).expect("default")).len(),
            10
        );
        let db = reach_db(&dir, "one", options(1));
        assert_eq!(code_of(db.execute(REACH.into())), "INT-020");
        let db = reach_db(&dir, "max", options(i64::MAX));
        assert_eq!(
            results(&db.execute(REACH.into()).expect("no limit")).len(),
            10
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn log_writer_accepts_the_new_options() {
        let dir = temp_dir("writer_options");
        let out = graph(&dir, "out");
        let options = OpenOptions {
            allow_overrides: Some(vec![]),
            transaction_chunk_facts: Some(2),
            max_iterations: Some(5),
            ..OpenOptions::default()
        };
        let w = MiniGrafLogWriter::create(out.clone(), options).expect("create");
        w.finish().expect("finish");
        assert!(std::path::Path::new(&out).exists());
        let bad = OpenOptions {
            max_iterations: Some(-1),
            ..OpenOptions::default()
        };
        assert_eq!(
            code_of(MiniGrafLogWriter::create(graph(&dir, "bad"), bad)),
            "API-017"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn prepared_setters_derive_new_queries() {
        let dir = temp_dir("setters");
        // The setters are not gated: every override keyword is refused here.
        let db = reach_db(&dir, "setters", with_overrides(Some(vec![])));
        let none = HashMap::new;
        let pq = db.prepare(REACH.into()).expect("prepare");
        let mut cursor_rows = 0;
        let cursor = pq.query(none()).expect("cursor");
        for derived in [
            pq.with_max_derived_facts(1).expect("setter"),
            pq.with_max_iterations(1).expect("setter"),
        ] {
            assert_eq!(code_of(derived.execute(none())), "INT-020");
            assert_eq!(code_of(derived.query(none())), "INT-020");
        }
        // Deriving did not disturb a cursor open on the receiver.
        while let Some(batch) = cursor.next_batch(3).expect("batch") {
            cursor_rows += serde_json::from_str::<Vec<serde_json::Value>>(&batch)
                .expect("rows")
                .len();
        }
        assert_eq!(cursor_rows, 10);
        assert_eq!(results(&pq.execute(none()).expect("receiver")).len(), 10);

        let grouped = db.prepare(GROUPED.into()).expect("prepare");
        let small = grouped.with_query_memory_limit(16).expect("setter");
        assert_eq!(code_of(small.execute(none())), "QRY-012");
        assert_eq!(
            results(&grouped.execute(none()).expect("receiver")).len(),
            2
        );
        let unlimited = small.with_query_memory_limit(i64::MAX).expect("setter");
        assert_eq!(
            results(&unlimited.execute(none()).expect("no limit")).len(),
            2
        );

        // The last value for a limit applies; the others are kept.
        let chained = pq
            .with_max_derived_facts(1)
            .and_then(|q| q.with_max_iterations(1))
            .and_then(|q| q.with_max_iterations(i64::MAX))
            .expect("setters");
        assert_eq!(code_of(chained.execute(none())), "INT-020");
        let raised = chained.with_max_derived_facts(i64::MAX).expect("setter");
        assert_eq!(results(&raised.execute(none()).expect("raised")).len(), 10);

        for r in [
            pq.with_max_derived_facts(-1),
            pq.with_query_memory_limit(-1),
            pq.with_max_iterations(-1),
        ] {
            assert_eq!(code_of(r), "API-017");
        }
        // A derived query keeps its database open.
        let derived = pq.with_max_iterations(10).expect("setter");
        drop(pq);
        drop(db);
        assert_eq!(
            results(&derived.execute(none()).expect("db alive")).len(),
            10
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    // ── Write transactions (minigraf#824) ────────────────────────────────────

    const NAMES: &str = "(query [:find ?n :where [?e :person/name ?n]])";
    const TX_SETUP: &str =
        r#"(transact [[:alice :person/name "Alice"] [:bob :person/name "Bob"]])"#;

    /// The sorted names a query JSON returns.
    fn names_of(json: &str) -> Vec<String> {
        let v: serde_json::Value = serde_json::from_str(json).expect("json");
        let mut names: Vec<String> = v["results"]
            .as_array()
            .expect("results")
            .iter()
            .map(|r| r[0].as_str().expect("a name").to_string())
            .collect();
        names.sort();
        names
    }

    fn db_names(db: &MiniGrafDb) -> Vec<String> {
        names_of(&db.execute(NAMES.into()).expect("query"))
    }

    /// The `XXX-NNN` code an error message carries.
    fn code(e: &MiniGrafError) -> String {
        let s = e.to_string();
        let start = s.find('[').map_or(0, |i| i + 1);
        s[start..].chars().take(7).collect()
    }

    fn tx_db() -> Arc<MiniGrafDb> {
        let db = MiniGrafDb::open_in_memory().expect("open");
        db.execute(TX_SETUP.into()).expect("setup");
        db
    }

    /// Run `f` on a new thread; `Some` of its result if it returns within `t`.
    fn within<T: Send + 'static>(
        t: std::time::Duration,
        f: impl FnOnce() -> T + Send + 'static,
    ) -> (Option<T>, std::sync::mpsc::Receiver<T>) {
        let (send, recv) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            let _ = send.send(f());
        });
        match recv.recv_timeout(t) {
            Ok(v) => (Some(v), recv),
            Err(_) => (None, recv),
        }
    }

    const WAITS: std::time::Duration = std::time::Duration::from_millis(300);
    const STUCK: std::time::Duration = std::time::Duration::from_secs(30);

    #[test]
    fn tx_reads_its_own_writes_and_commits() {
        let dir = temp_dir("tx_commit");
        let path = graph(&dir, "g");
        {
            let db = MiniGrafDb::open(path.clone()).expect("open");
            db.execute(TX_SETUP.into()).expect("setup");
            let tx = db.begin_write().expect("begin");
            assert_eq!(
                tx.execute(r#"(transact [[:carol :person/name "Carol"]])"#.into())
                    .expect("transact"),
                r#"{"ok":true}"#
            );
            assert_eq!(
                tx.execute(r#"(retract [[:bob :person/name "Bob"]])"#.into())
                    .expect("retract"),
                r#"{"ok":true}"#
            );
            assert_eq!(
                names_of(&tx.execute(NAMES.into()).expect("query in tx")),
                ["Alice", "Carol"]
            );
            // Another thread reads the committed state meanwhile.
            let db2 = db.clone();
            let (outside, _) = within(STUCK, move || db_names(&db2));
            assert_eq!(outside.expect("a query does not wait"), ["Alice", "Bob"]);
            tx.commit().expect("commit");
            assert_eq!(db_names(&db), ["Alice", "Carol"]);
            assert!(!tx.is_open());
        }
        let db = MiniGrafDb::open(path).expect("reopen");
        assert_eq!(db_names(&db), ["Alice", "Carol"], "after reopening");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn tx_rollback_leaves_the_database_as_it_was() {
        let dir = temp_dir("tx_rollback");
        let path = graph(&dir, "g");
        {
            let db = MiniGrafDb::open(path.clone()).expect("open");
            db.execute(TX_SETUP.into()).expect("setup");
            let tx = db.begin_write().expect("begin");
            tx.execute(r#"(transact [[:carol :person/name "Carol"]])"#.into())
                .expect("transact");
            tx.execute(r#"(retract [[:bob :person/name "Bob"]])"#.into())
                .expect("retract");
            tx.rollback().expect("rollback");
            assert_eq!(db_names(&db), ["Alice", "Bob"]);
        }
        let db = MiniGrafDb::open(path).expect("reopen");
        assert_eq!(db_names(&db), ["Alice", "Bob"], "after reopening");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn tx_failed_statement_stages_nothing_and_stays_open() {
        let db = tx_db();
        let tx = db.begin_write().expect("begin");
        let parse = tx.execute("(transact [[:a".into()).expect_err("parse");
        assert_eq!(code(&parse), "PRS-004");
        let windows = tx
            .execute(
                r#"(transact [[:x :p/v 1 {:valid-from "2020-01-01T00:00:00Z"}]
                    [:x :p/v 1 {:valid-from "2021-01-01T00:00:00Z"}]])"#
                    .into(),
            )
            .expect_err("two windows of a triple");
        assert_eq!(code(&windows), "API-011");
        tx.execute(r#"(transact [[:carol :person/name "Carol"]])"#.into())
            .expect("still open");
        tx.commit().expect("commit");
        assert_eq!(db_names(&db), ["Alice", "Bob", "Carol"]);
    }

    #[test]
    fn tx_finished_is_api018() {
        let db = tx_db();
        let committed = db.begin_write().expect("begin");
        committed.commit().expect("commit");
        let rolled_back = db.begin_write().expect("begin");
        rolled_back.rollback().expect("rollback");
        for tx in [&committed, &rolled_back] {
            assert_eq!(
                code(&tx.execute(NAMES.into()).expect_err("execute")),
                "API-018"
            );
            assert_eq!(code(&tx.commit().expect_err("commit")), "API-018");
            tx.rollback().expect("rollback does nothing");
        }

        // A commit that fails finishes the transaction: a valid-time window
        // still ahead when staged has ended at the commit time (API-019).
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("time")
            .as_secs();
        let end = now + 2;
        let tx = db.begin_write().expect("begin");
        tx.execute(format!(
            "(transact [[:dave :person/name \"Dave\" {{:valid-to {}}}]])",
            rfc3339(end)
        ))
        .expect("the window is ahead when staged");
        while std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("time")
            .as_secs()
            <= end
        {
            std::thread::sleep(std::time::Duration::from_millis(50));
        }
        assert_eq!(code(&tx.commit().expect_err("ended")), "API-019");
        assert_eq!(
            code(&tx.execute(NAMES.into()).expect_err("finished")),
            "API-018"
        );
        db.execute(r#"(transact [[:erin :person/name "Erin"]])"#.into())
            .expect("the failed commit released the write lock");
        assert_eq!(db_names(&db), ["Alice", "Bob", "Erin"]);
    }

    /// `secs` since the epoch as an RFC 3339 UTC string (whole seconds).
    fn rfc3339(secs: u64) -> String {
        let days = secs / 86_400;
        let rem = secs % 86_400;
        // Civil-from-days (Howard Hinnant), for 1970 onwards.
        let z = days + 719_468;
        let era = z / 146_097;
        let doe = z - era * 146_097;
        let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
        let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
        let mp = (5 * doy + 2) / 153;
        let d = doy - (153 * mp + 2) / 5 + 1;
        let m = if mp < 10 { mp + 3 } else { mp - 9 };
        let y = yoe + era * 400 + u64::from(m <= 2);
        format!(
            "\"{y:04}-{m:02}-{d:02}T{:02}:{:02}:{:02}Z\"",
            rem / 3600,
            rem % 3600 / 60,
            rem % 60
        )
    }

    /// After `release` on another thread, writes through the database
    /// succeed from that thread and from this one.
    fn assert_lock_released(db: &Arc<MiniGrafDb>, what: &str) {
        let db2 = db.clone();
        let (other, _) = within(STUCK, move || {
            db2.execute(r#"(transact [[:t :p/n "other thread"]])"#.into())
                .map(|_| ())
        });
        assert!(
            other.expect("not stuck").is_ok(),
            "{what}: another thread writes"
        );
        db.execute(r#"(transact [[:t :p/n "this thread"]])"#.into())
            .unwrap_or_else(|_| panic!("{what}: the begin thread writes"));
    }

    #[test]
    fn tx_dropped_open_rolls_back_and_releases_the_lock() {
        let db = tx_db();
        // Dropped on the begin thread.
        let tx = db.begin_write().expect("begin");
        tx.execute(r#"(transact [[:carol :person/name "Carol"]])"#.into())
            .expect("transact");
        drop(tx);
        assert_eq!(db_names(&db), ["Alice", "Bob"]);
        assert_lock_released(&db, "dropped on the begin thread");

        // The last reference dropped on another thread, as a collector does.
        let tx = db.begin_write().expect("begin");
        tx.execute(r#"(transact [[:dave :person/name "Dave"]])"#.into())
            .expect("transact");
        std::thread::spawn(move || drop(tx)).join().expect("join");
        assert_eq!(db_names(&db), ["Alice", "Bob"]);
        assert_lock_released(&db, "dropped on another thread");
    }

    #[test]
    fn tx_guards_the_begin_thread_and_others_wait() {
        let dir = temp_dir("tx_guard");
        let db = MiniGrafDb::open(graph(&dir, "g")).expect("open");
        db.execute(TX_SETUP.into()).expect("setup");
        let pq = db.prepare(NAMES.into()).expect("prepare");
        let tx = db.begin_write().expect("begin");
        tx.execute(r#"(transact [[:carol :person/name "Carol"]])"#.into())
            .expect("transact");

        let refused: Vec<(&str, Result<(), MiniGrafError>)> = vec![
            ("execute", db.execute(NAMES.into()).map(|_| ())),
            ("query", db.query(NAMES.into()).map(|_| ())),
            ("prepare", db.prepare(NAMES.into()).map(|_| ())),
            ("fact_log", db.fact_log(FactFilter::default()).map(|_| ())),
            ("checkpoint", db.checkpoint()),
            ("begin_write", db.begin_write().map(|_| ())),
            ("prepared execute", pq.execute(HashMap::new()).map(|_| ())),
        ];
        for (name, result) in refused {
            let err = result.expect_err(name);
            assert_eq!(code(&err), "INT-001", "{name} on the begin thread");
        }

        // A write and a begin from another thread wait; a query does not,
        // also while a begin is waiting.
        let db2 = db.clone();
        let (early_write, write) = within(WAITS, move || {
            db2.execute(r#"(transact [[:w :person/name "W"]])"#.into())
                .map(|_| ())
        });
        assert!(early_write.is_none(), "a write from another thread waits");
        let db3 = db.clone();
        let (early_begin, begin) = within(WAITS, move || {
            let tx = db3.begin_write()?;
            tx.execute(r#"(transact [[:b :person/name "B"]])"#.into())?;
            tx.commit()
        });
        assert!(early_begin.is_none(), "a begin from another thread waits");
        let db4 = db.clone();
        let (query, _) = within(STUCK, move || db_names(&db4));
        assert_eq!(
            query.expect("a query runs while a begin waits"),
            ["Alice", "Bob"]
        );

        tx.commit().expect("commit");
        write
            .recv_timeout(STUCK)
            .expect("the write ends")
            .expect("it succeeds");
        begin
            .recv_timeout(STUCK)
            .expect("the begin ends")
            .expect("it succeeds");
        assert_eq!(db_names(&db), ["Alice", "B", "Bob", "Carol", "W"]);
        drop(db);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn tx_used_from_several_threads() {
        let db = tx_db();
        let tx = db.begin_write().expect("begin");
        let handles: Vec<_> = (0..8)
            .map(|i| {
                let tx = tx.clone();
                std::thread::spawn(move || {
                    for j in 0..20 {
                        tx.execute(format!("(transact [[:t{i}_{j} :p/n {j}]])"))
                            .expect("a whole statement");
                        tx.execute("(query [:find (count ?e) :where [?e :p/n _]])".into())
                            .expect("a whole query");
                    }
                })
            })
            .collect();
        for h in handles {
            h.join().expect("join");
        }
        // Committed on another thread than the one that began it.
        let tx2 = tx.clone();
        std::thread::spawn(move || tx2.commit())
            .join()
            .expect("join")
            .expect("commit");
        let count = db
            .execute("(query [:find (count ?e) :where [?e :p/n _]])".into())
            .expect("count");
        assert!(count.contains("[[160]]"), "every statement committed");
        assert_lock_released(&db, "committed on another thread");

        // A call racing a commit either runs before it or fails with API-018.
        let tx = db.begin_write().expect("begin");
        let racers: Vec<_> = (0..4)
            .map(|i| {
                let tx = tx.clone();
                std::thread::spawn(move || {
                    (0..50)
                        .map(|j| tx.execute(format!("(transact [[:r{i}_{j} :p/r {j}]])")))
                        .filter_map(Result::err)
                        .map(|e| code(&e))
                        .collect::<Vec<_>>()
                })
            })
            .collect();
        tx.commit().expect("commit");
        for r in racers {
            for c in r.join().expect("join") {
                assert_eq!(c, "API-018", "a call that loses the race");
            }
        }
        assert_lock_released(&db, "after the race");
    }

    #[test]
    fn tx_options_and_read_only() {
        let dir = temp_dir("tx_options");
        let path = graph(&dir, "g");
        {
            let db = MiniGrafDb::open(path.clone()).expect("open");
            db.execute(TX_SETUP.into()).expect("setup");
            let err = db
                .begin_write_with(TxOptions {
                    synchronous: None,
                    chunk_facts: Some(-1),
                })
                .err()
                .expect("negative chunk_facts");
            assert_eq!(code(&err), "API-017");
            // A small chunk: the transaction becomes large and commits as a
            // checkpoint.
            let tx = db
                .begin_write_with(TxOptions {
                    synchronous: Some(SyncMode::Normal),
                    chunk_facts: Some(3),
                })
                .expect("begin");
            for i in 0..4 {
                tx.execute(format!(
                    "(transact [[:c{i}a :person/name \"C{i}a\"] [:c{i}b :person/name \"C{i}b\"]])"
                ))
                .expect("transact");
            }
            assert_eq!(
                names_of(&tx.execute(NAMES.into()).expect("query")).len(),
                10
            );
            tx.commit().expect("commit");
            let tx = db
                .begin_write_with(TxOptions {
                    synchronous: Some(SyncMode::Full),
                    chunk_facts: None,
                })
                .expect("begin");
            tx.execute(r#"(transact [[:f :person/name "F"]])"#.into())
                .expect("transact");
            tx.commit().expect("commit");
        }
        {
            let db = MiniGrafDb::open(path.clone()).expect("reopen");
            assert_eq!(db_names(&db).len(), 11, "every fact after reopening");
        }
        let ro = MiniGrafDb::open_with_options(
            path,
            OpenOptions {
                read_only: Some(true),
                ..OpenOptions::default()
            },
        )
        .expect("read-only");
        assert_eq!(code(&ro.begin_write().err().expect("read-only")), "API-014");
        assert_eq!(db_names(&ro).len(), 11, "still readable");
        let mem = MiniGrafDb::open_in_memory().expect("open");
        mem.begin_write_with(TxOptions {
            synchronous: Some(SyncMode::Normal),
            chunk_facts: Some(1),
        })
        .expect("in memory, accepted and ignored")
        .commit()
        .expect("commit");
        drop(ro);
        let _ = std::fs::remove_dir_all(&dir);
    }

    struct SeesStaged;
    impl MiniGrafPredicate for SeesStaged {
        fn test(&self, value: MiniGrafValue) -> Result<bool, MiniGrafUdfError> {
            Ok(value
                == MiniGrafValue::Text {
                    value: "Carol".into(),
                })
        }
    }

    struct CallsBack(Arc<MiniGrafDb>);
    impl MiniGrafPredicate for CallsBack {
        fn test(&self, _value: MiniGrafValue) -> Result<bool, MiniGrafUdfError> {
            self.0
                .execute(NAMES.into())
                .map(|_| true)
                .map_err(|e| MiniGrafUdfError::Failed { msg: e.to_string() })
        }
    }

    #[test]
    fn tx_user_functions() {
        let db = tx_db();
        db.register_predicate("staged?".into(), Arc::new(SeesStaged))
            .expect("register");
        db.register_predicate("calls-back?".into(), Arc::new(CallsBack(db.clone())))
            .expect("register");
        let tx = db.begin_write().expect("begin");
        tx.execute(r#"(transact [[:carol :person/name "Carol"]])"#.into())
            .expect("transact");
        let seen = tx
            .execute("(query [:find ?n :where [?e :person/name ?n] [(staged? ?n)]])".into())
            .expect("a user function sees staged values");
        assert_eq!(names_of(&seen), ["Carol"]);
        let err = tx
            .execute("(query [:find ?n :where [?e :person/name ?n] [(calls-back? ?n)]])".into())
            .expect_err("a guarded call from inside the transaction");
        assert_eq!(code(&err), "QRY-014");
        assert!(err.to_string().contains("INT-001"), "the cause is INT-001");
        tx.commit().expect("the transaction still commits");
        assert_eq!(db_names(&db), ["Alice", "Bob", "Carol"]);
    }

    /// Calls the transaction it runs in (minigraf#886) and records what each
    /// call returned.
    #[derive(Default)]
    struct CallsItsTx {
        tx: Mutex<Option<Arc<MiniGrafWriteTransaction>>>,
        seen: Mutex<Vec<(&'static str, String)>>,
        fail: bool,
    }
    impl MiniGrafPredicate for CallsItsTx {
        fn test(&self, _value: MiniGrafValue) -> Result<bool, MiniGrafUdfError> {
            let tx = self.tx.lock().expect("tx").clone().expect("a transaction");
            let calls: [(&'static str, Result<(), MiniGrafError>); 3] = [
                ("execute", tx.execute(NAMES.into()).map(|_| ())),
                ("commit", tx.commit()),
                ("rollback", tx.rollback()),
            ];
            let mut first = None;
            let mut seen = self.seen.lock().expect("seen");
            for (name, result) in calls {
                let text = result.err().map(|e| e.to_string()).unwrap_or_default();
                first.get_or_insert_with(|| text.clone());
                seen.push((name, text));
            }
            seen.push(("is_open", tx.is_open().to_string()));
            if self.fail {
                return Err(MiniGrafUdfError::Failed {
                    msg: first.unwrap_or_default(),
                });
            }
            Ok(true)
        }
    }

    const CALLS_TX: &str = "(query [:find ?n :where [?e :person/name ?n] [(calls-tx? ?n)]])";

    /// A transaction on `db` with Carol staged and `calls-tx?` calling it.
    fn tx_calling_itself(
        db: &Arc<MiniGrafDb>,
        fail: bool,
    ) -> (Arc<MiniGrafWriteTransaction>, Arc<CallsItsTx>) {
        let udf = Arc::new(CallsItsTx {
            fail,
            ..Default::default()
        });
        db.register_predicate("calls-tx?".into(), udf.clone())
            .expect("register");
        let tx = db.begin_write().expect("begin");
        tx.execute(r#"(transact [[:carol :person/name "Carol"]])"#.into())
            .expect("transact");
        *udf.tx.lock().expect("tx") = Some(tx.clone());
        (tx, udf)
    }

    #[test]
    fn tx_refuses_a_call_from_its_own_statement() {
        let db = tx_db();
        let (tx, udf) = tx_calling_itself(&db, false);
        let tx2 = tx.clone();
        let (result, _recv) = within(STUCK, move || tx2.execute(CALLS_TX.into()));
        let rows = result
            .expect("the statement returns: no deadlock")
            .expect("the function swallowed the refusals");
        assert_eq!(names_of(&rows), ["Alice", "Bob", "Carol"]);
        let seen = udf.seen.lock().expect("seen");
        assert_eq!(seen.len(), 12, "four calls for each of three rows");
        for (name, text) in seen.iter() {
            if *name == "is_open" {
                assert_eq!(text, "true", "open while its statement runs");
            } else {
                assert!(text.contains("[INT-001]"), "{name} is refused with INT-001");
            }
        }
        drop(seen);
        *udf.tx.lock().expect("tx") = None;
        // Neither the refused commit nor the refused rollback finished it.
        assert!(tx.is_open());
        tx.commit().expect("the transaction still commits");
        assert_eq!(db_names(&db), ["Alice", "Bob", "Carol"]);
    }

    #[test]
    fn tx_refused_call_fails_the_query_when_passed_on() {
        let db = tx_db();
        let (tx, udf) = tx_calling_itself(&db, true);
        let tx2 = tx.clone();
        let (result, _recv) = within(STUCK, move || tx2.execute(CALLS_TX.into()));
        let err = result
            .expect("the statement returns: no deadlock")
            .expect_err("the function passed the refusal on");
        assert_eq!(code(&err), "QRY-014");
        assert!(err.to_string().contains("INT-001"), "the cause is INT-001");
        *udf.tx.lock().expect("tx") = None;
        tx.rollback().expect("rollback");
        assert_eq!(db_names(&db), ["Alice", "Bob"]);
    }

    /// Blocks inside a statement until told to go on.
    struct Waits {
        entered: Mutex<std::sync::mpsc::Sender<()>>,
        go_on: Mutex<std::sync::mpsc::Receiver<()>>,
    }
    impl MiniGrafPredicate for Waits {
        fn test(&self, _value: MiniGrafValue) -> Result<bool, MiniGrafUdfError> {
            let _ = self.entered.lock().expect("entered").send(());
            let _ = self.go_on.lock().expect("go_on").recv();
            Ok(true)
        }
    }

    #[test]
    fn tx_call_from_another_thread_waits_for_the_statement() {
        let db = tx_db();
        let (entered, has_entered) = std::sync::mpsc::channel();
        let (go_on, may_go_on) = std::sync::mpsc::channel();
        db.register_predicate(
            "waits?".into(),
            Arc::new(Waits {
                entered: Mutex::new(entered),
                go_on: Mutex::new(may_go_on),
            }),
        )
        .expect("register");
        let tx = db.begin_write().expect("begin");
        let tx2 = tx.clone();
        let statement = std::thread::spawn(move || {
            tx2.execute(
                r#"(query [:find ?n :where [:alice :person/name ?n] [(waits? ?n)]])"#.into(),
            )
        });
        has_entered.recv_timeout(STUCK).expect("the statement runs");
        // This thread is not the one running the statement: it waits.
        let tx3 = tx.clone();
        let (early, later) = within(WAITS, move || {
            tx3.execute(r#"(transact [[:carol :person/name "Carol"]])"#.into())
        });
        assert!(early.is_none(), "a call from another thread waits");
        go_on.send(()).expect("go on");
        statement.join().expect("join").expect("the statement");
        later
            .recv_timeout(STUCK)
            .expect("the waiting call runs")
            .expect("transact");
        tx.commit().expect("commit");
        assert_eq!(db_names(&db), ["Alice", "Bob", "Carol"]);
    }

    #[test]
    fn tx_rules_stay_after_rollback() {
        let db = tx_db();
        let tx = db.begin_write().expect("begin");
        tx.execute("(rule [(named ?e ?n) [?e :person/name ?n]])".into())
            .expect("rule");
        tx.rollback().expect("rollback");
        let r = db
            .execute("(query [:find ?n :where (named ?e ?n)])".into())
            .expect("the rule is still registered");
        assert_eq!(names_of(&r), ["Alice", "Bob"]);
    }

    #[test]
    fn tx_empty_and_query_only() {
        let db = tx_db();
        let before = db.current_tx_count().expect("count");
        db.begin_write().expect("begin").commit().expect("empty");
        let tx = db.begin_write().expect("begin");
        tx.execute(NAMES.into()).expect("query");
        tx.commit().expect("query only");
        assert_eq!(db.current_tx_count().expect("count"), before);
    }

    #[test]
    fn tx_keeps_its_database_open() {
        let dir = temp_dir("tx_keeps_open");
        let path = graph(&dir, "g");
        let tx = {
            let db = MiniGrafDb::open(path.clone()).expect("open");
            db.execute(TX_SETUP.into()).expect("setup");
            let tx = db.begin_write().expect("begin");
            tx.execute(r#"(transact [[:carol :person/name "Carol"]])"#.into())
                .expect("transact");
            tx
            // Every MiniGrafDb reference is dropped here.
        };
        tx.execute(r#"(transact [[:dave :person/name "Dave"]])"#.into())
            .expect("the transaction still runs");
        tx.commit()
            .expect("commit after the database object is gone");
        drop(tx);
        let db = MiniGrafDb::open(path).expect("reopen");
        assert_eq!(db_names(&db), ["Alice", "Bob", "Carol", "Dave"]);
        drop(db);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
