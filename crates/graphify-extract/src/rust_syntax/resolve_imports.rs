//! Named imports resolve to source definitions, never to invented call edges.
use super::{Context, Index, call_position, text};
use graphify_core::{confidence::Confidence, model::GraphEdge};
use serde_json::{Value, json};

pub(super) fn resolve(edge: &mut GraphEdge, index: &Index) {
    let original = edge.target.clone();
    let stub = edge
        .extra
        .entry("unresolved_target".into())
        .or_insert_with(|| json!(original))
        .as_str()
        .unwrap_or(&original)
        .to_owned();
    edge.target = stub.clone();
    edge.extra.remove("candidate_ids");
    edge.extra.remove("resolution_truncated");
    edge.extra.remove("resolution_ambiguity");
    let Some(import) = index.nodes.get(&stub) else {
        return;
    };
    edge.extra
        .insert("direction".into(), json!("source_to_target"));
    edge.extra.insert(
        "resolution_basis".into(),
        json!("syntax-and-imports; no compiler type resolution"),
    );
    edge.confidence = Confidence::Extracted;
    edge.confidence_score = Confidence::Extracted.default_score();
    edge.extra
        .insert("resolution".into(), json!("import-unresolved"));
    if import.extra.get("import_glob") == Some(&Value::Bool(true)) {
        edge.extra
            .insert("resolution".into(), json!("glob-unexpanded"));
        return;
    }
    let module = text(import, "module_path");
    let krate = text(import, "crate_name");
    let scope = import
        .extra
        .get("binding_scope")
        .and_then(Value::as_str)
        .unwrap_or(module);
    let no_aliases = Value::Null;
    let aliases = import.extra.get("crate_aliases").unwrap_or(&no_aliases);
    let Some(canonical) =
        index.canonical_alias(text(import, "import_path"), module, krate, aliases)
    else {
        return;
    };
    let found = index.expand(
        &canonical,
        &Context {
            module,
            krate,
            scope,
            position: call_position(edge),
            source_file: &edge.source_file,
        },
        false,
    );
    if found.ids.len() > 1 || found.truncated || found.ambiguous_crate {
        edge.confidence = Confidence::Ambiguous;
        edge.confidence_score = Confidence::Ambiguous.default_score();
        edge.extra.insert("resolution".into(), json!("ambiguous"));
        edge.extra.insert("candidate_ids".into(), json!(found.ids));
        if found.truncated {
            edge.extra
                .insert("resolution_truncated".into(), json!(true));
        }
        if found.ambiguous_crate {
            edge.extra.insert(
                "resolution_ambiguity".into(),
                json!("crate-name-shared-by-packages"),
            );
        }
    } else if let Some(id) = found.ids.into_iter().next() {
        edge.target = id;
        edge.confidence = Confidence::Inferred;
        edge.confidence_score = 0.9;
        edge.extra
            .insert("resolution".into(), json!("import-symbol"));
    }
}
