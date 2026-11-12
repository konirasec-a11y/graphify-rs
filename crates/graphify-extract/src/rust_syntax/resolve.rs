//! Conservative, syntax-backed Rust namespace resolution.
//!
//! Resolves only observed call expressions. Imports never manufacture calls, and
//! receiver methods require compiler type information, so remain unresolved.
use std::collections::BTreeSet;

use graphify_core::{
    confidence::Confidence,
    model::{ExtractionResult, GraphNode},
};
use serde_json::{Value, json};

#[path = "resolve_imports.rs"]
mod imports;

#[path = "resolve_index.rs"]
mod index;
use index::Index;

const MAX_EXPANSIONS: usize = 512;
const MAX_ALIAS_DEPTH: usize = 64;

struct Context<'a> {
    module: &'a str,
    krate: &'a str,
    scope: &'a str,
    position: Option<(u64, u64)>,
    source_file: &'a str,
}

#[derive(Default)]
struct Matches {
    ids: BTreeSet<String>,
    alias: bool,
    truncated: bool,
    occupied: bool,
    trait_dispatch: bool,
    ambiguous_crate: bool,
}

fn text<'a>(node: &'a GraphNode, key: &str) -> &'a str {
    node.extra.get(key).and_then(Value::as_str).unwrap_or("")
}

fn is_rust(node: &GraphNode) -> bool {
    text(node, "language") == "rust"
}

fn join(prefix: &str, suffix: &str) -> String {
    if suffix.is_empty() {
        prefix.to_owned()
    } else if prefix.is_empty() {
        suffix.to_owned()
    } else {
        format!("{prefix}::{suffix}")
    }
}

fn first_segment(path: &str) -> &str {
    path.split("::").next().unwrap_or("")
}

fn position(node: &GraphNode, prefix: &str) -> Option<(u64, u64)> {
    Some((
        node.extra.get(&format!("{prefix}_line"))?.as_u64()?,
        node.extra.get(&format!("{prefix}_column"))?.as_u64()?,
    ))
}

fn call_position(edge: &graphify_core::model::GraphEdge) -> Option<(u64, u64)> {
    Some((
        edge.source_location
            .as_deref()?
            .strip_prefix('L')?
            .parse()
            .ok()?,
        edge.extra.get("source_column")?.as_u64()?,
    ))
}

/// Resolve Rust calls over the complete project extraction. Safe to repeat after
/// per-file cached extraction: each edge retains its original unresolved target.
pub(super) fn resolve(result: &mut ExtractionResult) {
    super::path_overrides::apply(result);
    let index = Index::new(result);
    for edge in &mut result.edges {
        if edge.extra.get("language").and_then(Value::as_str) != Some("rust") {
            continue;
        }
        if !matches!(edge.relation.as_str(), "calls" | "imports") {
            continue;
        }
        if edge.relation == "calls" {
            edge.extra
                .insert("syntax_confidence".into(), json!("EXTRACTED"));
            edge.extra
                .insert("target_confidence".into(), json!("UNRESOLVED"));
            edge.extra
                .insert("confidence_scope".into(), json!("observed-callsite"));
        }
        let unsupported_layout = index
            .nodes
            .get(&edge.source)
            .is_some_and(|node| text(node, "module_layout") == "quarantined-path-override");
        if unsupported_layout
            || edge.extra.get("resolution_disabled") == Some(&Value::Bool(true))
            || index.nodes.get(&edge.source).is_some_and(|node| {
                node.extra.get("resolution_disabled") == Some(&Value::Bool(true))
            })
        {
            edge.extra.remove("candidate_ids");
            edge.extra.remove("resolution_truncated");
            edge.extra.remove("resolution_ambiguity");
            if let Some(original) = edge.extra.get("unresolved_target").and_then(Value::as_str) {
                edge.target = original.to_owned();
            }
            edge.confidence = Confidence::Extracted;
            edge.confidence_score = Confidence::Extracted.default_score();
            edge.extra.insert(
                "resolution".into(),
                json!(if unsupported_layout {
                    "unsupported-module-layout"
                } else {
                    "unsupported-target"
                }),
            );
            continue;
        }
        if edge.relation == "imports" {
            imports::resolve(edge, &index);
            continue;
        }
        if edge.relation != "calls" {
            continue;
        }
        let fallback = edge
            .extra
            .entry("unresolved_target".to_owned())
            .or_insert_with(|| json!(edge.target))
            .as_str()
            .unwrap_or(&edge.target)
            .to_owned();
        edge.target = fallback;
        edge.extra.remove("candidate_ids");
        edge.extra.remove("resolution_truncated");
        edge.extra.remove("resolution_ambiguity");
        edge.extra
            .insert("direction".into(), json!("source_to_target"));
        let kind = edge
            .extra
            .get("call_kind")
            .and_then(Value::as_str)
            .unwrap_or("dynamic");
        let shadowed = edge.extra.get("shadowed") == Some(&Value::Bool(true));
        if kind != "path" || shadowed {
            edge.confidence = Confidence::Extracted;
            edge.confidence_score = Confidence::Extracted.default_score();
            edge.extra.insert(
                "resolution".into(),
                json!(if shadowed {
                    "shadowed"
                } else if kind == "method" {
                    "method-unresolved"
                } else if kind == "macro" {
                    "macro-unexpanded"
                } else {
                    "dynamic-unresolved"
                }),
            );
            continue;
        }
        let Some(caller) = index.nodes.get(&edge.source) else {
            continue;
        };
        let module = edge
            .extra
            .get("module_path")
            .and_then(Value::as_str)
            .unwrap_or_else(|| text(caller, "module_path"))
            .to_owned();
        let krate = edge
            .extra
            .get("crate_name")
            .and_then(Value::as_str)
            .unwrap_or_else(|| text(caller, "crate_name"))
            .to_owned();
        let scope = edge
            .extra
            .get("lexical_scope")
            .and_then(Value::as_str)
            .unwrap_or_else(|| text(caller, "qualified_name"))
            .to_owned();
        let raw = edge
            .extra
            .get("callee")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_owned();
        let call_at = call_position(edge);
        let mut lookups = Vec::new();
        if let Some(paths) = edge.extra.get("lookup_paths").and_then(Value::as_array) {
            lookups.extend(paths.iter().filter_map(Value::as_str).map(str::to_owned));
        }
        let explicit_candidates = !lookups.is_empty();
        if lookups.is_empty() {
            let first = first_segment(raw.trim_start_matches("::"));
            let no_aliases = Value::Null;
            let aliases = edge.extra.get("crate_aliases").unwrap_or(&no_aliases);
            let external_alias = aliases.get(first).and_then(Value::as_str).is_some();
            let local_name = index.local_name(caller, &scope, &module, first);
            if raw.starts_with("::")
                || ["crate", "self", "super"].contains(&first)
                || (raw.contains("::")
                    && !local_name
                    && (index.crates.contains(first) || external_alias))
            {
                if let Some(canonical) = index.canonical_alias(&raw, &module, &krate, aliases) {
                    lookups.push(canonical);
                }
            } else {
                lookups.extend(
                    index
                        .lexical_prefixes(caller, &scope, &module)
                        .iter()
                        .map(|prefix| join(prefix, &raw)),
                );
            }
        }
        let mut resolution = Matches::default();
        for path in lookups {
            let candidate = index.expand(
                &path,
                &Context {
                    module: &module,
                    krate: &krate,
                    scope: &scope,
                    position: call_at,
                    source_file: &edge.source_file,
                },
                true,
            );
            if explicit_candidates {
                resolution.ids.extend(candidate.ids);
                resolution.alias |= candidate.alias;
                resolution.truncated |= candidate.truncated;
                resolution.occupied |= candidate.occupied;
                resolution.trait_dispatch |= candidate.trait_dispatch;
                resolution.ambiguous_crate |= candidate.ambiguous_crate;
            } else if candidate.occupied || !candidate.ids.is_empty() || candidate.truncated {
                resolution = candidate;
                break;
            }
        }
        edge.extra.insert(
            "resolution_basis".into(),
            json!("syntax-and-imports; no compiler type resolution"),
        );
        if resolution.truncated || resolution.ids.len() > 1 || resolution.ambiguous_crate {
            edge.confidence = Confidence::Ambiguous;
            edge.confidence_score = Confidence::Ambiguous.default_score();
            edge.extra.insert("resolution".into(), json!("ambiguous"));
            edge.extra
                .insert("target_confidence".into(), json!("AMBIGUOUS"));
            edge.extra
                .insert("confidence_scope".into(), json!("target-candidate"));
            edge.extra
                .insert("candidate_ids".into(), json!(resolution.ids));
            if resolution.truncated {
                edge.extra
                    .insert("resolution_truncated".into(), json!(true));
            }
            if resolution.ambiguous_crate {
                edge.extra.insert(
                    "resolution_ambiguity".into(),
                    json!("crate-name-shared-by-packages"),
                );
            }
        } else if let Some(target) = resolution.ids.into_iter().next() {
            edge.target = target;
            edge.confidence = Confidence::Inferred;
            edge.confidence_score = 0.9;
            edge.extra
                .insert("target_confidence".into(), json!("INFERRED"));
            edge.extra
                .insert("confidence_scope".into(), json!("target-candidate"));
            edge.extra.insert(
                "resolution".into(),
                json!(if resolution.alias {
                    "import-alias"
                } else if raw.contains("::") {
                    "qualified-path"
                } else {
                    "lexical"
                }),
            );
        } else {
            edge.confidence = Confidence::Extracted;
            edge.confidence_score = Confidence::Extracted.default_score();
            edge.extra.insert(
                "resolution".into(),
                json!(if resolution.trait_dispatch {
                    "trait-dispatch-unresolved"
                } else {
                    "unresolved"
                }),
            );
        }
    }
}

#[cfg(test)]
#[path = "resolve_tests.rs"]
mod tests;

#[cfg(test)]
#[path = "resolve_additional_tests.rs"]
mod additional_tests;
