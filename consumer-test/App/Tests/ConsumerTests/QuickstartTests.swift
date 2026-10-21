// The README quick start, line for line (readme-sync checks it).
import XCTest
import MinigrafKit

final class QuickstartTests: XCTestCase {
    func testQuickStart() throws {
        let db = try MiniGrafDb.openInMemory()
        let result = try db.execute(datalog: #"(transact [[:alice :name "Alice"]])"#)
        print(result)  // {"transacted":1}
        XCTAssertTrue(result.contains("\"transacted\""), "quick start transact returned \(result)")
    }
}
