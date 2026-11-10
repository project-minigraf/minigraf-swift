// The override list (minigraf#872).

extension Overridable {
    /// The override list that applies when `OpenOptions.allowOverrides` is `nil`:
    /// the Rust `Overridable::DEFAULTS`, read from the core library. A list passed
    /// as `allowOverrides` replaces it; an empty one refuses every override keyword
    /// (API-025).
    public static var defaults: [Overridable] { overridableDefaults() }
}
