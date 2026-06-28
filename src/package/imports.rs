use std::collections::BTreeSet;
use std::fs;
use std::path::PathBuf;

use crate::ast::{Block, BlockItem, Expr, ImportDecl, ImportOrigin, Program, TopLevelItem};
use crate::error::{Error, Result};
use crate::package::PackageSource;
use crate::parser::parse_program;

pub(crate) fn expand_package_imports(
    program: &mut Program,
    packages: &[PackageSource],
) -> Result<()> {
    let mut loaded_modules = BTreeSet::new();
    expand_package_imports_with_loaded(program, packages, &mut loaded_modules)
}

fn expand_package_imports_with_loaded(
    program: &mut Program,
    packages: &[PackageSource],
    loaded_modules: &mut BTreeSet<String>,
) -> Result<()> {
    let mut source_imports = Vec::new();
    program.items.retain(|item| {
        let TopLevelItem::Import(import) = item else {
            return true;
        };
        if is_source_package_import(import, packages) {
            source_imports.push(import.clone());
            false
        } else {
            true
        }
    });

    for import in source_imports {
        let package_name = import.path.first().expect("import path is not empty");
        let module_path = source_module_path(&import)?;
        let module_key = module_path.join(".");
        let import_key = format!("{package_name}.{module_key}.{}", import.name);
        let mut module = load_source_package_module(package_name, packages, &module_path)?;
        if !module_exports(&module, &import.name) {
            return Err(Error::new(format!(
                "package module `{package_name}.{}` does not export `{}`",
                module_key, import.name
            )));
        }
        retain_item_dependencies(&mut module, &import.name);
        if loaded_modules.insert(import_key) {
            expand_package_imports_with_loaded(&mut module, packages, loaded_modules)?;
            if package_name == "std" {
                mark_stdlib_origin(&mut module);
            }
            program.items.extend(module.items);
        }
    }

    Ok(())
}

pub(crate) fn mark_stdlib_origin(program: &mut Program) {
    for item in &mut program.items {
        if let TopLevelItem::Import(import) = item {
            import.origin = ImportOrigin::Stdlib;
        }
    }
}

fn is_source_package_import(import: &ImportDecl, packages: &[PackageSource]) -> bool {
    let Some(package) = import.path.first() else {
        return false;
    };
    package == "std" || packages.iter().any(|source| source.name == *package)
}

fn source_module_path(import: &ImportDecl) -> Result<Vec<String>> {
    if import.path.len() < 2 {
        return Err(Error::new(format!(
            "source package import `{}` must include a module path",
            format_import_path(&import.path, &import.name)
        )));
    }
    Ok(import.path[1..].to_vec())
}

fn load_source_package_module(
    package_name: &str,
    packages: &[PackageSource],
    module_path: &[String],
) -> Result<Program> {
    let (source, label) = if package_name == "std" {
        if let Some(package) = packages.iter().find(|package| package.name == "std") {
            load_package_module(package, module_path)?
        } else {
            return Err(Error::new(
                "package `std` is not available; add it with `emela package add std --git URL --rev REV` or pass `--package DIR`",
            ));
        }
    } else {
        let package = packages
            .iter()
            .find(|package| package.name == package_name)
            .ok_or_else(|| Error::new(format!("unknown package `{package_name}`")))?;
        load_package_module(package, module_path)?
    };
    parse_program(&label, &source)
}

fn load_package_module(
    package: &PackageSource,
    module_path: &[String],
) -> Result<(String, String)> {
    load_module_file(
        package.source_root.clone(),
        module_path,
        &format!("package `{}`", package.name),
    )
}

fn load_module_file(
    mut root: PathBuf,
    module_path: &[String],
    label: &str,
) -> Result<(String, String)> {
    for part in module_path {
        root.push(part);
    }
    root.set_extension("emel");
    let source = fs::read_to_string(&root).map_err(|err| {
        Error::new(format!(
            "failed to read {label} module `{}`: {err}",
            root.display()
        ))
    })?;
    Ok((source, root.display().to_string()))
}

fn module_exports(module: &Program, name: &str) -> bool {
    module.items.iter().any(|item| match item {
        TopLevelItem::Function(function) => function.name == name,
        TopLevelItem::Import(_) => false,
        TopLevelItem::Struct(decl) => decl.name == name,
        TopLevelItem::Enum(decl) => decl.name == name,
    })
}

fn retain_item_dependencies(module: &mut Program, export_name: &str) {
    let mut needed = BTreeSet::from([export_name.to_string()]);
    loop {
        let before = needed.len();
        for function in module.functions() {
            if needed.contains(&function.name) {
                collect_block_dependencies(&function.body, &mut needed);
            }
        }
        if needed.len() == before {
            break;
        }
    }

    module.items.retain(|item| match item {
        TopLevelItem::Function(function) => needed.contains(&function.name),
        TopLevelItem::Import(import) => needed.contains(&import.name),
        TopLevelItem::Struct(_) | TopLevelItem::Enum(_) => true,
    });
}

fn collect_block_dependencies(block: &Block, needed: &mut BTreeSet<String>) {
    for item in &block.items {
        match item {
            BlockItem::Binding { expr, .. } | BlockItem::Expr(expr) => {
                collect_expr_dependencies(expr, needed);
            }
        }
    }
}

fn collect_expr_dependencies(expr: &Expr, needed: &mut BTreeSet<String>) {
    match expr {
        Expr::Var(name, _) => {
            needed.insert(name.clone());
        }
        Expr::String(_, _) => {}
        Expr::Call { name, args, .. } => {
            needed.insert(name.clone());
            for arg in args {
                collect_expr_dependencies(arg, needed);
            }
        }
        Expr::MethodCall { receiver, args, .. } => {
            collect_expr_dependencies(receiver, needed);
            for arg in args {
                collect_expr_dependencies(arg, needed);
            }
        }
        Expr::FieldAccess { receiver, .. } => collect_expr_dependencies(receiver, needed),
        Expr::StructLiteral { value, .. } => collect_expr_dependencies(value, needed),
        Expr::Binary { left, right, .. } => {
            collect_expr_dependencies(left, needed);
            collect_expr_dependencies(right, needed);
        }
        Expr::Match {
            scrutinee, arms, ..
        } => {
            collect_expr_dependencies(scrutinee, needed);
            for arm in arms {
                collect_expr_dependencies(&arm.expr, needed);
            }
        }
        Expr::Lambda { body, .. } => collect_expr_dependencies(body, needed),
        Expr::Block(block, _) => collect_block_dependencies(block, needed),
        Expr::Int(_, _) | Expr::Bool(_, _) | Expr::Unit(_) => {}
    }
}

fn format_import_path(path: &[String], name: &str) -> String {
    let mut parts = path.to_vec();
    parts.push(name.to_string());
    parts.join(".")
}
