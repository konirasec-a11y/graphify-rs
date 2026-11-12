//! Quarantine remapped module trees without disabling unrelated production code.
use graphify_core::model::ExtractionResult;
use serde_json::{Value, json};
use std::{
    collections::{BTreeMap, BTreeSet},
    path::{Component, Path, PathBuf},
};

fn text<'a>(node: &'a graphify_core::model::GraphNode, key: &str) -> &'a str {
    node.extra.get(key).and_then(Value::as_str).unwrap_or("")
}
fn normalized(path: &Path) -> PathBuf {
    if let Ok(path) = path.canonicalize() {
        return path;
    }
    let path = if path.is_absolute() {
        path.to_owned()
    } else {
        std::env::current_dir().unwrap_or_default().join(path)
    };
    let mut result = PathBuf::new();
    for component in path.components() {
        match component {
            Component::CurDir => {}
            Component::ParentDir => {
                result.pop();
            }
            component => result.push(component.as_os_str()),
        }
    }
    result
}

/// Mark only declared namespaces and their physical source trees as unsupported.
/// No filename-derived namespace from a remapped file may become a real target.
pub(super) fn apply(result: &mut ExtractionResult) {
    let files: BTreeMap<_, _> = result
        .nodes
        .iter()
        .filter(|node| text(node, "language") == "rust" && text(node, "item_kind") == "file")
        .map(|node| {
            (
                node.source_file.clone(),
                (
                    normalized(Path::new(&node.source_file)),
                    text(node, "module_path").to_owned(),
                    text(node, "package_name").to_owned(),
                ),
            )
        })
        .collect();
    let mut affected_files = BTreeSet::new();
    let mut namespaces = BTreeSet::new();
    for node in &result.nodes {
        let Some(override_path) = node
            .extra
            .get("module_path_override")
            .and_then(Value::as_str)
        else {
            continue;
        };
        let Some((source, file_module, package)) = files.get(&node.source_file) else {
            continue;
        };
        let declared = text(node, "module_path");
        namespaces.insert((package.clone(), declared.to_owned()));
        let mut directory = source.parent().unwrap_or(Path::new("")).to_owned();
        let inline = declared
            .strip_prefix(&format!("{file_module}::"))
            .and_then(|suffix| suffix.rsplit_once("::").map(|(parent, _)| parent));
        if let Some(inline) = inline {
            if !matches!(
                source.file_stem().and_then(|stem| stem.to_str()),
                Some("mod" | "lib" | "main")
            ) {
                directory.push(source.file_stem().unwrap_or_default());
            }
            directory.extend(inline.split("::"));
        }
        let target = normalized(&directory.join(override_path));
        let mut trees = vec![];
        if target.file_stem().is_some_and(|stem| stem == "mod") {
            trees.push(target.parent().unwrap_or(Path::new("")).to_owned());
        } else if let Some(parent) = target.parent() {
            trees.push(parent.join(target.file_stem().unwrap_or_default()));
            trees.push(parent.join(declared.rsplit("::").next().unwrap_or("")));
        }
        for (spelling, (path, _, file_package)) in &files {
            if file_package == package
                && (path == &target || trees.iter().any(|tree| path.starts_with(tree)))
            {
                affected_files.insert(spelling.clone());
            }
        }
    }
    for node in &mut result.nodes {
        if text(node, "language") != "rust" {
            continue;
        }
        node.extra.remove("module_layout");
        let module = text(node, "module_path");
        let package = text(node, "package_name");
        let affected_namespace = namespaces.iter().any(|(owner, namespace)| {
            package == owner
                && !namespace.is_empty()
                && (module == namespace || module.starts_with(&format!("{namespace}::")))
        });
        if affected_files.contains(&node.source_file) || affected_namespace {
            node.extra
                .insert("module_layout".into(), json!("quarantined-path-override"));
        }
    }
}
