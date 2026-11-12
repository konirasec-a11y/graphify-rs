use super::{
    resolve,
    tests::{fixture, function, import, resolution},
};
use serde_json::json;

#[test]
fn named_type_imports_retarget_the_definition_without_manufacturing_calls() {
    let binding = import(
        "binding",
        "app",
        "engine::dto::Record",
        "Record",
        false,
        false,
    );
    let mut record = function("record", "engine::dto::Record", "engine::dto");
    record.node_type = graphify_core::model::NodeType::Struct;
    record.extra.insert("item_kind".into(), json!("struct"));
    let mut graph = fixture(
        function("run", "app::run", "app"),
        "ignored",
        vec![binding, record],
    );
    graph.edges[0].relation = "imports".into();
    graph.edges[0].target = "binding".into();
    graph.edges[0].extra.remove("unresolved_target");
    resolve(&mut graph);
    assert_eq!(graph.edges[0].target, "record");
    assert_eq!(resolution(&graph), "import-symbol");
    assert_eq!(graph.edges.len(), 1);
    assert_eq!(graph.edges[0].relation, "imports");
    let before = serde_json::to_value(&graph).unwrap();
    resolve(&mut graph);
    assert_eq!(serde_json::to_value(&graph).unwrap(), before);
}

#[test]
fn cargo_dependency_aliases_resolve_but_a_local_namespace_takes_precedence() {
    let mut graph = fixture(
        function("run", "app::run", "app"),
        "renamed::api::save",
        vec![function("external", "engine::api::save", "engine::api")],
    );
    graph.edges[0]
        .extra
        .insert("crate_aliases".into(), json!({"renamed":"engine"}));
    resolve(&mut graph);
    assert_eq!(graph.edges[0].target, "external");
    graph.nodes.push(function(
        "local",
        "app::renamed::api::save",
        "app::renamed::api",
    ));
    resolve(&mut graph);
    assert_eq!(graph.edges[0].target, "local");
}

#[test]
fn innermost_use_binding_is_hoisted_and_non_callable_bindings_still_shadow() {
    for constructor in [false, true] {
        let mut outer = import(
            "outer",
            "app",
            "crate::outer_target",
            "helper",
            false,
            false,
        );
        let mut inner = import(
            "inner",
            "app",
            "crate::inner_target",
            "helper",
            false,
            false,
        );
        for (binding, start, end) in [(&mut outer, 1, 30), (&mut inner, 10, 20)] {
            binding.source_file = "run.rs".into();
            binding
                .extra
                .insert("binding_scope".into(), json!("app::run"));
            for (key, value) in [
                ("scope_start_line", start),
                ("scope_start_column", 1),
                ("scope_end_line", end),
                ("scope_end_column", 1),
            ] {
                binding.extra.insert(key.into(), json!(value));
            }
        }
        inner.source_location = Some("L18".into());
        let mut target = function("inner_target", "app::inner_target", "app");
        if constructor {
            target.node_type = graphify_core::model::NodeType::Struct;
            target.extra.insert("item_kind".into(), json!("struct"));
        }
        let mut graph = fixture(
            function("run", "app::run", "app"),
            "helper",
            vec![
                outer,
                inner,
                target,
                function("outer_target", "app::outer_target", "app"),
            ],
        );
        for (line, expected) in [
            (5, "outer_target"),
            (15, if constructor { "stub" } else { "inner_target" }),
            (25, "outer_target"),
        ] {
            graph.edges[0].source_location = Some(format!("L{line}"));
            resolve(&mut graph);
            assert_eq!(
                graph.edges[0].target, expected,
                "constructor={constructor}, line={line}"
            );
        }
    }
}

#[test]
fn local_use_bindings_from_a_cfg_homonym_in_another_file_do_not_leak() {
    let mut binding = import("other_file", "app", "crate::target", "helper", false, false);
    binding
        .extra
        .insert("binding_scope".into(), json!("app::run"));
    binding.extra.extend([
        ("scope_start_line".into(), json!(1)),
        ("scope_start_column".into(), json!(1)),
        ("scope_end_line".into(), json!(20)),
        ("scope_end_column".into(), json!(1)),
    ]);
    let mut graph = fixture(
        function("run", "app::run", "app"),
        "helper",
        vec![
            binding,
            function("target", "app::target", "app"),
            function("local", "app::helper", "app"),
        ],
    );
    resolve(&mut graph);
    assert_eq!(graph.edges[0].target, "local");
}

#[test]
fn a_dependency_package_does_not_bind_to_an_unrelated_package_with_the_same_lib_name() {
    let mut frontend = function("button", "ui::Button::new", "ui");
    frontend
        .extra
        .insert("package_name".into(), json!("bellatrix_ui"));
    let mut graph = fixture(
        function("run", "host::run", "host"),
        "zed_ui::Button::new",
        vec![frontend],
    );
    graph.edges[0]
        .extra
        .insert("crate_aliases".into(), json!({"zed_ui":"ui"}));
    resolve(&mut graph);
    assert_eq!(graph.edges[0].target, "stub");
    graph.edges[0]
        .extra
        .insert("callee".into(), json!("frontend::Button::new"));
    graph.edges[0]
        .extra
        .insert("crate_aliases".into(), json!({"frontend":"bellatrix_ui"}));
    resolve(&mut graph);
    assert_eq!(graph.edges[0].target, "button");
}

#[test]
fn shared_crate_names_across_packages_are_not_treated_as_one_namespace() {
    let mut frontend = function("button", "ui::Button::new", "ui");
    frontend
        .extra
        .insert("package_name".into(), json!("bellatrix_ui"));
    let mut vendor = function("vendor", "ui::Other::new", "ui");
    vendor.extra.insert("package_name".into(), json!("ui"));
    let mut graph = fixture(
        function("run", "host::run", "host"),
        "frontend::Button::new",
        vec![frontend, vendor],
    );
    graph.edges[0]
        .extra
        .insert("crate_aliases".into(), json!({"frontend":"bellatrix_ui"}));
    resolve(&mut graph);
    assert_eq!(graph.edges[0].target, "stub");
    assert_eq!(resolution(&graph), "ambiguous");
    assert_eq!(
        graph.edges[0].extra["resolution_ambiguity"],
        "crate-name-shared-by-packages"
    );
}

#[test]
fn unsupported_target_disables_a_previously_resolved_call() {
    let mut graph = fixture(
        function("run", "app::run", "app"),
        "save",
        vec![function("save", "app::save", "app")],
    );
    resolve(&mut graph);
    assert_eq!(graph.edges[0].target, "save");
    graph.edges[0]
        .extra
        .insert("resolution_disabled".into(), json!(true));
    resolve(&mut graph);
    assert_eq!(graph.edges[0].target, "stub");
    assert_eq!(resolution(&graph), "unsupported-target");
}
