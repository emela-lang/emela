//! A board definition (`docs/design/board-definition.md`): the board's logical pins and the MCU
//! pins they are, read by the driver from the TOML file the project selects
//! (`[build] board` in `Pome.toml`), and the GPIO interrupts the project binds to its interrupt
//! entries (`[embedded.gpio_interrupts]`).

/// A board, as far as the compiler uses it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BoardDef {
    /// `stm32f4-nucleo-f446re`.
    pub id: String,
    /// `NUCLEO-F446RE`.
    pub name: String,
    /// The HAL the generated adapter calls (`STM32CubeF4`); `None` generates no adapter.
    pub hal: Option<String>,
    /// The logical pins, by `GpioPin` ID.
    pub pins: Vec<LogicalPin>,
    /// The peripherals' routes (`[peripheral_routes.uart0]`, `.i2c1`, `.spi1`), by kind.
    pub uarts: Vec<Route>,
    pub i2cs: Vec<Route>,
    pub spis: Vec<Route>,
}

/// A peripheral route: the logical peripheral (`Uart0`, the constructor of `UartId`) is MCU
/// instance `instance` (`USART2`) on logical pins by role (`tx`, `rx`; `scl`, `sda`; `sck`,
/// `miso`, `mosi`), with alternate function `af`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Route {
    /// `Uart0`.
    pub id: String,
    pub instance: String,
    pub pins: Vec<(String, i32)>,
    pub af: u8,
}

impl Route {
    pub fn pin(&self, role: &str) -> Option<i32> {
        self.pins.iter().find(|(r, _)| r == role).map(|(_, p)| *p)
    }
}

/// The pin roles of a kind of route.
pub fn route_roles(kind: &str) -> &'static [&'static str] {
    match kind {
        "uart" => &["tx", "rx"],
        "i2c" => &["scl", "sda"],
        "spi" => &["sck", "miso", "mosi"],
        _ => &[],
    }
}

/// A logical pin: `GpioPin(id)` is MCU pin `port``pin` (`PA5`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LogicalPin {
    pub id: i32,
    /// The label on the board (`D13`).
    pub label: String,
    /// `'A'` for `PA5`.
    pub port: char,
    /// `5` for `PA5`.
    pub pin: u8,
}

impl BoardDef {
    pub fn pin(&self, id: i32) -> Option<&LogicalPin> {
        self.pins.iter().find(|p| p.id == id)
    }
}

impl LogicalPin {
    /// `PA5`.
    pub fn mcu_name(&self) -> String {
        format!("P{}{}", self.port, self.pin)
    }
}

/// A GPIO interrupt bound to an interrupt entry: the edges enabled by `enable_interrupt` on
/// `pin` call `handler` (a public function taking a `GpioEvent`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GpioInterrupt {
    pub pin: i32,
    pub handler: String,
}
