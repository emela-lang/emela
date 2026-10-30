//! Embedded interfaces and Board handlers owned by this backend.

// Preserve the original module order when frontends append these to core/std sources.
const SOURCES: &[(&str, &str)] = &[
    (
        "Std.Embedded.Peripheral",
        include_str!("../lib/embedded/peripheral.emel"),
    ),
    (
        "Std.Embedded.Gpio",
        include_str!("../lib/embedded/gpio.emel"),
    ),
    (
        "Std.Embedded.Uart",
        include_str!("../lib/embedded/uart.emel"),
    ),
    ("Std.Embedded.I2c", include_str!("../lib/embedded/i2c.emel")),
    ("Std.Embedded.Spi", include_str!("../lib/embedded/spi.emel")),
    (
        "Std.Embedded.Board",
        include_str!("../lib/embedded/board.emel"),
    ),
];

/// Embedded modules as (module name, source) pairs, loaded before name resolution.
///
/// Frontends append these to `emela_core::core_sources()` before resolving imports,
/// type checking or lowering. The CLI and driver do this automatically. The files
/// are compiled into this crate; no source checkout or filesystem lookup is needed.
pub fn bundled_sources() -> &'static [(&'static str, &'static str)] {
    SOURCES
}
