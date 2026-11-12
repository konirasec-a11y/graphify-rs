//! Graph assembly and deduplication for graphify.
//!
//! Takes [`ExtractionResult`]s from multiple files and assembles them into a
//! single [`KnowledgeGraph`], skipping dangling edges.

mod codegraph_merge;

pub use codegraph_merge::merge_codegraph_edges;

use tracing::debug;

use graphify_core::error::Result;
use graphify_core::graph::KnowledgeGraph;
use graphify_core::model::ExtractionResult;

/// Build a [`KnowledgeGraph`] from a single extraction result.
///
/// All nodes are added first; edges that reference unknown source/target
/// nodes are silently skipped (dangling-edge protection).
pub fn build_from_extraction(extraction: &ExtractionResult) -> Result<KnowledgeGraph> {
    build(std::slice::from_ref(extraction))
}

/// Merge borrowed extraction results, keeping the first node for each ID.
///
/// Nodes and edges are copied only into the final graph; no combined extraction
/// is materialized. Use [`build_owned`] when the input is no longer needed.
pub fn build(extractions: &[ExtractionResult]) -> Result<KnowledgeGraph> {
    let mut graph = KnowledgeGraph::new();
    for node in extractions.iter().flat_map(|result| &result.nodes) {
        let _ = graph.add_node(node.clone());
    }
    append_edges(
        &mut graph,
        extractions
            .iter()
            .flat_map(|result| result.edges.iter().cloned()),
    );
    graph.set_hyperedges(
        extractions
            .iter()
            .flat_map(|result| result.hyperedges.iter().cloned())
            .collect(),
    );
    Ok(graph)
}

/// Assemble a graph by moving extraction payloads instead of cloning them.
///
/// All nodes are inserted before any edge, so cross-file references, duplicate
/// node precedence, parallel edges and dangling-edge handling match [`build`].
/// The CLI uses this path to keep large syntax metadata in a single owner.
pub fn build_owned(extractions: Vec<ExtractionResult>) -> Result<KnowledgeGraph> {
    let edge_count = extractions.iter().map(|result| result.edges.len()).sum();
    let mut edges = Vec::with_capacity(edge_count);
    let mut hyperedges = Vec::new();
    let mut graph = KnowledgeGraph::new();
    for result in extractions {
        for node in result.nodes {
            let _ = graph.add_node(node);
        }
        edges.extend(result.edges);
        hyperedges.extend(result.hyperedges);
    }
    append_edges(&mut graph, edges);
    graph.set_hyperedges(hyperedges);
    Ok(graph)
}

fn append_edges(
    graph: &mut KnowledgeGraph,
    edges: impl IntoIterator<Item = graphify_core::model::GraphEdge>,
) {
    let mut skipped = 0usize;
    for edge in edges {
        if graph.add_edge(edge).is_err() {
            skipped += 1;
        }
    }
    if skipped > 0 {
        debug!("skipped {skipped} dangling edge(s)");
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use graphify_core::confidence::Confidence;
    use graphify_core::model::{GraphEdge, GraphNode, Hyperedge, NodeType};
    use std::collections::HashMap;

    fn make_node(id: &str) -> GraphNode {
        GraphNode {
            id: id.into(),
            label: id.into(),
            source_file: "test.rs".into(),
            source_location: None,
            node_type: NodeType::Class,
            community: None,
            extra: HashMap::new(),
        }
    }

    fn make_edge(src: &str, tgt: &str) -> GraphEdge {
        GraphEdge {
            source: src.into(),
            target: tgt.into(),
            relation: "calls".into(),
            confidence: Confidence::Extracted,
            confidence_score: 1.0,
            source_file: "test.rs".into(),
            source_location: None,
            weight: 1.0,
            provenance: None,
            extra: HashMap::new(),
        }
    }

    #[test]
    fn build_from_empty() {
        let ext = ExtractionResult::default();
        let graph = build_from_extraction(&ext).unwrap();
        assert_eq!(graph.node_count(), 0);
        assert_eq!(graph.edge_count(), 0);
    }

    #[test]
    fn build_with_nodes_and_edges() {
        let ext = ExtractionResult {
            nodes: vec![make_node("a"), make_node("b"), make_node("c")],
            edges: vec![make_edge("a", "b"), make_edge("b", "c")],
            hyperedges: vec![],
        };
        let graph = build_from_extraction(&ext).unwrap();
        assert_eq!(graph.node_count(), 3);
        assert_eq!(graph.edge_count(), 2);
        assert!(graph.get_node("a").is_some());
        assert!(graph.get_node("b").is_some());
        assert!(graph.get_node("c").is_some());
    }

    #[test]
    fn dangling_edges_skipped() {
        let ext = ExtractionResult {
            nodes: vec![make_node("a"), make_node("b")],
            edges: vec![
                make_edge("a", "b"),       // valid
                make_edge("a", "missing"), // dangling
                make_edge("gone", "b"),    // dangling
            ],
            hyperedges: vec![],
        };
        let graph = build_from_extraction(&ext).unwrap();
        assert_eq!(graph.node_count(), 2);
        assert_eq!(graph.edge_count(), 1); // only a->b
    }

    #[test]
    fn build_merges_multiple_extractions() {
        let ext1 = ExtractionResult {
            nodes: vec![make_node("a"), make_node("b")],
            edges: vec![make_edge("a", "b")],
            hyperedges: vec![],
        };
        let ext2 = ExtractionResult {
            nodes: vec![make_node("c")],
            edges: vec![make_edge("b", "c")],
            hyperedges: vec![],
        };
        let graph = build(&[ext1, ext2]).unwrap();
        assert_eq!(graph.node_count(), 3);
        assert_eq!(graph.edge_count(), 2);
    }

    #[test]
    fn duplicate_nodes_first_wins() {
        let ext = ExtractionResult {
            nodes: vec![make_node("a"), make_node("a")],
            edges: vec![],
            hyperedges: vec![],
        };
        let graph = build_from_extraction(&ext).unwrap();
        assert_eq!(graph.node_count(), 1);
    }

    #[test]
    fn hyperedges_stored() {
        let ext = ExtractionResult {
            nodes: vec![make_node("a"), make_node("b")],
            edges: vec![],
            hyperedges: vec![Hyperedge {
                nodes: vec!["a".into(), "b".into()],
                relation: "coexist".into(),
                label: "together".into(),
            }],
        };
        let graph = build_from_extraction(&ext).unwrap();
        assert_eq!(graph.hyperedges.len(), 1);
        assert_eq!(graph.hyperedges[0].relation, "coexist");
    }

    #[test]
    fn owned_assembly_preserves_evidence_parallel_calls_and_first_node_precedence() {
        let mut first = make_node("a");
        first.label = "first definition".into();
        first
            .extra
            .insert("qualified_name".into(), serde_json::json!("app::a"));
        let mut duplicate = make_node("a");
        duplicate.label = "later duplicate".into();
        let mut call = make_edge("a", "b");
        call.source_location = Some("L12".into());
        call.extra
            .insert("direction".into(), serde_json::json!("source_to_target"));
        call.extra
            .insert("candidate_ids".into(), serde_json::json!(["b", "c"]));
        let mut repeated = call.clone();
        repeated.source_location = Some("L13".into());
        let extractions = vec![
            ExtractionResult {
                nodes: vec![first],
                edges: vec![call, repeated, make_edge("a", "missing")],
                hyperedges: vec![Hyperedge {
                    nodes: vec!["a".into(), "b".into()],
                    relation: "group".into(),
                    label: "evidence".into(),
                }],
            },
            ExtractionResult {
                nodes: vec![duplicate, make_node("b")],
                edges: vec![make_edge("b", "a")],
                hyperedges: vec![],
            },
        ];
        let borrowed = build(&extractions).unwrap().to_node_link_json();
        let owned = build_owned(extractions).unwrap();
        assert_eq!(owned.to_node_link_json(), borrowed);
        assert_eq!(owned.node_count(), 2);
        assert_eq!(owned.edge_count(), 3);
        assert_eq!(owned.get_node("a").unwrap().label, "first definition");
    }
}
