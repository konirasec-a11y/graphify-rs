//! Rust syntax evidence with scoped identities and explicit unresolved call sites.
//!
//! This is not rustc name/type resolution. Lexical targets remain INFERRED;
//! method dispatch, macro expansion and cfg selection are deliberately unresolved.
mod extract;
mod layout;
mod path_overrides;
mod resolve;
#[cfg(test)]
mod tests;

use graphify_core::model::ExtractionResult;
use std::path::Path;

pub(crate) fn extract(path: &Path, source: &str) -> ExtractionResult {
    match syn::parse_file(source) {
        Ok(syntax) => {
            let mut result = extract::extract(path, &syntax);
            resolve::resolve(&mut result);
            result
        }
        Err(error) => {
            let file = graphify_core::model::GraphNode {
                id: extract::rust_id(&[&path.to_string_lossy()]),
                label: path
                    .file_stem()
                    .unwrap_or_default()
                    .to_string_lossy()
                    .into_owned(),
                source_file: path.to_string_lossy().into_owned(),
                source_location: None,
                node_type: graphify_core::model::NodeType::File,
                community: None,
                extra: std::collections::HashMap::from([
                    ("language".into(), serde_json::json!("rust")),
                    ("item_kind".into(), serde_json::json!("file")),
                    (
                        "extraction_error".into(),
                        serde_json::json!(error.to_string()),
                    ),
                ]),
            };
            ExtractionResult {
                nodes: vec![file],
                ..Default::default()
            }
        }
    }
}

/// Resolve syntax-backed Rust call sites after all files have been assembled.
/// Repeating the pass replaces targets from their original call-site stub.
pub fn resolve_project(result: &mut ExtractionResult) {
    resolve::resolve(result);
}
