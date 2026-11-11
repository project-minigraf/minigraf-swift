// Write transactions (minigraf#824).

extension MiniGrafDb {
    /// Begins a write transaction with `options`; an option left `nil` is the
    /// database's. See `beginWrite()`. End it with `commit()` or `rollback()`,
    /// or use `withWriteTransaction`.
    public func beginWrite(options: TxOptions) throws -> MiniGrafWriteTransaction {
        try beginWriteWith(options: options)
    }

    /// Runs `body` in a write transaction and returns its result. The
    /// transaction rolls back when `body` throws or returns without calling
    /// `commit()`: a commit is always explicit.
    ///
    /// ```swift
    /// try db.withWriteTransaction { tx in
    ///     _ = try tx.execute(datalog: "(transact [[:alice :balance 90]])")
    ///     try tx.commit()
    /// }
    /// ```
    public func withWriteTransaction<T>(
        options: TxOptions = TxOptions(),
        _ body: (MiniGrafWriteTransaction) throws -> T
    ) throws -> T {
        let tx = try beginWriteWith(options: options)
        // Does nothing once committed.
        defer { try? tx.rollback() }
        return try body(tx)
    }
}
