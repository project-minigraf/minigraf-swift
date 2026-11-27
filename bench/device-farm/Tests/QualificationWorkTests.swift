import XCTest

// Pure controls: no MiniGraf, UIKit, simulator or physical device required.
final class QualificationWorkTests: XCTestCase {
    func testTwoFixturesAndExactTimingBoundary() throws {
        var events: [String] = []
        var next = 0
        var tick: UInt64 = 10
        let elapsed = try QualificationWork.invoke(setup: {
            next += 1; events.append("setup\(next)"); return next
        }, action: { fixture in
            events.append("action\(fixture)"); return [Int64(fixture)]
        }, validate: { value in
            events.append("validate\(value[0])")
        }, cleanup: { fixture in events.append("cleanup\(fixture)") }, clock: {
            events.append("clock"); defer { tick += 20 }; return tick
        })
        XCTAssertEqual(elapsed, 20) // No division by two: original denominator.
        XCTAssertEqual(events, ["setup1", "setup2", "clock", "action1", "action2", "clock",
                                "validate1", "validate2", "cleanup1", "cleanup2"])
    }

    func testEitherWrongResultFailsAndBothFixturesAreCleaned() {
        let wrongResults: [[Int64]] = [[], [0, 0, 3, 3]]
        for badIndex in [1, 2] {
            for wrong in wrongResults {
                var next = 0
                var checked = 0
                var cleaned: [Int] = []
                XCTAssertThrowsError(try QualificationWork.invoke(setup: {
                    next += 1; return next
                }, action: { fixture in
                    fixture == badIndex ? wrong : [0, 1, 2, 3]
                }, validate: { value in
                    checked += 1
                    try ResultChecks.numbers(value, [0, 1, 2, 3])
                }, cleanup: { cleaned.append($0) }, clock: { 0 }))
                XCTAssertEqual(checked, 2)
                XCTAssertEqual(cleaned, [1, 2])
            }
        }
    }

    func testSecondSetupFailureCleansFirstFixture() {
        var next = 0
        var cleaned: [Int] = []
        XCTAssertThrowsError(try QualificationWork.invoke(setup: { () throws -> Int in
            next += 1
            if next == 2 { throw BenchmarkFailure.invalid("setup") }
            return next
        }, action: { $0 }, validate: { _ in }, cleanup: { cleaned.append($0) }, clock: { 0 }))
        XCTAssertEqual(cleaned, [1])
    }

    func testActionAndCleanupFailuresCannotProduceValidSample() {
        for failAction in [true, false] {
            var next = 0
            var cleaned: [Int] = []
            XCTAssertThrowsError(try QualificationWork.invoke(setup: { next += 1; return next },
                action: { fixture in
                    if failAction && fixture == 2 { throw BenchmarkFailure.invalid("action") }
                    return fixture
                }, validate: { _ in }, cleanup: { fixture in
                    cleaned.append(fixture)
                    if fixture == 1 { throw BenchmarkFailure.invalid("cleanup") }
                }, clock: { 0 }))
            XCTAssertEqual(cleaned, [1, 2])
        }
    }
}
