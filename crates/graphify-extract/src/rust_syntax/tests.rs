use super::*;
use graphify_core::{
    confidence::Confidence,
    model::{GraphEdge, GraphNode},
};
use serde_json::Value;
use std::path::PathBuf;

struct Fixture(tempfile::TempDir);
impl Fixture {
    fn new() -> Self {
        let root = tempfile::tempdir().unwrap();
        std::fs::write(
            root.path().join("Cargo.toml"),
            "[package]\nname = \"fixture\"\nversion = \"0.1.0\"\n",
        )
        .unwrap();
        Self(root)
    }
    fn file(&self, path: &str, source: &str) -> ExtractionResult {
        let path = self.0.path().join(path);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, source).unwrap();
        extract(&path, source)
    }
    fn path(&self, path: &str) -> PathBuf {
        self.0.path().join(path)
    }
}
fn text<'a>(node: &'a GraphNode, key: &str) -> &'a str {
    node.extra.get(key).and_then(Value::as_str).unwrap_or("")
}
fn definition<'a>(graph: &'a ExtractionResult, qualified: &str) -> &'a GraphNode {
    graph
        .nodes
        .iter()
        .find(|node| text(node, "qualified_name") == qualified)
        .unwrap_or_else(|| panic!("missing {qualified}"))
}
fn resolved_calls(graph: &ExtractionResult) -> Vec<&GraphEdge> {
    graph
        .edges
        .iter()
        .filter(|e| {
            e.relation == "calls"
                && e.extra
                    .get("resolution")
                    .and_then(Value::as_str)
                    .is_some_and(|s| ["lexical", "qualified-path", "import-alias"].contains(&s))
        })
        .collect()
}
fn combine(parts: Vec<ExtractionResult>) -> ExtractionResult {
    let mut graph = ExtractionResult::default();
    for part in parts {
        graph.nodes.extend(part.nodes);
        graph.edges.extend(part.edges);
    }
    resolve_project(&mut graph);
    graph
}

#[test]
fn identical_names_in_inline_modules_remain_distinct_and_case_sensitive() {
    let fixture = Fixture::new();
    let graph = fixture.file(
        "src/lib.rs",
        r#"
mod first { fn same() {} fn run() { same(); } }
mod second { fn same() {} fn run() { same(); } }
fn Same() {} fn same() {} fn run() { same(); }
"#,
    );
    let calls = resolved_calls(&graph);
    assert_eq!(calls.len(), 3);
    for module in ["fixture::first", "fixture::second", "fixture"] {
        let from = definition(&graph, &format!("{module}::run"));
        let to = definition(&graph, &format!("{module}::same"));
        assert!(
            calls
                .iter()
                .any(|e| e.source == from.id && e.target == to.id)
        );
    }
    let ids: std::collections::BTreeSet<_> = graph.nodes.iter().map(|n| &n.id).collect();
    assert_eq!(ids.len(), graph.nodes.len());
}

#[test]
fn inherent_and_trait_implementations_never_overwrite_methods() {
    let fixture = Fixture::new();
    let graph = fixture.file(
        "src/lib.rs",
        r#"
struct Item; trait A { fn run(); } trait B { fn run(); }
impl Item { fn run() {} }
impl A for Item { fn run() {} }
impl B for Item { fn run() {} }
fn main() { Item::run(); }
"#,
    );
    let targets: Vec<_> = graph
        .nodes
        .iter()
        .filter(|n| n.extra.get("has_body") == Some(&Value::Bool(true)) && n.label == ".run()")
        .collect();
    assert_eq!(targets.len(), 3);
    assert_eq!(resolved_calls(&graph).len(), 1);
    assert_eq!(
        resolved_calls(&graph)[0].target,
        definition(&graph, "fixture::Item::run").id
    );
}

#[test]
fn nested_functions_closures_and_async_blocks_own_their_calls() {
    let fixture = Fixture::new();
    let graph = fixture.file(
        "src/lib.rs",
        r#"
fn target() {}
fn outer() {
    fn inner() { target(); }
    let closure = || target();
    let future = async { target(); };
    inner();
}
"#,
    );
    let target = definition(&graph, "fixture::target");
    let outer = definition(&graph, "fixture::outer");
    let inner = definition(&graph, "fixture::outer::inner");
    let calls = resolved_calls(&graph);
    assert!(
        calls
            .iter()
            .any(|e| e.source == inner.id && e.target == target.id)
    );
    assert!(
        !calls
            .iter()
            .any(|e| e.source == outer.id && e.target == target.id)
    );
    assert!(
        calls
            .iter()
            .any(|e| e.source == outer.id && e.target == inner.id)
    );
    assert_eq!(calls.iter().filter(|e| e.target == target.id).count(), 3);
}

#[test]
fn recursion_and_two_calls_on_one_line_keep_exact_sites() {
    let fixture = Fixture::new();
    let mut graph = fixture.file("src/lib.rs", "fn work() { work(); work(); }\n");
    crate::dedup::dedup_file(&mut graph);
    let function = definition(&graph, "fixture::work");
    let calls = resolved_calls(&graph);
    assert_eq!(calls.len(), 2);
    assert!(
        calls
            .iter()
            .all(|e| e.source == function.id && e.target == function.id)
    );
    assert_eq!(calls[0].source_location.as_deref(), Some("L1"));
    assert_ne!(
        calls[0].extra["source_column"],
        calls[1].extra["source_column"]
    );
    assert!(calls.iter().all(
        |e| e.extra["direction"] == "source_to_target" && e.confidence == Confidence::Inferred
    ));
}

#[test]
fn receiver_dispatch_literals_macros_and_function_pointers_are_not_resolved_by_name() {
    let fixture = Fixture::new();
    let graph=fixture.file("src/lib.rs",r#"
fn get() {} fn target() {}
fn run(target: fn()) { let text = "get(); target()"; receiver.get(); target(); println!("target()"); }
"#);
    assert!(resolved_calls(&graph).is_empty());
    let calls: Vec<_> = graph
        .edges
        .iter()
        .filter(|e| e.relation == "calls")
        .collect();
    assert_eq!(calls.len(), 3);
    assert!(
        calls
            .iter()
            .any(|e| e.extra["resolution"] == "method-unresolved")
    );
    assert!(calls.iter().any(|e| e.extra["call_kind"] == "macro"));
}

#[test]
fn let_initializer_uses_old_binding_and_inner_block_does_not_leak() {
    let fixture = Fixture::new();
    let graph = fixture.file(
        "src/lib.rs",
        r#"
fn target() {} fn run() { let target = target(); target(); }
fn other() { { let target = || {}; target(); } target(); }
"#,
    );
    let calls = resolved_calls(&graph);
    assert_eq!(calls.len(), 2);
    assert!(
        graph
            .edges
            .iter()
            .filter(|e| e.relation == "calls")
            .any(|e| e.extra["shadowed"] == true)
    );
}

#[test]
fn generic_type_and_local_type_alias_do_not_bind_to_a_global_struct() {
    let fixture = Fixture::new();
    let graph = fixture.file(
        "src/lib.rs",
        r#"
struct T; impl T { fn run() {} }
fn generic<T>() { T::run(); }
fn alias() { T::run(); type T = Hidden; }
"#,
    );
    assert!(resolved_calls(&graph).is_empty());
}

#[test]
fn duplicate_cfg_definitions_are_ambiguous_not_last_write_wins() {
    let fixture = Fixture::new();
    let graph = fixture.file(
        "src/lib.rs",
        r#"
#[cfg(unix)] fn target() {}
#[cfg(windows)] fn target() {}
fn run() { target(); }
"#,
    );
    let call = graph.edges.iter().find(|e| e.relation == "calls").unwrap();
    assert_eq!(call.confidence, Confidence::Ambiguous);
    assert_eq!(call.extra["resolution"], "ambiguous");
    assert_eq!(call.extra["candidate_ids"].as_array().unwrap().len(), 2);
}

#[test]
fn cross_file_alias_and_public_glob_facade_require_actual_calls() {
    let fixture = Fixture::new();
    let graph=combine(vec![
        fixture.file("src/lib.rs","pub mod api { pub use crate::operations::*; }\nmod operations;\nfn main() { api::work(); }"),
        fixture.file("src/operations.rs","pub fn work() {} pub fn unused() {}"),
    ]);
    let calls = resolved_calls(&graph);
    assert_eq!(calls.len(), 1);
    assert_eq!(
        calls[0].target,
        definition(&graph, "fixture::operations::work").id
    );
    assert!(
        !calls
            .iter()
            .any(|e| e.target == definition(&graph, "fixture::operations::unused").id)
    );
}

#[test]
fn project_resolution_is_rerun_after_loading_single_file_results() {
    let fixture = Fixture::new();
    let source = "use crate::helpers::work as perform;\nfn run() { perform(); }";
    let partial = fixture.file("src/lib.rs", source);
    assert!(resolved_calls(&partial).is_empty());
    let helpers = fixture.file("src/helpers.rs", "pub fn work() {}");
    let mut combined = combine(vec![partial, helpers]);
    let first = resolved_calls(&combined)[0].target.clone();
    resolve_project(&mut combined);
    assert_eq!(resolved_calls(&combined)[0].target, first);
    assert_eq!(
        std::fs::read_to_string(fixture.path("src/lib.rs")).unwrap(),
        source
    );
}

#[test]
fn invalid_rust_is_visible_and_never_regex_guessed() {
    let fixture = Fixture::new();
    let result = fixture.file("src/lib.rs", "fn not_valid(");
    assert_eq!(result.nodes.len(), 1);
    assert!(result.nodes[0].extra.contains_key("extraction_error"));
    assert!(result.edges.is_empty());
}

#[test]
fn let_chain_binds_patterns_before_evaluating_later_conditions() {
    let fixture = Fixture::new();
    let graph = fixture.file(
        "src/lib.rs",
        r#"
fn target() -> bool { true }
fn run(opt: Option<fn()->bool>) { if let Some(target) = opt && target() { target(); } }
"#,
    );
    assert!(resolved_calls(&graph).is_empty());
    assert_eq!(
        graph
            .edges
            .iter()
            .filter(|edge| edge.relation == "calls" && edge.extra["resolution"] == "shadowed")
            .count(),
        2
    );
}

#[test]
fn trait_generics_do_not_resolve_to_unrelated_same_named_structs() {
    let fixture = Fixture::new();
    let graph = fixture.file(
        "src/lib.rs",
        r#"
struct T; impl T { fn make() {} }
trait H { fn make(); }
trait C<T: H> { fn run() { T::make(); } }
"#,
    );
    assert!(resolved_calls(&graph).is_empty());
    let call = graph
        .edges
        .iter()
        .find(|edge| edge.relation == "calls")
        .unwrap();
    assert_eq!(call.extra["resolution"], "shadowed");
}

#[test]
fn rust_ids_preserve_path_case_instead_of_colliding_after_normalization() {
    let fixture = Fixture::new();
    let graph = combine(vec![
        fixture.file("src/Foo.rs", "pub fn work() {}"),
        fixture.file("src/foo.rs", "pub fn work() {}"),
    ]);
    let ids: std::collections::BTreeSet<_> = graph.nodes.iter().map(|node| &node.id).collect();
    assert_eq!(ids.len(), graph.nodes.len());
    assert_ne!(
        definition(&graph, "fixture::Foo::work").id,
        definition(&graph, "fixture::foo::work").id
    );
}

#[test]
fn binary_root_cannot_steal_a_same_package_library_function() {
    let fixture = Fixture::new();
    let library = fixture.file("src/lib.rs", "pub fn library_only() {}");
    let binary = fixture.file("src/main.rs", "fn main() { library_only(); }");
    let graph = combine(vec![library, binary]);
    assert!(resolved_calls(&graph).is_empty());
    let call = graph
        .edges
        .iter()
        .find(|edge| edge.relation == "calls")
        .unwrap();
    assert_eq!(call.extra["resolution"], "unsupported-target");
}

#[test]
fn a_custom_library_root_is_a_root_not_a_filename_module() {
    let fixture = Fixture::new();
    std::fs::write(
        fixture.path("Cargo.toml"),
        "[package]\nname='fixture'\nversion='0.1.0'\n[lib]\npath='code/library.rs'\n",
    )
    .unwrap();
    let graph = fixture.file(
        "code/library.rs",
        "pub fn work() {} fn caller() { crate::work(); }",
    );
    assert_eq!(resolved_calls(&graph).len(), 1);
    definition(&graph, "fixture::work");
}

#[test]
fn remapped_modules_do_not_pretend_their_filename_is_the_declared_namespace() {
    let fixture = Fixture::new();
    let root = fixture.file(
        "src/lib.rs",
        "#[path=\"elsewhere.rs\"] mod actual; fn caller() { elsewhere::work(); }",
    );
    let module = fixture.file("src/elsewhere.rs", "pub fn work() {}");
    let graph = combine(vec![root, module]);
    assert!(resolved_calls(&graph).is_empty());
    assert!(
        graph
            .nodes
            .iter()
            .any(|node| node.extra.contains_key("module_path_override"))
    );
}

#[test]
fn cargo_aliases_do_not_bind_to_an_indexed_unrelated_package_with_the_same_lib_name() {
    for (binding, dependency) in [
        ("zed_ui", "zed_ui = { package='ui', path='../vendor_ui' }"),
        ("ui", "ui = { path='../vendor_ui' }"),
    ] {
        let fixture = Fixture::new();
        std::fs::create_dir_all(fixture.path("frontend/src")).unwrap();
        std::fs::create_dir_all(fixture.path("vendor_ui/src")).unwrap();
        std::fs::write(fixture.path("frontend/Cargo.toml"), format!(
            "[package]\nname='bellatrix_ui'\nversion='0.1.0'\n[lib]\nname='ui'\n[dependencies]\n{dependency}\n"
        )).unwrap();
        std::fs::write(
            fixture.path("vendor_ui/Cargo.toml"),
            "[package]\nname='ui'\nversion='0.1.0'\n[lib]\nname='ui'\n",
        )
        .unwrap();
        std::fs::write(fixture.path("vendor_ui/src/lib.rs"), "pub fn work() {}").unwrap();
        // The dependency exists on disk but was excluded from the graph corpus.
        let graph = fixture.file(
            "frontend/src/lib.rs",
            &format!("pub fn work() {{}} pub fn caller() {{ work(); {binding}::work(); }}"),
        );
        assert_eq!(resolved_calls(&graph).len(), 1, "binding={binding}");
        let call = graph
            .edges
            .iter()
            .find(|edge| {
                edge.relation == "calls" && edge.extra["callee"] == format!("{binding}::work")
            })
            .unwrap();
        assert_eq!(call.extra["resolution"], "unresolved");
        assert_eq!(
            resolved_calls(&graph)[0].target,
            definition(&graph, "ui::work").id
        );
    }
}
