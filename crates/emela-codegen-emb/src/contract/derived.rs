/// Structural operations over the materialized layouts of a concrete Flow type.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DerivedOperation {
    /// Equality; function values are not comparable.
    Eq,
    /// Escaped structural display; function values are displayed as `<fn>`.
    Show,
}
