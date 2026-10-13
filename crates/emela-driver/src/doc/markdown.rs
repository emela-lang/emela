//! Writing the collected docs as Markdown: `index.md` and one page per module.

use std::fmt::Write;

use super::{DocItem, DocModule};
use crate::output::OutputFile;

/// `index.md` listing the modules under `title`, and one `<Module>.md` per module.
pub fn render_markdown(title: &str, modules: &[DocModule]) -> Vec<OutputFile> {
    let mut index = format!("# {title}\n\n");
    for module in modules {
        writeln!(index, "- [{0}]({0}.md)", module.name).unwrap();
    }
    let mut files = vec![OutputFile::new("index.md", index)];
    files.extend(
        modules
            .iter()
            .map(|module| OutputFile::new(format!("{}.md", module.name), module_page(module))),
    );
    files
}

/// Types first, then values (as in Gleam's docs), each in source order. An empty section is
/// left out.
fn module_page(module: &DocModule) -> String {
    let mut page = format!("# {}\n", module.name);
    let sections = [("Types", true), ("Values", false)];
    for (heading, types) in sections {
        let items: Vec<&DocItem> = module
            .items
            .iter()
            .filter(|item| item.kind.is_type() == types)
            .collect();
        if items.is_empty() {
            continue;
        }
        write!(page, "\n## {heading}\n").unwrap();
        for item in items {
            write_item(&mut page, item);
        }
    }
    page
}

/// The name as a heading, the signature as a code block, the doc, and the children as a list.
fn write_item(page: &mut String, item: &DocItem) {
    write!(
        page,
        "\n### {}\n\n```emela\n{}\n```\n",
        item.name, item.signature
    )
    .unwrap();
    if !item.doc.is_empty() {
        write!(page, "\n{}\n", item.doc).unwrap();
    }
    if item.children.is_empty() {
        return;
    }
    page.push('\n');
    for child in &item.children {
        writeln!(page, "- `{}`", child.signature).unwrap();
        if child.doc.is_empty() {
            continue;
        }
        // A blank line makes the doc its own paragraph instead of continuing the signature's
        // line, and the indent keeps a doc of several paragraphs inside the list item.
        page.push('\n');
        for line in child.doc.lines() {
            if line.is_empty() {
                page.push('\n');
            } else {
                writeln!(page, "  {line}").unwrap();
            }
        }
    }
}
