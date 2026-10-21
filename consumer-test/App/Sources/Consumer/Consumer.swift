// An app module that depends on MinigrafKit, as a user's would; the checks are
// in Tests/ConsumerTests.
import MinigrafKit

public func openDatabase(path: String) throws -> MiniGrafDb {
    try MiniGrafDb.open(path: path)
}
