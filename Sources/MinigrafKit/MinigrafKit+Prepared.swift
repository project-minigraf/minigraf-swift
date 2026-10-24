// Plain Swift values for prepared queries (minigraf#181).
import Foundation

/// A value that binds a `$slot` of a prepared query: `Bool`, `Int`, `Int32`,
/// `Int64`, `Double`, `Float`, `String`, `UUID` (an entity, which also fits
/// value and rule-argument slots), a `Set` of `MinigrafSetElement`s, a
/// `MiniGrafValue` or a `MiniGrafBindValue`.
public protocol MinigrafBindable {
    var minigrafBindValue: MiniGrafBindValue { get }
}

extension Bool: MinigrafBindable {
    public var minigrafBindValue: MiniGrafBindValue { .value(value: .bool(value: self)) }
}

extension Int: MinigrafBindable {
    public var minigrafBindValue: MiniGrafBindValue { .value(value: .int64(value: Int64(self))) }
}

extension Int32: MinigrafBindable {
    public var minigrafBindValue: MiniGrafBindValue { .value(value: .int64(value: Int64(self))) }
}

extension Int64: MinigrafBindable {
    public var minigrafBindValue: MiniGrafBindValue { .value(value: .int64(value: self)) }
}

extension Double: MinigrafBindable {
    public var minigrafBindValue: MiniGrafBindValue { .value(value: .float64(value: self)) }
}

extension Float: MinigrafBindable {
    public var minigrafBindValue: MiniGrafBindValue { .value(value: .float64(value: Double(self))) }
}

extension String: MinigrafBindable {
    public var minigrafBindValue: MiniGrafBindValue { .value(value: .text(value: self)) }
}

extension UUID: MinigrafBindable {
    public var minigrafBindValue: MiniGrafBindValue { .entity(uuid: uuidString) }
}

extension MiniGrafValue: MinigrafBindable {
    public var minigrafBindValue: MiniGrafBindValue { .value(value: self) }
}

extension MiniGrafBindValue: MinigrafBindable {
    public var minigrafBindValue: MiniGrafBindValue { self }
}

/// A value that can be an element of a bound `Set` (minigraf#316): `Bool`,
/// `Int`, `Int32`, `Int64`, `Double`, `Float`, `String`, `UUID` (a ref) or a
/// `MiniGrafValue`. A `MiniGrafValue.set` element is a set inside a set, which
/// throws `MiniGrafError.Api` with `[API-021]`.
public protocol MinigrafSetElement: Hashable {
    var minigrafValue: MiniGrafValue { get }
}

extension Bool: MinigrafSetElement {
    public var minigrafValue: MiniGrafValue { .bool(value: self) }
}

extension Int: MinigrafSetElement {
    public var minigrafValue: MiniGrafValue { .int64(value: Int64(self)) }
}

extension Int32: MinigrafSetElement {
    public var minigrafValue: MiniGrafValue { .int64(value: Int64(self)) }
}

extension Int64: MinigrafSetElement {
    public var minigrafValue: MiniGrafValue { .int64(value: self) }
}

extension Double: MinigrafSetElement {
    public var minigrafValue: MiniGrafValue { .float64(value: self) }
}

extension Float: MinigrafSetElement {
    public var minigrafValue: MiniGrafValue { .float64(value: Double(self)) }
}

extension String: MinigrafSetElement {
    public var minigrafValue: MiniGrafValue { .text(value: self) }
}

extension UUID: MinigrafSetElement {
    public var minigrafValue: MiniGrafValue { .ref(value: uuidString) }
}

extension MiniGrafValue: MinigrafSetElement {
    public var minigrafValue: MiniGrafValue { self }
}

/// A `Set` binds a set value; mix element types with `Set<MiniGrafValue>`.
extension Set: MinigrafBindable where Element: MinigrafSetElement {
    public var minigrafBindValue: MiniGrafBindValue { .value(value: .set(value: map { $0.minigrafValue })) }
}

private func bindValues(_ bindings: [String: (any MinigrafBindable)?]) -> [String: MiniGrafBindValue] {
    bindings.mapValues { $0?.minigrafBindValue ?? .value(value: .null) }
}

extension MiniGrafPreparedQuery {
    /// Runs the query with `bindings` (slot names without the `$`; `nil` is
    /// `Null`) and returns its result as JSON, encoded like `execute(datalog:)`.
    public func execute(_ bindings: [String: (any MinigrafBindable)?]) throws -> String {
        try execute(bindings: bindValues(bindings))
    }

    /// Runs the query with `bindings` and returns a cursor over its rows.
    public func query(_ bindings: [String: (any MinigrafBindable)?]) throws -> MiniGrafCursor {
        try query(bindings: bindValues(bindings))
    }
}
