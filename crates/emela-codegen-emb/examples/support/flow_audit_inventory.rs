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

pub fn check_document(
    name: &str,
    expected: &BTreeSet<String>,
    document: &str,
) -> Result<(), String> {
    let prefix = format!("<!-- flow-audit:{name} ");
    let lines: Vec<_> = document
        .lines()
        .filter_map(|line| line.strip_prefix(&prefix))
        .collect();
    if lines.len() != 1 {
        return Err(format!(
            "{name}: expected one inventory marker, got {}",
            lines.len()
        ));
    }
    let marker = lines[0]
        .strip_suffix(" -->")
        .ok_or_else(|| format!("{name}: malformed inventory marker"))?;
    let listed: Vec<_> = marker.split_whitespace().map(str::to_owned).collect();
    let listed_set: BTreeSet<_> = listed.iter().cloned().collect();
    if listed_set.len() != listed.len() {
        return Err(format!("{name}: duplicated inventory entries"));
    }
    if &listed_set != expected {
        return Err(format!(
            "{name}: missing {:?}; extra {:?}",
            expected.difference(&listed_set).collect::<Vec<_>>(),
            listed_set.difference(expected).collect::<Vec<_>>()
        ));
    }
    for variant in expected {
        let qualified = format!("`{name}::{variant}`");
        if !document.contains(&qualified) {
            return Err(format!("{name}: {qualified} has no rendered audit entry"));
        }
    }
    Ok(())
}

fn test_functions(directory: &Path, names: &mut BTreeSet<String>) -> Result<(), String> {
    for entry in fs::read_dir(directory).map_err(|e| format!("{}: {e}", directory.display()))? {
        let path = entry.map_err(|e| e.to_string())?.path();
        if path.is_dir() {
            test_functions(&path, names)?;
        } else if path.extension().is_some_and(|ext| ext == "rs") {
            let source = fs::read_to_string(&path).map_err(|e| e.to_string())?;
            // Test fixtures contain large raw strings; only inspect the synchronous test
            // declaration immediately following the attribute, rather than lexing their bodies.
            for declaration in source.split("#[test]").skip(1) {
                if let Some(rest) = declaration.trim_start().strip_prefix("fn ") {
                    let name: String = rest
                        .chars()
                        .take_while(|c| c.is_ascii_alphanumeric() || *c == '_')
                        .collect();
                    if !name.is_empty() {
                        names.insert(name);
                    }
                }
            }
        }
    }
    Ok(())
}

fn table_cells(line: &str) -> Vec<String> {
    let mut cells = vec![String::new()];
    let mut quoted = false;
    for ch in line.trim_matches('|').chars() {
        if ch == '`' {
            quoted = !quoted;
        }
        if ch == '|' && !quoted {
            cells.push(String::new());
        } else {
            cells.last_mut().unwrap().push(ch);
        }
    }
    cells
}

pub fn check_test_references(document: &str, names: &BTreeSet<String>) -> Result<usize, String> {
    let mut column = None;
    let mut references = 0;
    for line in document.lines() {
        if !line.starts_with('|') {
            column = None;
            continue;
        }
        let cells = table_cells(line);
        if let Some(index) = cells
            .iter()
            .position(|cell| cell.contains("試験") || cell.contains("テスト"))
        {
            column = Some(index);
            continue;
        }
        let Some(cell) = column.and_then(|index| cells.get(index)) else {
            continue;
        };
        for reference in cell.split('`').skip(1).step_by(2) {
            let function = reference.rsplit("::").next().unwrap();
            if function.starts_with(|c: char| c.is_ascii_lowercase())
                && function.contains('_')
                && reference
                    .split("::")
                    .all(|part| part.chars().all(|c| c.is_ascii_alphanumeric() || c == '_'))
            {
                if !names.contains(function) {
                    return Err(format!("audit refers to nonexistent test `{reference}`"));
                }
                references += 1;
            }
        }
    }
    if references == 0 {
        return Err("audit has no checked test references".into());
    }
    Ok(references)
}

pub fn check(crate_root: &Path) -> Result<Vec<(String, usize)>, String> {
    let document_path = "docs/implementation.md";
    let document = fs::read_to_string(crate_root.join(document_path))
        .map_err(|e| format!("{document_path}: {e}"))?;
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
        check_document(name, &expected, &document)?;
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
    let mut tests = BTreeSet::new();
    test_functions(&crate_root.join("tests"), &mut tests)?;
    check_test_references(&document, &tests).map_err(|e| format!("{document_path}: {e}"))?;
    Ok(counts)
}
