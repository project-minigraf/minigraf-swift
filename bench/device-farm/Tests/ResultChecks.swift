import Foundation

enum BenchmarkFailure: Error { case invalid(String) }

func requireResult(_ condition: Bool, _ message: String) throws {
    if !condition { throw BenchmarkFailure.invalid(message) }
}

enum ResultChecks {
    static func numbers(_ actual: [Int64], _ expected: [Int64]) throws {
        try requireResult(actual.count == expected.count, "wrong row count")
        try requireResult(actual.reduce(0, +) == expected.reduce(0, +), "wrong row sum")
        try requireResult(actual.sorted() == expected.sorted(), "wrong row values")
    }
}
