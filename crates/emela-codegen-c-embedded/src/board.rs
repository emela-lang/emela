//! Names on the board boundary, derived from the declarations of the `Std.Embedded.*` modules.
//!
//! The module `Std.Embedded.Gpio` is the peripheral `Gpio`: its header is `emela_gpio.h`, its
//! effect operations are the C functions `emela_gpio_<op>`, its types are `EmelaGpio<Type>` (the
//! `Gpio` is not repeated: `GpioPin` is `EmelaGpioPin`), and the constructors of its enums are the
//! constants `EMELA_GPIO_<TYPE>_<CONSTRUCTOR>` (`EMELA_GPIO_PIN_STATE_LOW`).

/// The prefix of the modules whose effects the board provides.
const PREFIX: &str = "Std.Embedded.";

/// A peripheral module of the board boundary.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub(crate) struct Peripheral {
    /// `Std.Embedded.Gpio`.
    pub path: String,
    /// `Gpio`.
    pub short: String,
}

impl Peripheral {
    /// `emela_gpio.h`.
    pub fn header(&self) -> String {
        format!("emela_{}.h", snake(&self.short))
    }

    /// `emela_gpio_write`.
    pub fn op_function(&self, op: &str) -> String {
        format!("emela_{}_{}", snake(&self.short), op)
    }

    /// `EmelaGpioPinState` for `PinState`, `EmelaGpioPin` for `GpioPin`.
    pub fn type_name(&self, item: &str) -> String {
        format!("Emela{}", self.qualified(item))
    }

    /// `EMELA_GPIO_PIN_STATE_LOW` for `PinState.Low`.
    pub fn constant(&self, item: &str, ctor: &str) -> String {
        format!(
            "EMELA_{}_{}",
            snake(&self.qualified(item)).to_uppercase(),
            snake(ctor).to_uppercase()
        )
    }

    fn qualified(&self, item: &str) -> String {
        if item.starts_with(&self.short) {
            item.to_owned()
        } else {
            format!("{}{item}", self.short)
        }
    }
}

/// The peripheral and the item name of a definition named with its module path
/// (`Std.Embedded.Gpio.PinState`), if it is on the board boundary.
pub(crate) fn split(name: &str) -> Option<(Peripheral, &str)> {
    let rest = name.strip_prefix(PREFIX)?;
    let (short, item) = rest.rsplit_once('.')?;
    if short.contains('.') {
        return None;
    }
    Some((
        Peripheral {
            path: format!("{PREFIX}{short}"),
            short: short.to_owned(),
        },
        item,
    ))
}

/// The peripheral of an operation of a board effect, named `Std.Embedded.Gpio.Gpio.write`.
pub(crate) fn split_op(name: &str) -> Option<Peripheral> {
    let (effect, _) = name.rsplit_once('.')?;
    split(effect).map(|(p, _)| p)
}

/// `pin_state` for `PinState`, `gpio` for `Gpio`.
fn snake(name: &str) -> String {
    let mut out = String::new();
    for (i, c) in name.chars().enumerate() {
        if c.is_ascii_uppercase() {
            if i > 0 {
                out.push('_');
            }
            out.push(c.to_ascii_lowercase());
        } else {
            out.push(c);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names() {
        let (p, item) = split("Std.Embedded.Gpio.PinState").unwrap();
        assert_eq!(item, "PinState");
        assert_eq!(p.header(), "emela_gpio.h");
        assert_eq!(
            p.op_function("enable_interrupt"),
            "emela_gpio_enable_interrupt"
        );
        assert_eq!(p.type_name("PinState"), "EmelaGpioPinState");
        assert_eq!(p.type_name("GpioPin"), "EmelaGpioPin");
        assert_eq!(p.constant("PinState", "Low"), "EMELA_GPIO_PIN_STATE_LOW");
        assert_eq!(p.constant("Pull", "NoPull"), "EMELA_GPIO_PULL_NO_PULL");
        assert_eq!(
            p.constant("GpioMode", "OutputPushPull"),
            "EMELA_GPIO_MODE_OUTPUT_PUSH_PULL"
        );
        let (p, item) = split("Std.Embedded.Gpio.Gpio").unwrap();
        assert_eq!((p.short.as_str(), item), ("Gpio", "Gpio"));
        assert!(split("Std.Option.Option").is_none());
        assert!(split("Main.GpioPin").is_none());
    }
}
