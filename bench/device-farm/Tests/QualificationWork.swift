import Foundation

// Used only by testQualificationSlowdown. Keep the #839 invoke path intact.
// Both independent fixtures and both retained results outlive the timed region.
enum QualificationWork {
    static func invoke<S, T>(setup: () throws -> S, action: (S) throws -> T,
                             validate: (T) throws -> Void, cleanup: (S) throws -> Void,
                             clock: () -> UInt64) throws -> UInt64 {
        let first = try setup()
        let second: S
        do { second = try setup() } catch {
            try? cleanup(first)
            throw error
        }
        var failure: Error?
        var elapsed: UInt64 = 0
        do {
            let start = clock()
            let firstValue = try action(first)
            let secondValue = try action(second)
            elapsed = clock() - start
            // Do not stop checking when the first result is wrong.
            do { try validate(firstValue) } catch { failure = error }
            do { try validate(secondValue) } catch { if failure == nil { failure = error } }
        } catch { failure = error }
        do { try cleanup(first) } catch { if failure == nil { failure = error } }
        do { try cleanup(second) } catch { if failure == nil { failure = error } }
        if let failure { throw failure }
        return elapsed
    }
}
