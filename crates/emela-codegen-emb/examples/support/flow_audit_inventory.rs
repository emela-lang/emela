use std::collections::BTreeSet;
use std::fs;
use std::path::Path;

fn tokens(source: &str) -> Vec<String> {
    let bytes = source.as_bytes();
    let mut out = Vec::new();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i].is_ascii_whitespace() {
            i += 1;
        } else if bytes[i..].starts_with(b"//") {
            while i < bytes.len() && bytes[i] != b'\n' {
                i += 1;
            }
        } else if bytes[i..].starts_with(b"/*") {
            let mut depth = 1;
            i += 2;
            while i < bytes.len() && depth != 0 {
                if bytes[i..].starts_with(b"/*") {
                    depth += 1;
                    i += 2;
                } else if bytes[i..].starts_with(b"*/") {
                    depth -= 1;
                    i += 2;
                } else {
                    i += 1;
                }
            }
        } else if bytes[i] == b'"' {
            i += 1;
            while i < bytes.len() {
                if bytes[i] == b'\\' {
                    i += 2;
                } else if bytes[i] == b'"' {
                    i += 1;
                    break;
                } else {
                    i += 1;
                }
            }
        } else if bytes[i].is_ascii_alphabetic() || bytes[i] == b'_' {
            let start = i;
            i += 1;
            while i < bytes.len() && (bytes[i].is_ascii_alphanumeric() || bytes[i] == b'_') {
                i += 1;
            }
            out.push(source[start..i].to_owned());
        } else {
            out.push(char::from(bytes[i]).to_string());
            i += 1;
        }
    }
    out
}

pub fn enum_variants(source: &str, name: &str) -> Result<BTreeSet<String>, String> {
    let tokens = tokens(source);
    let start = tokens
        .windows(3)
        .position(|w| w[0] == "enum" && w[1] == name && w[2] == "{")
        .ok_or_else(|| format!("enum {name} is missing"))?
        + 3;
    let (mut curly, mut parens, mut brackets) = (1, 0, 0);
    let mut expected = true;
    let mut found = BTreeSet::new();
    for token in &tokens[start..] {
        match token.as_str() {
            "{" => curly += 1,
            "}" => {
                curly -= 1;
                if curly == 0 {
                    return Ok(found);
                }
            }
            "(" => parens += 1,
            ")" => parens -= 1,
            "[" => brackets += 1,
            "]" => brackets -= 1,
            "," if curly == 1 && parens == 0 && brackets == 0 => expected = true,
            _ if expected
                && curly == 1
                && parens == 0
                && brackets == 0
                && token.as_bytes()[0].is_ascii_alphabetic() =>
            {
                found.insert(token.clone());
                expected = false;
            }
            _ => {}
        }
    }
    Err(format!("enum {name} has no closing brace"))
}

pub fn check_inventory(
    owner: &str,
    expected: &BTreeSet<String>,
    inventory: &serde_json::Value,
) -> Result<(), String> {
    let items = inventory["items"]
        .as_array()
        .ok_or("missing inventory items")?;
    let prefix = format!("{owner}::");
    let mut listed = BTreeSet::new();
    for item in items
        .iter()
        .filter(|item| item["kind"] == "variant" && item["parent"] == owner)
    {
        let id = item["id"].as_str().ok_or("missing variant ID")?;
        let variant = id
            .strip_prefix(&prefix)
            .filter(|name| !name.is_empty() && !name.contains("::"))
            .ok_or_else(|| format!("{owner}: invalid variant ID {id}"))?;
        if !listed.insert(variant.to_owned()) {
            return Err(format!("{owner}: duplicated inventory entry {id}"));
        }
    }
    if &listed != expected {
        return Err(format!(
            "{owner}: missing {:?}; extra {:?}",
            expected.difference(&listed).collect::<Vec<_>>(),
            listed.difference(expected).collect::<Vec<_>>()
        ));
    }
    Ok(())
}

pub fn check(crate_root: &Path) -> Result<Vec<(String, usize)>, String> {
    let inventory_path = "docs/generated/flow-ir-inventory.json";
    let inventory: serde_json::Value = serde_json::from_str(
        &fs::read_to_string(crate_root.join(inventory_path))
            .map_err(|e| format!("{inventory_path}: {e}"))?,
    )
    .map_err(|e| format!("{inventory_path}: {e}"))?;
    let definitions = [
        ("Value", "../emela-lir/src/ir.rs"),
        ("Const", "../emela-lir/src/ir.rs"),
        ("PrimOp", "../emela-lir/src/ir.rs"),
        ("PrimTy", "../emela-lir/src/ir.rs"),
        ("Callee", "../emela-lir/src/ir.rs"),
        ("Operand", "../emela-lir/src/ir.rs"),
        ("Stmt", "../emela-lir/src/ir.rs"),
        ("Term", "../emela-lir/src/ir.rs"),
        ("OnFail", "../emela-lir/src/ir.rs"),
        ("SwitchOn", "../emela-lir/src/ir.rs"),
        ("CaseKey", "../emela-lir/src/ir.rs"),
        ("Stage", "../emela-lir/src/ir.rs"),
        ("FiberOp", "../emela-core/src/ir.rs"),
        ("RuntimeOp", "../emela-lir/src/ir.rs"),
        ("Builtin", "../emela-core/src/intrinsics.rs"),
    ];
    let mut counts = Vec::new();
    for (name, source) in definitions {
        let source =
            fs::read_to_string(crate_root.join(source)).map_err(|e| format!("{source}: {e}"))?;
        let expected = enum_variants(&source, name)?;
        let namespace = if matches!(name, "FiberOp" | "Builtin") {
            "emela_core"
        } else {
            "emela_lir"
        };
        check_inventory(&format!("{namespace}::{name}"), &expected, &inventory)?;
        if name == "Builtin" {
            // Lowering-only cell/display builtins are absent from the public ALL table.
            let public: BTreeSet<_> = emela_core::Builtin::ALL
                .iter()
                .map(|op| format!("{op:?}"))
                .collect();
            if public.len() != emela_core::Builtin::ALL.len() {
                return Err("Builtin::ALL has duplicate entries".into());
            }
            let internal = [
                "CellNew",
                "CellGet",
                "CellSet",
                "ShowString",
                "DebugPrint",
                "AssertFail",
            ]
            .map(str::to_owned)
            .into_iter()
            .collect();
            if public.union(&internal).cloned().collect::<BTreeSet<_>>() != expected {
                return Err("Builtin enum differs from ALL plus internal lowering builtins".into());
            }
        }
        counts.push((name.to_owned(), expected.len()));
    }
    Ok(counts)
}
