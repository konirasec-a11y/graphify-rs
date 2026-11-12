//! Rust syntax ownership and call-site extraction; no method type inference.
use super::layout;
use graphify_core::{
    confidence::Confidence,
    model::{ExtractionResult, GraphEdge, GraphNode, NodeType},
};
use proc_macro2::Span;
use quote::ToTokens;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::{
    collections::{BTreeSet, HashMap},
    path::Path,
};
use syn::{
    spanned::Spanned,
    visit::{self, Visit},
};

pub(super) fn extract(path: &Path, syntax: &syn::File) -> ExtractionResult {
    let context = layout::context(path);
    let source = path.to_string_lossy().into_owned();
    let file_id = rust_id(&[&source]);
    let module_path = join(&context.crate_name, &context.module);
    let mut result = ExtractionResult::default();
    result.nodes.push(GraphNode {
        id: file_id.clone(),
        label: path
            .file_stem()
            .unwrap_or_default()
            .to_string_lossy()
            .into_owned(),
        source_file: source.clone(),
        source_location: None,
        node_type: NodeType::File,
        community: None,
        extra: extras(json!({"language":"rust", "crate_name":context.crate_name,"package_name":context.package_name,"resolution_disabled":context.resolution_disabled,
            "module_path":module_path, "item_kind":"file", "source_layout":true})),
    });
    let mut collector = Collector {
        source,
        file_id: file_id.clone(),
        context,
        module_paths: vec![module_path],
        owner: None,
        trait_owner: false,
        parents: vec![file_id],
        callables: Vec::new(),
        scopes: vec![BTreeSet::new()],
        blocks: Vec::new(),
        result,
    };
    collector.visit_file(syntax);
    collector.result
}

pub(super) fn rust_id(parts: &[&str]) -> String {
    let mut digest = Sha256::new();
    for part in parts {
        digest.update((part.len() as u64).to_le_bytes());
        digest.update(part.as_bytes());
    }
    format!(
        "{}_{:x}",
        graphify_core::id::make_id(parts)
            .chars()
            .take(100)
            .collect::<String>(),
        digest.finalize()
    )
}

fn extras(value: Value) -> HashMap<String, Value> {
    value
        .as_object()
        .expect("literal metadata object")
        .iter()
        .map(|(k, v)| (k.clone(), v.clone()))
        .collect()
}
fn join(root: &str, pieces: &[String]) -> String {
    std::iter::once(root)
        .chain(pieces.iter().map(String::as_str))
        .collect::<Vec<_>>()
        .join("::")
}
fn name(ident: &syn::Ident) -> String {
    ident.to_string().trim_start_matches("r#").into()
}
fn path_name(path: &syn::Path) -> String {
    let prefix = if path.leading_colon.is_some() {
        "::"
    } else {
        ""
    };
    format!(
        "{prefix}{}",
        path.segments
            .iter()
            .map(|p| name(&p.ident))
            .collect::<Vec<_>>()
            .join("::")
    )
}
fn type_name(ty: &syn::Type) -> String {
    match ty {
        syn::Type::Path(p) if p.qself.is_none() => path_name(&p.path),
        _ => ty.to_token_stream().to_string(),
    }
}
fn bindings(pattern: &syn::Pat) -> BTreeSet<String> {
    struct Names(BTreeSet<String>);
    impl<'ast> Visit<'ast> for Names {
        fn visit_pat_ident(&mut self, node: &'ast syn::PatIdent) {
            self.0.insert(name(&node.ident));
            visit::visit_pat_ident(self, node);
        }
    }
    let mut names = Names(BTreeSet::new());
    names.visit_pat(pattern);
    names.0
}

struct DefinitionFlags {
    public: bool,
    has_body: bool,
}

struct Callable {
    id: String,
    qualified: String,
}
struct Collector {
    source: String,
    file_id: String,
    context: layout::Context,
    module_paths: Vec<String>,
    owner: Option<String>,
    trait_owner: bool,
    parents: Vec<String>,
    callables: Vec<Callable>,
    scopes: Vec<BTreeSet<String>>,
    blocks: Vec<Span>,
    result: ExtractionResult,
}

impl Collector {
    fn module_path(&self) -> String {
        self.module_paths.last().cloned().expect("module scope")
    }
    fn lexical_scope(&self) -> String {
        self.callables
            .last()
            .map(|f| f.qualified.clone())
            .unwrap_or_else(|| self.module_path())
    }
    fn qualified(&self, item: &str) -> String {
        if let Some(function) = self.callables.last() {
            format!("{}::{item}", function.qualified)
        } else if let Some(owner) = &self.owner {
            format!("{}::{owner}::{item}", self.module_path())
        } else {
            format!("{}::{item}", self.module_path())
        }
    }
    fn node(
        &mut self,
        qualified: String,
        label: String,
        kind: NodeType,
        item_kind: &str,
        span: Span,
        flags: DefinitionFlags,
    ) -> String {
        let DefinitionFlags { public, has_body } = flags;
        let start = span.start();
        let end = span.end();
        let id = rust_id(&[
            &self.source,
            &qualified,
            &format!("{}:{}", start.line, start.column),
        ]);
        let mut extra = extras(json!({"language":"rust","qualified_name":qualified,
            "module_path":self.module_path(),"crate_name":self.context.crate_name,"package_name":self.context.package_name,"resolution_disabled":self.context.resolution_disabled,
            "item_kind":item_kind,"has_body":has_body,"is_public":public,"end_line":end.line,
            "source_column":start.column+1,"name_resolution":"source-layout-and-imports"}));
        if let Some(parent) = self.callables.last() {
            extra.insert("lexical_parent".into(), json!(parent.id));
        }
        self.result.nodes.push(GraphNode {
            id: id.clone(),
            label,
            source_file: self.source.clone(),
            source_location: Some(format!("L{}", start.line)),
            node_type: kind,
            community: None,
            extra,
        });
        self.syntax_edge(
            self.parents
                .last()
                .cloned()
                .unwrap_or_else(|| self.file_id.clone()),
            id.clone(),
            "defines",
            span,
            "ast:rust-definition",
            HashMap::new(),
        );
        id
    }
    fn syntax_edge(
        &mut self,
        source: String,
        target: String,
        relation: &str,
        span: Span,
        provenance: &str,
        mut extra: HashMap<String, Value>,
    ) {
        extra.insert("language".into(), json!("rust"));
        extra.insert("direction".into(), json!("source_to_target"));
        extra.insert("source_column".into(), json!(span.start().column + 1));
        let confidence = Confidence::Extracted;
        let confidence_score = confidence.default_score();
        self.result.edges.push(GraphEdge {
            source,
            target,
            relation: relation.into(),
            confidence,
            confidence_score,
            source_file: self.source.clone(),
            source_location: Some(format!("L{}", span.start().line)),
            weight: 1.0,
            provenance: Some(provenance.into()),
            extra,
        });
    }
    fn enter_callable(&mut self, id: String, qualified: String, initial: BTreeSet<String>) {
        self.parents.push(id.clone());
        self.callables.push(Callable { id, qualified });
        self.scopes.push(initial);
    }
    fn leave_callable(&mut self) {
        self.scopes.pop();
        self.callables.pop();
        self.parents.pop();
    }
    fn callable(
        &mut self,
        signature: &syn::Signature,
        body: Option<&syn::Block>,
        span: Span,
        public: bool,
        kind: &str,
    ) {
        let identifier = name(&signature.ident);
        let qualified = self.qualified(&identifier);
        let method = ["method", "trait_method"].contains(&kind);
        let label = if method {
            format!(".{identifier}()")
        } else {
            format!("{identifier}()")
        };
        let node_kind = if method {
            NodeType::Method
        } else {
            NodeType::Function
        };
        let id = self.node(
            qualified.clone(),
            label,
            node_kind,
            kind,
            span,
            DefinitionFlags {
                public,
                has_body: body.is_some(),
            },
        );
        let mut initial = BTreeSet::new();
        for parameter in &signature.inputs {
            if let syn::FnArg::Typed(argument) = parameter {
                initial.extend(bindings(&argument.pat));
            }
        }
        initial.extend(signature.generics.type_params().map(|p| name(&p.ident)));
        initial.extend(signature.generics.const_params().map(|p| name(&p.ident)));
        if let Some(block) = body {
            self.enter_callable(id, qualified, initial);
            self.visit_block(block);
            self.leave_callable();
        }
    }
    fn shadowed(&self, first: &str) -> bool {
        self.scopes.iter().rev().any(|scope| scope.contains(first))
    }
    fn call(&mut self, callee: String, call_kind: &str, span: Span) {
        let Some(caller) = self.callables.last() else {
            return;
        };
        let caller_id = caller.id.clone();
        let lexical = self.lexical_scope();
        let module = self.module_path();
        let id = rust_id(&[
            &self.source,
            "call-target",
            &format!("{}:{}", span.start().line, span.start().column),
            &callee,
        ]);
        let first = callee
            .trim_start_matches("::")
            .split("::")
            .next()
            .unwrap_or("");
        let shadowed = self.shadowed(first);
        let mut metadata = extras(
            json!({"callee":callee,"call_kind":call_kind,"module_path":module,
            "syntax_confidence":"EXTRACTED","target_confidence":"UNRESOLVED","confidence_scope":"observed-callsite",
            "crate_name":self.context.crate_name,"package_name":self.context.package_name,"resolution_disabled":self.context.resolution_disabled,"lexical_scope":lexical,"shadowed":shadowed,
            "crate_aliases":self.context.aliases,"unresolved_target":id,"resolution":match call_kind {
                "method"=>"method-unresolved","macro"=>"macro-unexpanded",_=>"unresolved"}}),
        );
        if first == "Self"
            && !self.trait_owner
            && let Some(owner) = &self.owner
        {
            metadata.insert(
                "lookup_paths".into(),
                json!([format!(
                    "{}::{}{}",
                    self.module_path(),
                    owner,
                    callee.trim_start_matches("Self")
                )]),
            );
        }
        self.result.nodes.push(GraphNode{id:id.clone(),label:callee.clone(),source_file:self.source.clone(),source_location:Some(format!("L{}",span.start().line)),node_type:NodeType::Function,community:None,
            extra:extras(json!({"language":"rust","item_kind":"call_target","unresolved":true,"has_body":false,"module_path":self.module_path(),"crate_name":self.context.crate_name}))});
        self.syntax_edge(caller_id, id, "calls", span, "ast:rust-callsite", metadata);
    }
    fn use_tree(&mut self, prefix: &str, tree: &syn::UseTree, public: bool, span: Span) {
        match tree {
            syn::UseTree::Path(p) => self.use_tree(
                &format!("{prefix}{}::", name(&p.ident)),
                &p.tree,
                public,
                span,
            ),
            syn::UseTree::Group(g) => {
                for item in &g.items {
                    self.use_tree(prefix, item, public, span)
                }
            }
            syn::UseTree::Name(n) => {
                let n = name(&n.ident);
                let path = if n == "self" {
                    prefix.trim_end_matches("::").to_owned()
                } else {
                    format!("{prefix}{n}")
                };
                let alias = if n == "self" {
                    prefix
                        .trim_end_matches("::")
                        .rsplit("::")
                        .next()
                        .unwrap_or("")
                        .to_owned()
                } else {
                    n
                };
                self.import(path, alias, false, public, span);
            }
            syn::UseTree::Rename(n) => {
                let n_original = name(&n.ident);
                let path = if n_original == "self" {
                    prefix.trim_end_matches("::").into()
                } else {
                    format!("{prefix}{n_original}")
                };
                self.import(path, name(&n.rename), false, public, span);
            }
            syn::UseTree::Glob(_) => self.import(
                prefix.trim_end_matches("::").into(),
                "*".into(),
                true,
                public,
                span,
            ),
        }
    }
    fn import(&mut self, path: String, alias: String, glob: bool, public: bool, span: Span) {
        let id = rust_id(&[
            &self.source,
            &self.lexical_scope(),
            "import",
            &path,
            &alias,
            &span.start().line.to_string(),
        ]);
        let mut metadata = extras(
            json!({"language":"rust","item_kind":"import","unresolved":true,"has_body":false,
            "module_path":self.module_path(),"lexical_scope":self.lexical_scope(),"binding_scope":self.lexical_scope(),"crate_name":self.context.crate_name,"package_name":self.context.package_name,"resolution_disabled":self.context.resolution_disabled,
            "crate_aliases":self.context.aliases,"import_path":path,"import_alias":alias,"import_glob":glob,"reexport":public}),
        );
        if !self.callables.is_empty()
            && let Some(block) = self.blocks.last()
        {
            metadata.insert("scope_start_line".into(), json!(block.start().line));
            metadata.insert("scope_start_column".into(), json!(block.start().column + 1));
            metadata.insert("scope_end_line".into(), json!(block.end().line));
            metadata.insert("scope_end_column".into(), json!(block.end().column + 1));
        }
        self.result.nodes.push(GraphNode {
            id: id.clone(),
            label: if glob {
                format!("use {path}::*")
            } else {
                format!("use {path} as {alias}")
            },
            source_file: self.source.clone(),
            source_location: Some(format!("L{}", span.start().line)),
            node_type: NodeType::Module,
            community: None,
            extra: metadata.clone(),
        });
        self.syntax_edge(
            self.parents
                .last()
                .cloned()
                .unwrap_or_else(|| self.file_id.clone()),
            id,
            "imports",
            span,
            "ast:rust-import",
            metadata,
        );
    }
    fn condition(&mut self, condition: &syn::Expr) -> BTreeSet<String> {
        match condition {
            syn::Expr::Let(binding) => {
                self.visit_expr(&binding.expr);
                bindings(&binding.pat)
            }
            syn::Expr::Binary(binary) if matches!(binary.op, syn::BinOp::And(_)) => {
                let mut left = self.condition(&binary.left);
                self.scopes.push(left.clone());
                let right = self.condition(&binary.right);
                self.scopes.pop();
                left.extend(right);
                left
            }
            syn::Expr::Paren(parenthesized) => self.condition(&parenthesized.expr),
            syn::Expr::Group(group) => self.condition(&group.expr),
            _ => {
                self.visit_expr(condition);
                BTreeSet::new()
            }
        }
    }
    fn scoped_condition(&mut self, condition: &syn::Expr, body: &syn::Block) {
        let names = self.condition(condition);
        self.scopes.push(names);
        self.visit_block(body);
        self.scopes.pop();
    }
}

mod visitor;
