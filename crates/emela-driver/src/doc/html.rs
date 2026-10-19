//! Writing the collected docs as HTML, in the manner of Gleam's docs: a sidebar of modules, the
//! items of the page with their signatures and docs, and a table of contents.
//!
//! Signatures and Emela code in docs are highlighted with the lexer when the pages are written,
//! so the pages need no script. In signatures, type names link to the item that defines them.

use std::collections::HashMap;
use std::fmt::Write;

use emela_syntax::SyntaxKind::{self, *};
use pulldown_cmark::{CodeBlockKind, Event, Parser, Tag, TagEnd};

use super::{DocItem, DocKind, DocModule};
use crate::output::OutputFile;

const STYLE: &str = include_str!("style.css");

/// `index.html` listing the modules under `title`, one `<Module>.html` per module, and
/// `style.css`.
pub fn render_html(title: &str, modules: &[DocModule]) -> Vec<OutputFile> {
    let links = Links::new(modules);
    let mut index = format!("<h1>{}</h1>\n<ul class=\"module-list\">\n", escape(title));
    for module in modules {
        writeln!(
            index,
            "<li><a href=\"{0}.html\">{0}</a></li>",
            escape(&module.name)
        )
        .unwrap();
    }
    index.push_str("</ul>\n");

    let mut files = vec![
        OutputFile::new("index.html", page(title, title, modules, None, "", &index)),
        OutputFile::new("style.css", STYLE),
    ];
    for module in modules {
        let (main, toc) = module_page(module, &links);
        files.push(OutputFile::new(
            format!("{}.html", module.name),
            page(
                title,
                &module.name,
                modules,
                Some(&module.name),
                &toc,
                &main,
            ),
        ));
    }
    files
}

/// The whole page: the sidebar of modules, `main`, and the table of contents.
fn page(
    title: &str,
    page_title: &str,
    modules: &[DocModule],
    current: Option<&str>,
    toc: &str,
    main: &str,
) -> String {
    let mut sidebar = String::new();
    for module in modules {
        let class = if current == Some(module.name.as_str()) {
            " class=\"current\""
        } else {
            ""
        };
        writeln!(
            sidebar,
            "<li><a href=\"{0}.html\"{class}>{0}</a></li>",
            escape(&module.name)
        )
        .unwrap();
    }
    let head_title = if page_title == title {
        escape(title)
    } else {
        format!("{} · {}", escape(page_title), escape(title))
    };
    format!(
        r#"<!DOCTYPE html>
<html lang="en">
<head>
<meta charset="utf-8">
<meta name="viewport" content="width=device-width, initial-scale=1">
<title>{head_title}</title>
<link rel="stylesheet" href="style.css">
</head>
<body>
<header class="topbar"><a href="index.html">{title}</a></header>
<div class="layout">
<nav class="sidebar">
<h2>Modules</h2>
<ul>
{sidebar}</ul>
</nav>
<main class="content">
{main}</main>
<aside class="toc">
{toc}</aside>
</div>
</body>
</html>
"#,
        title = escape(title),
    )
}

/// The module's items (types first, then values, each in source order) and the table of
/// contents for them.
fn module_page(module: &DocModule, links: &Links) -> (String, String) {
    let mut main = format!("<h1 class=\"module-name\">{}</h1>\n", escape(&module.name));
    let mut toc = String::new();
    for (heading, types) in [("Types", true), ("Values", false)] {
        let items: Vec<&DocItem> = module
            .items
            .iter()
            .filter(|item| item.kind.is_type() == types)
            .collect();
        if items.is_empty() {
            continue;
        }
        writeln!(main, "<section>\n<h2>{heading}</h2>").unwrap();
        writeln!(toc, "<h2>{heading}</h2>\n<ul>").unwrap();
        for item in items {
            write_item(&mut main, item, &module.name, links);
            writeln!(toc, "<li><a href=\"#{0}\">{0}</a></li>", escape(&item.name)).unwrap();
        }
        main.push_str("</section>\n");
        toc.push_str("</ul>\n");
    }
    (main, toc)
}

/// One item: its name and kind, the signature, the doc, and the children. Children get ids like
/// `Shape.Circle` so that they can be linked to.
fn write_item(main: &mut String, item: &DocItem, module: &str, links: &Links) {
    let name = escape(&item.name);
    writeln!(
        main,
        "<article class=\"item\" id=\"{name}\">\n\
         <div class=\"item-head\"><h3><a href=\"#{name}\">{name}</a></h3>\
         <span class=\"kind\">{}</span></div>\n\
         <pre class=\"signature\"><code>{}</code></pre>",
        kind_label(item.kind),
        highlight(&item.signature, Some((links, module))),
    )
    .unwrap();
    if !item.doc.is_empty() {
        writeln!(main, "<div class=\"doc\">\n{}</div>", markdown(&item.doc)).unwrap();
    }
    if !item.children.is_empty() {
        main.push_str("<ul class=\"children\">\n");
        for child in &item.children {
            writeln!(
                main,
                "<li id=\"{name}.{}\"><pre class=\"signature\"><code>{}</code></pre>",
                escape(&child.name),
                highlight(&child.signature, Some((links, module))),
            )
            .unwrap();
            if !child.doc.is_empty() {
                writeln!(main, "<div class=\"doc\">\n{}</div>", markdown(&child.doc)).unwrap();
            }
            main.push_str("</li>\n");
        }
        main.push_str("</ul>\n");
    }
    main.push_str("</article>\n");
}

fn kind_label(kind: DocKind) -> &'static str {
    match kind {
        DocKind::Type => "type",
        DocKind::Enum => "enum",
        DocKind::Error => "error",
        DocKind::Effect => "effect",
        DocKind::Handler => "handler",
        DocKind::Layer => "layer",
        DocKind::Trait => "trait",
        DocKind::Const => "const",
        DocKind::Fn => "fn",
    }
}

/// Where each type name is documented: the modules defining a type-side item of that name.
struct Links<'a> {
    by_name: HashMap<&'a str, Vec<&'a str>>,
}

impl<'a> Links<'a> {
    fn new(modules: &'a [DocModule]) -> Self {
        let mut by_name: HashMap<&str, Vec<&str>> = HashMap::new();
        for module in modules {
            for item in module.items.iter().filter(|item| item.kind.is_type()) {
                by_name
                    .entry(item.name.as_str())
                    .or_default()
                    .push(module.name.as_str());
            }
        }
        Links { by_name }
    }

    /// The link for `name` written on the page of `current`: the current module's item if it
    /// has one, otherwise the only module defining it. A name defined in several other modules
    /// is not linked, since the signature does not say which one it is.
    fn href(&self, current: &str, name: &str) -> Option<String> {
        let modules = self.by_name.get(name)?;
        if modules.contains(&current) {
            Some(format!("#{name}"))
        } else if let [module] = modules.as_slice() {
            Some(format!("{module}.html#{name}"))
        } else {
            None
        }
    }
}

/// Emela code as HTML, with a `<span>` per highlighted token. With `links`, type names that
/// are documented become links.
fn highlight(code: &str, links: Option<(&Links, &str)>) -> String {
    let mut out = String::new();
    let mut pos = 0;
    for token in emela_syntax::lex(code).tokens {
        let start = usize::from(token.range.start());
        let end = usize::from(token.range.end());
        if start > pos {
            out.push_str(&escape(&code[pos..start]));
        }
        let text = &code[start..end];
        let href = match (token.kind, links) {
            (TYPE_NAME | UPPER_NAME, Some((links, current))) => links.href(current, text),
            _ => None,
        };
        match (href, token_class(token.kind, text)) {
            (Some(href), _) => write!(
                out,
                "<a class=\"ty\" href=\"{}\">{}</a>",
                escape(&href),
                escape(text)
            )
            .unwrap(),
            (None, Some(class)) => {
                write!(out, "<span class=\"{class}\">{}</span>", escape(text)).unwrap()
            }
            (None, None) => out.push_str(&escape(text)),
        }
        pos = end;
    }
    if pos < code.len() {
        out.push_str(&escape(&code[pos..]));
    }
    out
}

/// The CSS class of a token, or `None` for punctuation, operators and plain names.
fn token_class(kind: SyntaxKind, text: &str) -> Option<&'static str> {
    match kind {
        FN_KW | TYPE_KW | ENUM_KW | ERROR_KW | CONST_KW | TRAIT_KW | IMPL_KW | EFFECT_KW
        | HANDLER_KW | LAYER_KW | IMPORT_KW | PUB_KW | OPAQUE_KW | SUSPEND_KW | IF_KW | ELSE_KW
        | MATCH_KW | FAIL_KW | ESCAPE_KW | USE_KW | WITH_KW | ASSERT_KW | SELF_KW
        | SELF_TYPE_KW | AS_KW | RESERVED_KW => Some("kw"),
        // Words that are keywords only by position (spec 3.2).
        LOWER_NAME if matches!(text, "fails" | "derive" | "init" | "release") => Some("kw"),
        TYPE_NAME | UPPER_NAME => Some("ty"),
        INT | FLOAT => Some("num"),
        STRING_QUOTE | STRING_TEXT => Some("str"),
        COMMENT | DOC_COMMENT => Some("comment"),
        _ => None,
    }
}

/// A doc comment (Markdown) as HTML. Indented code and ```` ```emela ```` blocks are
/// highlighted. Raw HTML in the doc is shown as text, so that a dependency's docs cannot put
/// scripts on the page.
fn markdown(doc: &str) -> String {
    let mut events = Vec::new();
    // `Some(is_emela)` inside a code block.
    let mut code_block: Option<bool> = None;
    let mut code = String::new();
    for event in Parser::new(doc) {
        match event {
            Event::Start(Tag::CodeBlock(kind)) => {
                let is_emela = match kind {
                    CodeBlockKind::Indented => true,
                    CodeBlockKind::Fenced(lang) => matches!(&*lang, "emela" | "emel"),
                };
                code_block = Some(is_emela);
                code.clear();
            }
            Event::Text(text) if code_block.is_some() => code.push_str(&text),
            Event::End(TagEnd::CodeBlock) => {
                let body = if code_block == Some(true) {
                    highlight(&code, None)
                } else {
                    escape(&code)
                };
                events.push(Event::Html(
                    format!("<pre><code>{body}</code></pre>\n").into(),
                ));
                code_block = None;
            }
            Event::Html(html) | Event::InlineHtml(html) => events.push(Event::Text(html)),
            event => events.push(event),
        }
    }
    let mut out = String::new();
    pulldown_cmark::html::push_html(&mut out, events.into_iter());
    out
}

fn escape(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for c in text.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&#39;"),
            c => out.push(c),
        }
    }
    out
}
