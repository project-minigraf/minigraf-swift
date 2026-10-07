// Iteration helpers over the generated bindings. A Swift `Sequence` cannot
// throw, and a batch read can fail, so these take a throwing closure instead.

extension MiniGrafCursor {
    /// Calls `body` with each batch of at most `size` rows, as a JSON array of
    /// rows encoded like `execute`'s `results`, until the cursor ends.
    public func forEachBatch(size: Int32 = 1000, _ body: (String) throws -> Void) throws {
        while let batch = try nextBatch(maxRows: size) {
            try body(batch)
        }
    }
}

extension MiniGrafFactLog {
    /// Calls `body` with each record, reading `batchSize` records at a time,
    /// until the log ends.
    public func forEachRecord(batchSize: Int32 = 1000, _ body: (FactRecord) throws -> Void) throws {
        while let batch = try nextBatch(maxRecords: batchSize) {
            for record in batch {
                try body(record)
            }
        }
    }
}

extension MiniGrafError {
    /// The error's text, starting with its code, such as `[API-015]`.
    public var message: String {
        switch self {
        case .Storage(let msg), .Query(let msg), .Parse(let msg), .Other(let msg):
            return msg
        }
    }
}

/// `validTo` of a fact that is valid forever.
public let validTimeForever: Int64 = Int64.max

/// `walCheckpointThreshold` that never checkpoints: no automatic checkpoint and
/// none when the handle closes. Call `checkpoint()` yourself.
public let walCheckpointNever: Int64 = Int64.max
