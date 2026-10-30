use emela_emb_inventory::{backend, definitions, implementation};
use std::path::PathBuf;

fn run() -> Result<(), String> {
    let mut args = std::env::args().skip(1).peekable();
    let command = if args.peek().is_some_and(|arg| !arg.starts_with('-')) {
        args.next().unwrap()
    } else {
        "all".into()
    };
    let mut root = definitions::repository_root().to_path_buf();
    let mut out_dir = None;
    let mut config = None;
    let (mut check, mut require) = (false, false);
    for_command(&command)?;
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--root" => root = PathBuf::from(args.next().ok_or("--root requires a path")?),
            "--out-dir" => {
                out_dir = Some(PathBuf::from(
                    args.next().ok_or("--out-dir requires a path")?,
                ))
            }
            "--config" => {
                config = Some(PathBuf::from(
                    args.next().ok_or("--config requires a path")?,
                ))
            }
            "--check" => check = true,
            "--require-evidence" => require = true,
            "--help" | "-h" => {
                println!(
                    "Usage: emela-emb-inventory <all|definitions|backend|implementation|tests> [--root REPO] [--out-dir PATH] [--check] [--require-evidence] [--config PATH]\n\nAudits Rust source without building compiler crates.\nGeneration saves reports before returning failure for audit gaps.\n--check never writes files. all/backend require complete reports; implementation can use --require-evidence."
                );
                return Ok(());
            }
            _ => return Err(format!("unknown argument {arg}")),
        }
    }
    let root = definitions::absolute(&root)?;
    let crate_dir = root.join("crates/emela-codegen-emb");
    let out_dir =
        definitions::absolute(&out_dir.unwrap_or_else(|| crate_dir.join("docs/generated")))?;
    let declarations = definitions::collect(&root, definitions::default_scope())?;
    if command == "tests" {
        backend::check_tests(
            &crate_dir,
            &serde_json::to_value(&declarations).map_err(|e| e.to_string())?,
        )?;
        println!(
            "Verified source test associations in {}",
            crate_dir.display()
        );
        return Ok(());
    }
    let mut reports = definitions::reports(&declarations, &root, &out_dir)?;
    let mut issues = Vec::new();
    if command == "all" || command == "backend" {
        match backend::generate(
            &crate_dir,
            &serde_json::to_value(&declarations).map_err(|e| e.to_string())?,
            &out_dir,
        ) {
            Ok(report) => {
                issues.extend(
                    report
                        .issues
                        .iter()
                        .map(|i| format!("{}: {}", i.id, i.message)),
                );
                reports.push(("flow-backend-inventory.md", report.markdown));
                reports.push(("flow-backend-inventory.json", report.json));
            }
            Err(message) => issues.push(format!("backend inventory: {message}")),
        }
    }
    if command == "all" || command == "implementation" {
        let path = definitions::absolute(
            &config.unwrap_or_else(|| crate_dir.join("tools/flow-implementation-routes.json")),
        )?;
        let path = path
            .strip_prefix(&root)
            .map_err(|_| "configuration must be inside the audited repository")?;
        match implementation::collect(&root, path, &declarations) {
            Ok((inventory, coverage)) => {
                reports.extend(implementation::reports(
                    &inventory, &coverage, &root, &out_dir,
                )?);
                println!(
                    "{} missing context records; {} representation gaps; {} route errors",
                    coverage
                        .summary
                        .get("missing_evidence")
                        .copied()
                        .unwrap_or(0),
                    coverage.representation_gaps.len(),
                    coverage.route_errors.len()
                );
                if (command == "all" || require)
                    && let Err(message) = implementation::require_evidence(&coverage)
                {
                    issues.push(message);
                }
            }
            Err(message) => issues.push(format!("implementation inventory: {message}")),
        }
    }
    // Save every usable report before audit failures become a nonzero exit code.
    let saved = definitions::save(&reports, &out_dir, check);
    println!(
        "{} {} reports from {} to {}",
        if check { "Checked" } else { "Generated" },
        reports.len(),
        root.display(),
        out_dir.display()
    );
    if let Err(message) = saved {
        issues.push(message);
    }
    if issues.is_empty() {
        Ok(())
    } else {
        Err(issues.join("\n"))
    }
}

fn for_command(command: &str) -> Result<(), String> {
    if ["all", "definitions", "backend", "implementation", "tests"].contains(&command) {
        Ok(())
    } else {
        Err(format!("unknown command {command}"))
    }
}

fn main() {
    if let Err(error) = run() {
        eprintln!("emela-emb-inventory: {error}");
        std::process::exit(1);
    }
}
