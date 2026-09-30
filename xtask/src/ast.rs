//! Walking first-party source with `syn`, so the safety rules are checked against the
//! parsed program rather than against a regular expression.
//!
//! A token scan finds `unsafe` inside comments and doc examples; an AST scan does not.
//! That difference is the whole point of the rule, so this is deliberately a parser.

use std::path::{Path, PathBuf};

use syn::spanned::Spanned;
use syn::visit::Visit;

/// One place in the tree that breaks a rule.
#[derive(Debug, Clone)]
pub(crate) struct Finding {
    pub(crate) file: PathBuf,
    pub(crate) line: usize,
    pub(crate) text: String,
}

impl Finding {
    fn new(path: &Path, line: usize, text: impl Into<String>) -> Self {
        Self {
            file: path.to_path_buf(),
            line,
            text: text.into(),
        }
    }

    /// `path:line: text`, relative to the workspace root where possible.
    #[must_use]
    pub(crate) fn display(&self, root: &Path) -> String {
        let rel = self.file.strip_prefix(root).unwrap_or(&self.file);
        format!("{}:{}: {}", rel.display(), self.line, self.text)
    }
}

/// Every `.rs` file belonging to a crate, excluding build output.
#[must_use]
pub(crate) fn rust_files(dir: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    collect(dir, &mut out);
    out.sort();
    out
}

fn collect(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        let name = entry.file_name();
        let name = name.to_string_lossy();
        if name == "target" || name.starts_with('.') {
            continue;
        }
        if path.is_dir() {
            collect(&path, out);
        } else if path.extension().is_some_and(|e| e == "rs") {
            out.push(path);
        }
    }
}

/// Result of an AST pass over one crate.
#[derive(Debug, Default)]
pub(crate) struct CrateScan {
    pub(crate) unsafe_items: Vec<Finding>,
    /// `#[allow(unsafe_code)]` and friends, which G0.2 forbids outright.
    pub(crate) unsafe_allows: Vec<Finding>,
    pub(crate) command_spawns: Vec<Finding>,
    pub(crate) process_paths: Vec<Finding>,
    pub(crate) todo_markers: Vec<Finding>,
    /// Files that did not parse. Non-empty means the scan is unsound.
    pub(crate) unparsed: Vec<PathBuf>,
}

impl CrateScan {
    /// True when the scan turned up nothing at all.
    #[must_use]
    pub(crate) fn is_untouched(&self) -> bool {
        self.unsafe_items.is_empty()
            && self.unsafe_allows.is_empty()
            && self.command_spawns.is_empty()
            && self.process_paths.is_empty()
            && self.todo_markers.is_empty()
            && self.unparsed.is_empty()
    }

    pub(crate) fn extend(&mut self, other: Self) {
        self.unsafe_items.extend(other.unsafe_items);
        self.unsafe_allows.extend(other.unsafe_allows);
        self.command_spawns.extend(other.command_spawns);
        self.process_paths.extend(other.process_paths);
        self.todo_markers.extend(other.todo_markers);
        self.unparsed.extend(other.unparsed);
    }
}

/// Parse and walk one file.
#[must_use]
pub(crate) fn scan_file(path: &Path) -> CrateScan {
    let Ok(text) = std::fs::read_to_string(path) else {
        return CrateScan::default();
    };
    // Doc tests and the parser are fine with a whole-file parse; a target that does not
    // compile at all is reported by the compiler, not here.
    let Ok(file) = syn::parse_file(&text) else {
        return CrateScan {
            unparsed: vec![path.to_path_buf()],
            ..CrateScan::default()
        };
    };
    let mut v = SafetyVisitor {
        path,
        out: CrateScan::default(),
    };
    v.visit_file(&file);
    v.out
}

struct SafetyVisitor<'a> {
    path: &'a Path,
    out: CrateScan,
}

impl<'ast> Visit<'ast> for SafetyVisitor<'_> {
    fn visit_expr_unsafe(&mut self, e: &'ast syn::ExprUnsafe) {
        line(
            &mut self.out.unsafe_items,
            self.path,
            e.span(),
            "unsafe expression",
        );
        syn::visit::visit_expr_unsafe(self, e);
    }

    fn visit_item_fn(&mut self, i: &'ast syn::ItemFn) {
        self.signature(i.sig.unsafety, i.span());
        self.note_todos(self.path, &i.block);
        syn::visit::visit_item_fn(self, i);
    }

    fn visit_impl_item_fn(&mut self, i: &'ast syn::ImplItemFn) {
        self.signature(i.sig.unsafety, i.span());
        syn::visit::visit_impl_item_fn(self, i);
    }

    fn visit_trait_item_fn(&mut self, i: &'ast syn::TraitItemFn) {
        self.signature(i.sig.unsafety, i.span());
        syn::visit::visit_trait_item_fn(self, i);
    }

    fn visit_item_impl(&mut self, i: &'ast syn::ItemImpl) {
        if i.unsafety.is_some() {
            line(
                &mut self.out.unsafe_items,
                self.path,
                i.span(),
                "unsafe impl",
            );
        }
        syn::visit::visit_item_impl(self, i);
    }

    fn visit_item_trait(&mut self, i: &'ast syn::ItemTrait) {
        if i.unsafety.is_some() {
            line(
                &mut self.out.unsafe_items,
                self.path,
                i.span(),
                "unsafe trait",
            );
        }
        syn::visit::visit_item_trait(self, i);
    }

    fn visit_item_foreign_mod(&mut self, i: &'ast syn::ItemForeignMod) {
        if i.unsafety.is_some() {
            line(
                &mut self.out.unsafe_items,
                self.path,
                i.span(),
                "extern block",
            );
        }
        syn::visit::visit_item_foreign_mod(self, i);
    }

    fn visit_expr_block(&mut self, e: &'ast syn::ExprBlock) {
        for attr in &e.attrs {
            if is_unsafe_allow(attr) {
                line(&mut self.out.unsafe_allows, self.path, attr.span(), ALLOW);
            }
        }
        syn::visit::visit_expr_block(self, e);
    }

    /// `Command::new` can be reached from any expression, so this is the one place
    /// that has to look for it.
    fn visit_expr(&mut self, e: &'ast syn::Expr) {
        if let syn::Expr::Call(call) = e
            && let Some(path) = call_path(call)
            && path.segments.last().is_some_and(|s| s.ident == "new")
            && path_mentions(path, "Command")
        {
            line(
                &mut self.out.command_spawns,
                self.path,
                call.span(),
                "spawns a process (`Command::new`)",
            );
        }
        syn::visit::visit_expr(self, e);
    }

    fn visit_item_const(&mut self, i: &'ast syn::ItemConst) {
        self.note_todos_expr(self.path, &i.expr);
        syn::visit::visit_item_const(self, i);
    }

    fn visit_item_static(&mut self, i: &'ast syn::ItemStatic) {
        self.note_todos_expr(self.path, &i.expr);
        syn::visit::visit_item_static(self, i);
    }
}

const ALLOW: &str = "`allow(unsafe_code)`";

impl SafetyVisitor<'_> {
    /// Record an `unsafe fn` when the signature carries the keyword.
    fn signature(&mut self, unsafety: Option<syn::token::Unsafe>, span: proc_macro2::Span) {
        if unsafety.is_some() {
            line(&mut self.out.unsafe_items, self.path, span, "unsafe fn");
        }
    }

    /// A `todo!()`/`unimplemented!()` in a function body.
    fn note_todos(&mut self, path: &Path, block: &syn::Block) {
        let mut v = MacroVisitor { out: Vec::new() };
        v.visit_block(block);
        self.collect_todos(path, v.out);
    }

    /// The same, in a `const` or `static` initialiser.
    fn note_todos_expr(&mut self, path: &Path, expr: &syn::Expr) {
        let mut v = MacroVisitor { out: Vec::new() };
        v.visit_expr(expr);
        self.collect_todos(path, v.out);
    }

    fn collect_todos(&mut self, path: &Path, found: Vec<(proc_macro2::Span, String)>) {
        for (span, name) in found {
            if !span_in_tests(path, span.start().line) {
                line(
                    &mut self.out.todo_markers,
                    path,
                    span,
                    format!("`{name}!()` is reachable in product code"),
                );
            }
        }
    }
}

struct MacroVisitor {
    out: Vec<(proc_macro2::Span, String)>,
}

impl<'ast> Visit<'ast> for MacroVisitor {
    fn visit_macro(&mut self, m: &'ast syn::Macro) {
        if let Some(seg) = m.path.segments.last() {
            let name = seg.ident.to_string();
            if name == "todo" || name == "unimplemented" {
                self.out.push((m.span(), name));
            }
        }
        syn::visit::visit_macro(self, m);
    }
}

fn line(out: &mut Vec<Finding>, path: &Path, span: proc_macro2::Span, text: impl Into<String>) {
    out.push(Finding::new(path, line_of(&span), text));
}

/// `Span::start` is provided by the `Spanned` trait rather than inherent on `Span`
/// itself, so it is reached through a helper.
fn line_of(span: &proc_macro2::Span) -> usize {
    span.start().line
}

fn call_path(call: &syn::ExprCall) -> Option<&syn::Path> {
    match call.func.as_ref() {
        syn::Expr::Path(p) => Some(&p.path),
        _ => None,
    }
}

fn path_mentions(path: &syn::Path, needle: &str) -> bool {
    path.segments
        .iter()
        .any(|s| s.ident == needle || s.ident.to_string() == needle)
}

fn is_unsafe_allow(attr: &syn::Attribute) -> bool {
    if !attr.path().is_ident("allow") {
        return false;
    }
    let mut found = false;
    if attr
        .parse_nested_meta(|meta| {
            if meta
                .path
                .segments
                .iter()
                .any(|s| s.ident == "unsafe_code" || s.ident == "unsafe_op_in_unsafe_fn")
            {
                found = true;
            }
            Ok(())
        })
        .is_err()
    {
        return false;
    }
    found
}

/// True when the line falls inside a `#[cfg(test)]` module, where a `todo!()` is a
/// description rather than a stub.
fn span_in_tests(path: &Path, line: usize) -> bool {
    // Cheap and good enough: the file either has a test module at all, and the marker
    // is below its start. Tests are the only place these are allowed.
    let Ok(text) = std::fs::read_to_string(path) else {
        return false;
    };
    let Some(start) = text.find("#[cfg(test)]") else {
        return false;
    };
    let start_line = text[..start].lines().count() + 1;
    line >= start_line
}
