use super::resolve;
use graphify_core::{
    confidence::Confidence,
    model::{ExtractionResult, GraphEdge, GraphNode},
};
use serde_json::{Value, json};

pub(super) fn function(id: &str, name: &str, module: &str) -> GraphNode {
    serde_json::from_value(json!({
        "id": id, "label": name, "source_file": format!("{id}.rs"),
        "source_location": "L1", "node_type": "function", "language": "rust",
        "qualified_name": name, "module_path": module,
        "crate_name": module.split("::").next().unwrap(), "item_kind": "function",
        "has_body": true, "is_public": true
    }))
    .unwrap()
}

pub(super) fn import(
    id: &str,
    module: &str,
    path: &str,
    alias: &str,
    glob: bool,
    public: bool,
) -> GraphNode {
    serde_json::from_value(json!({
        "id": id, "label": path, "source_file": format!("{id}.rs"),
        "source_location": "L1", "node_type": "module", "language": "rust",
        "module_path": module, "binding_scope": module,
        "crate_name": module.split("::").next().unwrap(), "item_kind": "import",
        "has_body": false, "unresolved": true, "import_path": path,
        "import_alias": alias, "import_glob": glob, "reexport": public
    }))
    .unwrap()
}

pub(super) fn fixture(
    caller: GraphNode,
    callee: &str,
    mut others: Vec<GraphNode>,
) -> ExtractionResult {
    let stub: GraphNode = serde_json::from_value(json!({
        "id": "stub", "label": callee, "source_file": caller.source_file,
        "source_location": "L9", "node_type": "function", "language": "rust",
        "item_kind": "call_target", "has_body": false, "unresolved": true
    }))
    .unwrap();
    let edge: GraphEdge = serde_json::from_value(json!({
        "source": caller.id, "target": "stub", "relation": "calls",
        "confidence": "EXTRACTED", "source_file": caller.source_file,
        "source_location": "L9", "source_column": 4,
        "language": "rust", "call_kind": "path", "callee": callee,
        "module_path": caller.extra["module_path"],
        "crate_name": caller.extra["crate_name"],
        "lexical_scope": caller.extra["qualified_name"], "shadowed": false,
        "unresolved_target": "stub", "provenance": "ast:rust:call"
    }))
    .unwrap();
    others.push(caller);
    others.push(stub);
    ExtractionResult {
        nodes: others,
        edges: vec![edge],
        hyperedges: vec![],
    }
}

pub(super) fn resolution(result: &ExtractionResult) -> &str {
    result.edges[0].extra["resolution"].as_str().unwrap()
}

#[test]
fn unqualified_functions_require_the_callers_lexical_namespace_and_keep_case() {
    let caller = function("run", "app::a::run", "app::a");
    let mut graph = fixture(
        caller,
        "save",
        vec![
            function("a_save", "app::a::save", "app::a"),
            function("b_save", "app::b::save", "app::b"),
            function("upper", "app::a::Save", "app::a"),
        ],
    );
    resolve(&mut graph);
    assert_eq!(graph.edges[0].target, "a_save");
    assert_eq!(resolution(&graph), "lexical");
    assert_eq!(graph.edges[0].confidence, Confidence::Inferred);
    assert_eq!(graph.edges[0].source_location.as_deref(), Some("L9"));
    assert_eq!(graph.edges[0].extra["source_column"], json!(4));
    assert_eq!(graph.edges[0].extra["direction"], "source_to_target");
}

#[test]
fn an_unrelated_global_basename_is_never_a_fallback() {
    let mut graph = fixture(
        function("run", "app::a::run", "app::a"),
        "save",
        vec![
            function("b_save", "app::b::save", "app::b"),
            function("upper", "app::a::Save", "app::a"),
        ],
    );
    resolve(&mut graph);
    assert_eq!(graph.edges[0].target, "stub");
    assert_eq!(resolution(&graph), "unresolved");
}

#[test]
fn cross_file_aliases_follow_public_api_glob_reexports() {
    let mut graph = fixture(
        function("run", "app::ui::run", "app::ui"),
        "create",
        vec![
            import(
                "binding",
                "app::ui",
                "engine::api::prepare",
                "create",
                false,
                false,
            ),
            import("reexport", "engine::api", "self::commands", "", true, true),
            function(
                "prepare",
                "engine::api::commands::prepare",
                "engine::api::commands",
            ),
        ],
    );
    resolve(&mut graph);
    assert_eq!(graph.edges[0].target, "prepare");
    assert_eq!(resolution(&graph), "import-alias");
    assert_eq!(graph.edges.len(), 1, "imports cannot manufacture calls");
}

#[test]
fn a_qualified_external_path_follows_reexports_without_an_import_at_the_callsite() {
    let mut graph = fixture(
        function("run", "app::ui::run", "app::ui"),
        "engine::api::prepare",
        vec![
            import(
                "reexport",
                "engine::api",
                "crate::commands::prepare",
                "prepare",
                false,
                true,
            ),
            function("prepare", "engine::commands::prepare", "engine::commands"),
        ],
    );
    resolve(&mut graph);
    assert_eq!(graph.edges[0].target, "prepare");
    assert_eq!(resolution(&graph), "import-alias");
}

#[test]
fn methods_and_shadowed_values_do_not_bind_to_a_same_named_function() {
    for (kind, shadowed, expected) in [
        ("method", false, "method-unresolved"),
        ("dynamic", false, "dynamic-unresolved"),
        ("path", true, "shadowed"),
    ] {
        let mut graph = fixture(
            function("run", "app::run", "app"),
            "save",
            vec![function("save", "app::save", "app")],
        );
        graph.edges[0].extra.insert("call_kind".into(), json!(kind));
        graph.edges[0]
            .extra
            .insert("shadowed".into(), json!(shadowed));
        graph.edges[0]
            .extra
            .insert("lookup_paths".into(), json!(["app::save"]));
        resolve(&mut graph);
        assert_eq!(graph.edges[0].target, "stub");
        assert_eq!(resolution(&graph), expected);
    }
}

#[test]
fn duplicate_cfg_definitions_remain_ambiguous_and_can_reopen_a_cached_resolution() {
    let mut graph = fixture(
        function("run", "app::run", "app"),
        "save",
        vec![function("first", "app::save", "app")],
    );
    resolve(&mut graph);
    assert_eq!(graph.edges[0].target, "first");
    graph.nodes.push(function("second", "app::save", "app"));
    resolve(&mut graph);
    assert_eq!(graph.edges[0].target, "stub");
    assert_eq!(resolution(&graph), "ambiguous");
    assert_eq!(graph.edges[0].confidence, Confidence::Ambiguous);
    assert_eq!(
        graph.edges[0].extra["candidate_ids"],
        json!(["first", "second"])
    );
    let before = serde_json::to_value(&graph).unwrap();
    resolve(&mut graph);
    assert_eq!(serde_json::to_value(&graph).unwrap(), before);
}

#[test]
fn two_glob_candidates_are_ambiguous_but_a_local_item_shadows_them() {
    let mut graph = fixture(
        function("run", "app::run", "app"),
        "save",
        vec![
            import("left_import", "app", "self::left", "", true, false),
            import("right_import", "app", "self::right", "", true, false),
            function("left", "app::left::save", "app::left"),
            function("right", "app::right::save", "app::right"),
        ],
    );
    resolve(&mut graph);
    assert_eq!(resolution(&graph), "ambiguous");
    graph.nodes.push(function("local", "app::save", "app"));
    resolve(&mut graph);
    assert_eq!(graph.edges[0].target, "local");
}

#[test]
fn alias_cycles_terminate_without_a_global_name_guess() {
    let mut graph = fixture(
        function("run", "app::run", "app"),
        "a::save",
        vec![
            import("ab", "app::a", "crate::b::save", "save", false, true),
            import("ba", "app::b", "crate::a::save", "save", false, true),
            function("unrelated", "app::unrelated::save", "app::unrelated"),
        ],
    );
    resolve(&mut graph);
    assert_eq!(graph.edges[0].target, "stub");
    assert_eq!(resolution(&graph), "unresolved");
    assert!(!graph.edges[0].extra.contains_key("resolution_truncated"));
}

#[test]
fn bare_calls_inside_methods_do_not_search_the_type_owner() {
    let mut caller = function("run", "app::Thing::run", "app");
    caller.extra.insert("item_kind".into(), json!("method"));
    let mut graph = fixture(
        caller,
        "helper",
        vec![
            function("associated", "app::Thing::helper", "app"),
            function("module", "app::helper", "app"),
        ],
    );
    resolve(&mut graph);
    assert_eq!(graph.edges[0].target, "module");
}

#[test]
fn nested_functions_can_find_sibling_items_through_their_lexical_parent_id() {
    let owner = function("owner", "app::outer", "app");
    let mut caller = function("run", "app::outer::run", "app");
    caller.extra.insert("lexical_parent".into(), json!("owner"));
    let mut graph = fixture(
        caller,
        "helper",
        vec![
            owner,
            function("nested", "app::outer::helper", "app"),
            function("module", "app::helper", "app"),
        ],
    );
    resolve(&mut graph);
    assert_eq!(graph.edges[0].target, "nested");
}

#[test]
fn trait_defaults_do_not_claim_a_runtime_implementation() {
    let mut target = function("default", "app::Trait::save", "app");
    target
        .extra
        .insert("item_kind".into(), json!("trait_method"));
    let mut graph = fixture(
        function("run", "app::run", "app"),
        "Trait::save",
        vec![target],
    );
    resolve(&mut graph);
    assert_eq!(graph.edges[0].target, "stub");
    assert_eq!(resolution(&graph), "trait-dispatch-unresolved");
}

#[test]
fn explicit_candidate_sets_do_not_pick_the_first_match() {
    let mut graph = fixture(
        function("run", "app::run", "app"),
        "Self::save",
        vec![
            function("first", "app::A::save", "app"),
            function("second", "app::B::save", "app"),
        ],
    );
    graph.edges[0].extra.insert(
        "lookup_paths".into(),
        json!(["app::A::save", "app::B::save"]),
    );
    resolve(&mut graph);
    assert_eq!(resolution(&graph), "ambiguous");
    assert_eq!(
        graph.edges[0].extra["candidate_ids"],
        json!(["first", "second"])
    );
}

#[test]
fn unknown_absolute_extern_paths_do_not_match_a_relative_module() {
    let mut graph = fixture(
        function("run", "app::run", "app"),
        "::external::save",
        vec![function("relative", "app::external::save", "app::external")],
    );
    resolve(&mut graph);
    assert_eq!(graph.edges[0].target, "stub");
}

#[test]
fn private_cross_crate_definitions_and_private_reexports_are_not_followed() {
    let mut private = function("private", "engine::private", "engine");
    private.extra.insert("is_public".into(), json!(false));
    let mut graph = fixture(
        function("run", "app::run", "app"),
        "engine::private",
        vec![private],
    );
    resolve(&mut graph);
    assert_eq!(graph.edges[0].target, "stub");
    let mut graph = fixture(
        function("run", "app::run", "app"),
        "engine::api::save",
        vec![
            import(
                "private_export",
                "engine::api",
                "crate::commands::save",
                "save",
                false,
                false,
            ),
            function("save", "engine::commands::save", "engine::commands"),
        ],
    );
    resolve(&mut graph);
    assert_eq!(graph.edges[0].target, "stub");
}

#[test]
fn self_and_super_qualifiers_do_not_escape_the_crate_root() {
    for (callee, target) in [
        ("self::save", "local"),
        ("super::save", "parent"),
        ("crate::save", "root"),
    ] {
        let mut graph = fixture(
            function("run", "app::a::b::run", "app::a::b"),
            callee,
            vec![
                function("local", "app::a::b::save", "app::a::b"),
                function("parent", "app::a::save", "app::a"),
                function("root", "app::save", "app"),
            ],
        );
        resolve(&mut graph);
        assert_eq!(graph.edges[0].target, target, "{callee}");
    }
}

#[test]
fn block_local_import_is_visible_only_inside_its_recorded_span() {
    let mut binding = import("local", "app", "crate::actual::save", "save", false, false);
    binding
        .extra
        .insert("binding_scope".into(), json!("app::run"));
    binding.source_file = "run.rs".into();
    for (key, value) in [
        ("scope_start_line", 8),
        ("scope_start_column", 1),
        ("scope_end_line", 10),
        ("scope_end_column", 5),
    ] {
        binding.extra.insert(key.into(), json!(value));
    }
    let mut graph = fixture(
        function("run", "app::run", "app"),
        "save",
        vec![
            binding,
            function("actual", "app::actual::save", "app::actual"),
            function("module", "app::save", "app"),
        ],
    );
    resolve(&mut graph);
    assert_eq!(graph.edges[0].target, "actual");
    graph.edges[0].source_location = Some("L11".into());
    resolve(&mut graph);
    assert_eq!(graph.edges[0].target, "module");
}

#[test]
fn missing_local_scope_coordinates_do_not_become_a_module_level_guess() {
    let mut binding = import("local", "app", "crate::actual::save", "save", false, false);
    binding
        .extra
        .insert("binding_scope".into(), json!("app::run"));
    binding.source_file = "run.rs".into();
    let mut graph = fixture(
        function("run", "app::run", "app"),
        "save",
        vec![
            binding,
            function("actual", "app::actual::save", "app::actual"),
            function("module", "app::save", "app"),
        ],
    );
    resolve(&mut graph);
    assert_eq!(resolution(&graph), "ambiguous");
    assert_eq!(
        graph.edges[0].extra.get("resolution_truncated"),
        Some(&Value::Bool(true))
    );
}
