use std::collections::BTreeMap;

use crate::ast::{Capability, PrimType, Type};
use crate::error::{Error, Result};

#[derive(Debug, Clone, Default)]
pub(crate) struct ExternalBindings {
    pub(crate) js_callee: Option<String>,
    pub(crate) native: Option<NativeBinding>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct NativeBinding {
    pub(crate) symbol: String,
    pub(crate) links: Vec<String>,
}

#[derive(Debug, Clone)]
pub(crate) struct ExternalFunction {
    pub(crate) path: Vec<String>,
    pub(crate) name: String,
    pub(crate) params: Vec<Type>,
    pub(crate) ret: Type,
    pub(crate) effectful: bool,
    pub(crate) capabilities: Vec<Capability>,
    pub(crate) bindings: ExternalBindings,
}

#[derive(Debug, Clone, Default)]
pub(crate) struct ExternalRegistry {
    functions: Vec<ExternalFunction>,
}

impl ExternalRegistry {
    pub(crate) fn builtin_native() -> Self {
        Self {
            functions: vec![
                ExternalFunction {
                    path: vec!["platform".to_string(), "io".to_string()],
                    name: "print_i32!".to_string(),
                    params: vec![Type::Prim(PrimType::I32)],
                    ret: Type::Prim(PrimType::Unit),
                    effectful: true,
                    capabilities: vec![Capability::Stdout],
                    bindings: ExternalBindings {
                        native: Some(NativeBinding {
                            symbol: "emela_print_i32".to_string(),
                            links: vec!["emela_runtime".to_string()],
                        }),
                        ..ExternalBindings::default()
                    },
                },
                ExternalFunction {
                    path: vec!["platform".to_string(), "io".to_string()],
                    name: "_print_i32!".to_string(),
                    params: vec![Type::Prim(PrimType::I32)],
                    ret: Type::Prim(PrimType::Unit),
                    effectful: true,
                    capabilities: vec![Capability::Stdout],
                    bindings: ExternalBindings {
                        native: Some(NativeBinding {
                            symbol: "emela_print_i32".to_string(),
                            links: vec!["emela_runtime".to_string()],
                        }),
                        ..ExternalBindings::default()
                    },
                },
                ExternalFunction {
                    path: vec!["platform".to_string(), "io".to_string()],
                    name: "print_bool!".to_string(),
                    params: vec![Type::Prim(PrimType::Bool)],
                    ret: Type::Prim(PrimType::Unit),
                    effectful: true,
                    capabilities: vec![Capability::Stdout],
                    bindings: ExternalBindings {
                        native: Some(NativeBinding {
                            symbol: "emela_print_bool".to_string(),
                            links: vec!["emela_runtime".to_string()],
                        }),
                        ..ExternalBindings::default()
                    },
                },
                ExternalFunction {
                    path: vec!["platform".to_string(), "io".to_string()],
                    name: "_print_bool!".to_string(),
                    params: vec![Type::Prim(PrimType::Bool)],
                    ret: Type::Prim(PrimType::Unit),
                    effectful: true,
                    capabilities: vec![Capability::Stdout],
                    bindings: ExternalBindings {
                        native: Some(NativeBinding {
                            symbol: "emela_print_bool".to_string(),
                            links: vec!["emela_runtime".to_string()],
                        }),
                        ..ExternalBindings::default()
                    },
                },
                ExternalFunction {
                    path: vec!["platform".to_string(), "clock".to_string()],
                    name: "now_i32!".to_string(),
                    params: Vec::new(),
                    ret: Type::Prim(PrimType::I32),
                    effectful: true,
                    capabilities: vec![Capability::Clock],
                    bindings: ExternalBindings {
                        native: Some(NativeBinding {
                            symbol: "emela_now_i32".to_string(),
                            links: vec!["emela_runtime".to_string()],
                        }),
                        ..ExternalBindings::default()
                    },
                },
                ExternalFunction {
                    path: vec!["platform".to_string(), "clock".to_string()],
                    name: "_now_i32!".to_string(),
                    params: Vec::new(),
                    ret: Type::Prim(PrimType::I32),
                    effectful: true,
                    capabilities: vec![Capability::Clock],
                    bindings: ExternalBindings {
                        native: Some(NativeBinding {
                            symbol: "emela_now_i32".to_string(),
                            links: vec!["emela_runtime".to_string()],
                        }),
                        ..ExternalBindings::default()
                    },
                },
            ],
        }
    }

    pub(crate) fn from_manifest_json(source: &str) -> Result<(String, Vec<Capability>, Self)> {
        let root = JsonParser::new(source).parse()?;
        let object = root.as_object("manifest root")?;
        let name = object
            .get("name")
            .ok_or_else(|| Error::new("platform manifest missing `name`"))?
            .as_string("name")?
            .to_string();
        let capabilities = object
            .get("capabilities")
            .ok_or_else(|| Error::new("platform manifest missing `capabilities`"))?
            .as_array("capabilities")?
            .iter()
            .map(|value| {
                let name = value.as_string("capability")?;
                Capability::parse(name)
                    .ok_or_else(|| Error::new(format!("unknown capability `{name}`")))
            })
            .collect::<Result<Vec<_>>>()?;
        let mut functions = Vec::new();
        let externs = object
            .get("externs")
            .ok_or_else(|| Error::new("platform manifest missing `externs`"))?
            .as_array("externs")?;
        for value in externs {
            functions.push(parse_external_function(value)?);
        }

        let registry = Self { functions };
        registry.check_duplicates()?;
        Ok((name, capabilities, registry))
    }

    pub(crate) fn resolve_import(&self, path: &[String], name: &str) -> Option<&ExternalFunction> {
        self.functions.iter().find(|function| {
            function.name == name
                && function.path.len() == path.len()
                && function
                    .path
                    .iter()
                    .zip(path.iter())
                    .all(|(left, right)| left == right)
        })
    }

    #[cfg(test)]
    pub(crate) fn native_links(&self) -> Vec<&str> {
        let mut links = Vec::new();
        for function in &self.functions {
            let Some(binding) = &function.bindings.native else {
                continue;
            };
            for link in &binding.links {
                if !links.contains(&link.as_str()) {
                    links.push(link.as_str());
                }
            }
        }
        links
    }

    fn check_duplicates(&self) -> Result<()> {
        let mut seen = Vec::<String>::new();
        for function in &self.functions {
            let key = format_external_path(function);
            if seen.contains(&key) {
                return Err(Error::new(format!("duplicate external import `{key}`")));
            }
            seen.push(key);
        }
        Ok(())
    }
}

pub(crate) fn format_external_path(function: &ExternalFunction) -> String {
    let mut parts = function.path.clone();
    parts.push(function.name.clone());
    parts.join(".")
}

fn parse_external_function(value: &JsonValue) -> Result<ExternalFunction> {
    let object = value.as_object("extern")?;
    let path = object
        .get("path")
        .ok_or_else(|| Error::new("external function missing `path`"))?
        .as_array("extern.path")?
        .iter()
        .map(|value| value.as_string("extern.path item").map(str::to_string))
        .collect::<Result<Vec<_>>>()?;
    if path.is_empty() {
        return Err(Error::new("external function path must not be empty"));
    }
    let name = object
        .get("name")
        .ok_or_else(|| Error::new("external function missing `name`"))?
        .as_string("extern.name")?
        .to_string();
    let params = object
        .get("params")
        .ok_or_else(|| Error::new("external function missing `params`"))?
        .as_array("extern.params")?
        .iter()
        .map(parse_type)
        .collect::<Result<Vec<_>>>()?;
    let ret = parse_type(
        object
            .get("return")
            .ok_or_else(|| Error::new("external function missing `return`"))?,
    )?;
    let effectful = object
        .get("effectful")
        .ok_or_else(|| Error::new("external function missing `effectful`"))?
        .as_bool("extern.effectful")?;
    let capabilities = object
        .get("capabilities")
        .ok_or_else(|| Error::new("external function missing `capabilities`"))?
        .as_array("extern.capabilities")?
        .iter()
        .map(|value| {
            let name = value.as_string("extern.capabilities item")?;
            Capability::parse(name)
                .ok_or_else(|| Error::new(format!("unknown capability `{name}`")))
        })
        .collect::<Result<Vec<_>>>()?;
    let bindings = parse_bindings(object.get("bindings"))?;
    Ok(ExternalFunction {
        path,
        name,
        params,
        ret,
        effectful,
        capabilities,
        bindings,
    })
}

fn parse_type(value: &JsonValue) -> Result<Type> {
    let name = value.as_string("type")?;
    match name {
        "I32" | "i32" => Ok(Type::Prim(PrimType::I32)),
        "Bool" | "bool" => Ok(Type::Prim(PrimType::Bool)),
        "Unit" | "unit" => Ok(Type::Prim(PrimType::Unit)),
        _ => Err(Error::new(format!(
            "unknown external manifest type `{name}`"
        ))),
    }
}

fn parse_bindings(value: Option<&JsonValue>) -> Result<ExternalBindings> {
    let Some(value) = value else {
        return Ok(ExternalBindings::default());
    };
    let object = value.as_object("extern.bindings")?;
    let js_callee = match object.get("js") {
        Some(js) => Some(
            js.as_object("extern.bindings.js")?
                .get("callee")
                .ok_or_else(|| Error::new("js binding missing `callee`"))?
                .as_string("extern.bindings.js.callee")?
                .to_string(),
        ),
        None => None,
    };
    let native = object.get("native").map(parse_native_binding).transpose()?;
    Ok(ExternalBindings { js_callee, native })
}

fn parse_native_binding(value: &JsonValue) -> Result<NativeBinding> {
    let object = value.as_object("extern.bindings.native")?;
    let symbol = object
        .get("symbol")
        .ok_or_else(|| Error::new("native binding missing `symbol`"))?
        .as_string("extern.bindings.native.symbol")?
        .to_string();
    let links = match object.get("link") {
        Some(value) => value
            .as_array("extern.bindings.native.link")?
            .iter()
            .map(|value| {
                value
                    .as_string("extern.bindings.native.link item")
                    .map(str::to_string)
            })
            .collect::<Result<Vec<_>>>()?,
        None => Vec::new(),
    };
    for key in object.keys() {
        if key != "symbol" && key != "link" {
            return Err(Error::new(format!("unknown native binding field `{key}`")));
        }
    }
    Ok(NativeBinding { symbol, links })
}

#[derive(Debug, Clone)]
enum JsonValue {
    Null,
    Bool(bool),
    Number,
    String(String),
    Array(Vec<JsonValue>),
    Object(BTreeMap<String, JsonValue>),
}

impl JsonValue {
    fn as_object(&self, context: &str) -> Result<&BTreeMap<String, JsonValue>> {
        match self {
            JsonValue::Object(object) => Ok(object),
            _ => Err(Error::new(format!("{context} must be an object"))),
        }
    }

    fn as_array(&self, context: &str) -> Result<&[JsonValue]> {
        match self {
            JsonValue::Array(values) => Ok(values),
            _ => Err(Error::new(format!("{context} must be an array"))),
        }
    }

    fn as_string(&self, context: &str) -> Result<&str> {
        match self {
            JsonValue::String(value) => Ok(value),
            _ => Err(Error::new(format!("{context} must be a string"))),
        }
    }

    fn as_bool(&self, context: &str) -> Result<bool> {
        match self {
            JsonValue::Bool(value) => Ok(*value),
            _ => Err(Error::new(format!("{context} must be a boolean"))),
        }
    }
}

struct JsonParser {
    chars: Vec<char>,
    pos: usize,
}

impl JsonParser {
    fn new(source: &str) -> Self {
        Self {
            chars: source.chars().collect(),
            pos: 0,
        }
    }

    fn parse(mut self) -> Result<JsonValue> {
        let value = self.parse_value()?;
        self.skip_ws();
        if self.pos != self.chars.len() {
            return Err(Error::new(format!(
                "unexpected trailing JSON at character {}",
                self.pos
            )));
        }
        Ok(value)
    }

    fn parse_value(&mut self) -> Result<JsonValue> {
        self.skip_ws();
        match self.peek() {
            Some('{') => self.parse_object(),
            Some('[') => self.parse_array(),
            Some('"') => self.parse_string().map(JsonValue::String),
            Some('t') => {
                self.expect_literal("true")?;
                Ok(JsonValue::Bool(true))
            }
            Some('f') => {
                self.expect_literal("false")?;
                Ok(JsonValue::Bool(false))
            }
            Some('n') => {
                self.expect_literal("null")?;
                Ok(JsonValue::Null)
            }
            Some('-' | '0'..='9') => self.parse_number(),
            Some(ch) => Err(Error::new(format!(
                "unexpected JSON character `{ch}` at character {}",
                self.pos
            ))),
            None => Err(Error::new("unexpected end of JSON")),
        }
    }

    fn parse_object(&mut self) -> Result<JsonValue> {
        self.expect_char('{')?;
        let mut object = BTreeMap::new();
        self.skip_ws();
        if self.eat('}') {
            return Ok(JsonValue::Object(object));
        }
        loop {
            self.skip_ws();
            let key = self.parse_string()?;
            self.skip_ws();
            self.expect_char(':')?;
            let value = self.parse_value()?;
            if object.insert(key.clone(), value).is_some() {
                return Err(Error::new(format!("duplicate JSON object key `{key}`")));
            }
            self.skip_ws();
            if self.eat('}') {
                break;
            }
            self.expect_char(',')?;
        }
        Ok(JsonValue::Object(object))
    }

    fn parse_array(&mut self) -> Result<JsonValue> {
        self.expect_char('[')?;
        let mut values = Vec::new();
        self.skip_ws();
        if self.eat(']') {
            return Ok(JsonValue::Array(values));
        }
        loop {
            values.push(self.parse_value()?);
            self.skip_ws();
            if self.eat(']') {
                break;
            }
            self.expect_char(',')?;
        }
        Ok(JsonValue::Array(values))
    }

    fn parse_string(&mut self) -> Result<String> {
        self.expect_char('"')?;
        let mut out = String::new();
        while let Some(ch) = self.bump() {
            match ch {
                '"' => return Ok(out),
                '\\' => {
                    let escaped = self
                        .bump()
                        .ok_or_else(|| Error::new("unterminated JSON escape"))?;
                    match escaped {
                        '"' | '\\' | '/' => out.push(escaped),
                        'b' => out.push('\u{0008}'),
                        'f' => out.push('\u{000c}'),
                        'n' => out.push('\n'),
                        'r' => out.push('\r'),
                        't' => out.push('\t'),
                        'u' => {
                            return Err(Error::new(
                                "unicode JSON escapes are not supported in platform manifests",
                            ));
                        }
                        _ => {
                            return Err(Error::new(format!(
                                "unsupported JSON escape `\\{escaped}`"
                            )));
                        }
                    }
                }
                _ => out.push(ch),
            }
        }
        Err(Error::new("unterminated JSON string"))
    }

    fn parse_number(&mut self) -> Result<JsonValue> {
        if self.eat('-') {
            // sign consumed
        }
        self.consume_digits();
        if self.eat('.') {
            self.consume_digits();
        }
        if matches!(self.peek(), Some('e' | 'E')) {
            self.bump();
            if matches!(self.peek(), Some('+' | '-')) {
                self.bump();
            }
            self.consume_digits();
        }
        Ok(JsonValue::Number)
    }

    fn consume_digits(&mut self) {
        while matches!(self.peek(), Some('0'..='9')) {
            self.bump();
        }
    }

    fn expect_literal(&mut self, literal: &str) -> Result<()> {
        for expected in literal.chars() {
            self.expect_char(expected)?;
        }
        Ok(())
    }

    fn skip_ws(&mut self) {
        while matches!(self.peek(), Some(' ' | '\n' | '\r' | '\t')) {
            self.bump();
        }
    }

    fn expect_char(&mut self, expected: char) -> Result<()> {
        match self.bump() {
            Some(ch) if ch == expected => Ok(()),
            Some(ch) => Err(Error::new(format!(
                "expected JSON character `{expected}`, got `{ch}` at character {}",
                self.pos.saturating_sub(1)
            ))),
            None => Err(Error::new(format!(
                "expected JSON character `{expected}` at end of input"
            ))),
        }
    }

    fn eat(&mut self, expected: char) -> bool {
        if self.peek() == Some(expected) {
            self.bump();
            true
        } else {
            false
        }
    }

    fn peek(&self) -> Option<char> {
        self.chars.get(self.pos).copied()
    }

    fn bump(&mut self) -> Option<char> {
        let ch = self.peek()?;
        self.pos += 1;
        Some(ch)
    }
}
