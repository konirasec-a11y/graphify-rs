//! Namespace/import index used only by Graphify's Rust resolver.
use super::{
    Context, MAX_ALIAS_DEPTH, MAX_EXPANSIONS, Matches, first_segment, is_rust, join, position, text,
};
use graphify_core::model::{ExtractionResult, GraphNode, NodeType};
use serde_json::Value;
use std::collections::{BTreeMap, BTreeSet, VecDeque};

#[derive(Clone)]
struct Definition {
    id: String,
    crate_name: String,
    callable: bool,
    public: bool,
    trait_method: bool,
}

#[derive(Clone)]
struct Import {
    path: String,
    module: String,
    alias: String,
    glob: bool,
    reexport: bool,
    scope: Option<((u64, u64), (u64, u64))>,
    aliases: Value,
    source_file: String,
}

pub(super) struct Index {
    names: BTreeMap<String, Vec<Definition>>,
    imports: BTreeMap<String, Vec<Import>>,
    pub(super) crates: BTreeSet<String>,
    pub(super) nodes: BTreeMap<String, GraphNode>,
    packages: BTreeMap<String, BTreeSet<String>>,
    pub(super) unsupported_layouts: BTreeSet<String>,
}

impl Index {
    pub(super) fn new(result: &ExtractionResult) -> Self {
        let mut index = Self {
            names: BTreeMap::new(),
            imports: BTreeMap::new(),
            crates: BTreeSet::new(),
            nodes: BTreeMap::new(),
            packages: BTreeMap::new(),
            unsupported_layouts: result
                .nodes
                .iter()
                .filter(|node| is_rust(node) && node.extra.contains_key("module_path_override"))
                .map(|node| text(node, "package_name").to_owned())
                .collect(),
        };
        for node in result.nodes.iter().filter(|node| is_rust(node)) {
            index.nodes.insert(node.id.clone(), node.clone());
            if node.extra.get("resolution_disabled") == Some(&Value::Bool(true))
                || index
                    .unsupported_layouts
                    .contains(text(node, "package_name"))
            {
                continue;
            }
            // A call/type placeholder is not evidence that its package or crate
            // is present. In particular it must not turn a custom lib name into
            // a second, invented package with that same name.
            if node.extra.get("unresolved") == Some(&Value::Bool(true))
                && text(node, "item_kind") != "import"
            {
                continue;
            }
            let krate = text(node, "crate_name");
            if !krate.is_empty() {
                index.crates.insert(krate.to_owned());
                let package = match text(node, "package_name") {
                    "" => krate,
                    package => package,
                };
                index
                    .packages
                    .entry(package.to_owned())
                    .or_default()
                    .insert(krate.to_owned());
            }
            if text(node, "item_kind") == "import" {
                index
                    .imports
                    .entry(
                        node.extra
                            .get("binding_scope")
                            .and_then(Value::as_str)
                            .unwrap_or_else(|| text(node, "module_path"))
                            .to_owned(),
                    )
                    .or_default()
                    .push(Import {
                        path: text(node, "import_path").to_owned(),
                        module: text(node, "module_path").to_owned(),
                        alias: text(node, "import_alias").to_owned(),
                        glob: node.extra.get("import_glob") == Some(&Value::Bool(true)),
                        reexport: node.extra.get("reexport") == Some(&Value::Bool(true)),
                        scope: position(node, "scope_start").zip(position(node, "scope_end")),
                        source_file: node.source_file.clone(),
                        aliases: node
                            .extra
                            .get("crate_aliases")
                            .cloned()
                            .unwrap_or(Value::Null),
                    });
                continue;
            }
            let name = text(node, "qualified_name");
            if name.is_empty() || node.extra.get("unresolved") == Some(&Value::Bool(true)) {
                continue;
            }
            index
                .names
                .entry(name.to_owned())
                .or_default()
                .push(Definition {
                    id: node.id.clone(),
                    crate_name: krate.to_owned(),
                    callable: matches!(node.node_type, NodeType::Function | NodeType::Method)
                        && node.extra.get("has_body") == Some(&Value::Bool(true))
                        && text(node, "item_kind") != "trait_method",
                    public: node.extra.get("is_public") == Some(&Value::Bool(true)),
                    trait_method: text(node, "item_kind") == "trait_method",
                });
        }
        index
    }

    fn canonical(&self, raw: &str, module: &str, krate: &str) -> Option<String> {
        let absolute = raw.trim().starts_with("::");
        let raw = raw.trim().trim_start_matches("::");
        if raw.is_empty() {
            return None;
        }
        let mut segments = raw.split("::").peekable();
        let first = segments.next()?;
        let suffix = segments.collect::<Vec<_>>().join("::");
        let prefix = match first {
            "crate" => krate.to_owned(),
            "self" => module.to_owned(),
            "super" => {
                let mut base = module.split("::").collect::<Vec<_>>();
                if base.len() <= 1 {
                    return None;
                }
                base.pop();
                let mut rest = suffix.as_str();
                while let Some(tail) = rest.strip_prefix("super::") {
                    if base.len() <= 1 {
                        return None;
                    }
                    base.pop();
                    rest = tail;
                }
                if rest == "super" {
                    if base.len() <= 1 {
                        return None;
                    }
                    base.pop();
                    rest = "";
                }
                return Some(join(&base.join("::"), rest));
            }
            external if self.crates.contains(external) => return Some(raw.to_owned()),
            _ if absolute => return Some(raw.to_owned()),
            _ => return Some(join(module, raw)),
        };
        Some(join(&prefix, &suffix))
    }

    fn namespace_exists(&self, path: &str) -> bool {
        self.names.contains_key(path)
            || self.imports.contains_key(path)
            || self
                .names
                .range(format!("{path}::")..)
                .next()
                .is_some_and(|(name, _)| name.starts_with(&format!("{path}::")))
    }

    pub(super) fn canonical_alias(
        &self,
        raw: &str,
        module: &str,
        krate: &str,
        aliases: &Value,
    ) -> Option<String> {
        let first = first_segment(raw.trim_start_matches("::"));
        if !raw.starts_with("::") && self.namespace_exists(&join(module, first)) {
            return Some(join(module, raw));
        }
        if let Some(actual) = aliases.get(first).and_then(Value::as_str) {
            if let Some(crates) = self.packages.get(actual).filter(|crates| crates.len() == 1) {
                let suffix = raw
                    .trim_start_matches("::")
                    .strip_prefix(first)
                    .unwrap_or("");
                return Some(format!("{}{suffix}", crates.first()?));
            }
            return None;
        }
        self.canonical(raw, module, krate)
    }

    /// Expand import bindings, keeping every surviving candidate. Bounds never
    /// silently turn a partial search into a unique resolution.
    pub(super) fn expand(&self, canonical: &str, context: &Context<'_>, call: bool) -> Matches {
        let caller_module = context.module;
        let caller_crate = context.krate;
        let caller_scope = context.scope;
        let call_position = context.position;
        let mut found = Matches::default();
        let mut queue = VecDeque::from([(canonical.to_owned(), 0usize, false)]);
        let mut visited = BTreeSet::new();
        while let Some((path, depth, aliased)) = queue.pop_front() {
            if !visited.insert(path.clone()) {
                continue;
            }
            if visited.len() > MAX_EXPANSIONS || depth > MAX_ALIAS_DEPTH {
                found.truncated = true;
                continue;
            }
            if let Some(definitions) = self.names.get(&path) {
                found.occupied = true;
                if self
                    .packages
                    .values()
                    .filter(|crates| crates.contains(first_segment(&path)))
                    .take(2)
                    .count()
                    > 1
                {
                    found.ambiguous_crate = true;
                    continue;
                }
                for definition in definitions {
                    found.trait_dispatch |= definition.trait_method;
                    if (!call || definition.callable)
                        && (definition.crate_name == caller_crate || definition.public)
                    {
                        found.ids.insert(definition.id.clone());
                        found.alias |= aliased;
                    }
                }
                // A concrete item shadows glob imports, including a constructor
                // or signature for which this analysis has no executable body.
                continue;
            }
            for (module, imports) in self.import_prefixes(&path) {
                let suffix = path.strip_prefix(&format!("{module}::")).unwrap_or("");
                // A qualified local module/item is not a glob lookup. Reusing
                // a glob for its own target invents `left::left::...` paths.
                if self.namespace_exists(&join(module, first_segment(suffix))) {
                    continue;
                }
                let mut eligible = Vec::new();
                for import in imports {
                    let named_tail = if suffix == import.alias {
                        Some("")
                    } else {
                        suffix.strip_prefix(&format!("{}::", import.alias))
                    };
                    if !import.glob && named_tail.is_none() {
                        continue;
                    }
                    let local = module != import.module;
                    if local && import.source_file != context.source_file {
                        continue;
                    }
                    let visible_from = if local { caller_scope } else { caller_module };
                    if !import.reexport
                        && visible_from != module
                        && !visible_from.starts_with(&format!("{module}::"))
                    {
                        continue;
                    }
                    if local {
                        match (import.scope, call_position) {
                            (Some((start, end)), Some(at)) if at >= start && at < end => {}
                            (Some(_), Some(_)) => continue,
                            _ => {
                                if import.glob
                                    || suffix == import.alias
                                    || suffix.starts_with(&format!("{}::", import.alias))
                                {
                                    found.truncated = true;
                                }
                                continue;
                            }
                        }
                    }
                    let Some(mut target) = self.canonical_alias(
                        &import.path,
                        &import.module,
                        first_segment(&import.module),
                        &import.aliases,
                    ) else {
                        found.occupied |= !import.glob;
                        continue;
                    };
                    if local
                        && !import.path.starts_with("::")
                        && !["crate", "self", "super"].contains(&first_segment(&import.path))
                        && !self.crates.contains(first_segment(&import.path))
                    {
                        let local_target = join(module, &import.path);
                        if self.namespace_exists(&local_target) {
                            target = local_target;
                        }
                    }
                    let target = if import.glob {
                        join(&target, suffix)
                    } else {
                        join(&target, named_tail.unwrap_or(""))
                    };
                    eligible.push((import, target, local));
                }
                // Local use items are hoisted within their block. An inner
                // binding shadows the outer one even when it names a constructor
                // or an external item whose body is unavailable to this index.
                let innermost = eligible
                    .iter()
                    .filter_map(|(item, _, local)| if *local { item.scope } else { None })
                    .max_by(|a, b| a.0.cmp(&b.0).then_with(|| b.1.cmp(&a.1)));
                eligible.retain(|(item, _, local)| !*local || item.scope == innermost);
                let named = eligible.iter().any(|(item, _, _)| !item.glob);
                for (item, target, local) in eligible {
                    if named && item.glob {
                        continue;
                    }
                    found.occupied |= local || !item.glob;
                    queue.push_back((target, depth + 1, true));
                }
            }
        }
        found
    }

    fn import_prefixes<'a>(&'a self, path: &'a str) -> Vec<(&'a str, &'a [Import])> {
        let mut result = Vec::new();
        for (position, _) in path.match_indices("::") {
            let module = &path[..position];
            if let Some(imports) = self.imports.get(module) {
                result.push((module, imports.as_slice()));
            }
        }
        result
    }

    pub(super) fn lexical_prefixes(
        &self,
        caller: &GraphNode,
        scope: &str,
        module: &str,
    ) -> Vec<String> {
        let mut prefixes = Vec::new();
        if !scope.is_empty() {
            prefixes.push(scope.to_owned());
        }
        let mut parent = text(caller, "lexical_parent");
        let mut seen = BTreeSet::new();
        while !parent.is_empty() && seen.insert(parent.to_owned()) {
            let Some(owner) = self.nodes.get(parent) else {
                break;
            };
            let name = text(owner, "qualified_name");
            if !name.is_empty() && !prefixes.iter().any(|prefix| prefix == name) {
                prefixes.push(name.to_owned());
            }
            parent = text(owner, "lexical_parent");
        }
        if !module.is_empty() && !prefixes.iter().any(|prefix| prefix == module) {
            prefixes.push(module.to_owned());
        }
        prefixes
    }

    pub(super) fn local_name(
        &self,
        caller: &GraphNode,
        scope: &str,
        module: &str,
        first: &str,
    ) -> bool {
        self.lexical_prefixes(caller, scope, module)
            .iter()
            .any(|prefix| self.namespace_exists(&join(prefix, first)))
            || self.imports.get(module).is_some_and(|bindings| {
                bindings
                    .iter()
                    .any(|binding| !binding.glob && binding.alias == first)
            })
    }
}
