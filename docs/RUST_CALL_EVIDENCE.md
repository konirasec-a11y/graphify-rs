# Rust call evidence

The Rust extractor uses `syn` to record syntax ownership, declarations, imports
and actual call expressions. It does not execute code or replace Rust compiler
name/type resolution.

- Function IDs include file, namespace, owner and source position. Two impls,
  inline modules, nested functions or cfg alternatives do not overwrite each other.
- Calls retain their line and column, including recursion and repeated calls to
  the same target. Closures and async blocks own their deferred bodies.
- A `calls` edge is created only from an observed call expression. Imports do not
  manufacture calls to every symbol in a file.
- Qualified paths, lexical function names, use aliases and public reexports may
  resolve to a unique source-layout candidate. These edges remain `INFERRED`,
  not compiler-confirmed dispatch facts.
- Receiver methods, function pointers, generic type dispatch, UFCS, trait dispatch,
  macro expansion, shadowed names and competing cfg definitions remain unresolved
  or ambiguous. Candidate IDs do not become invented call edges.

Each Rust edge has `direction: "source_to_target"`; calls and imports also carry
a `resolution` label. The knowledge graph's overall traversal is still undirected. The per-edge direction
records syntax ownership, not runtime scheduling or proof that a target executes.
The NetworkX export is a multigraph to retain separate call sites and relations.

Call-target placeholders carry `unresolved: true`, `item_kind: "call_target"` and
`has_body: false`; consumers must not count them as real function definitions.
Syntax errors remain visible as `extraction_error` metadata on their file node.

Confidence has an explicit scope. Every observed Rust call carries
`syntax_confidence: "EXTRACTED"`. An unresolved call edge points to its placeholder
and has `confidence_scope: "observed-callsite"`, edge confidence `EXTRACTED` and
`target_confidence: "UNRESOLVED"`: this confirms syntax only. A unique source
candidate instead has edge confidence and `target_confidence: "INFERRED"` with
`confidence_scope: "target-candidate"`. Competing candidates have `AMBIGUOUS`
target confidence. None of these values proves compiler dispatch or execution.

Cargo package/lib names and conventional source-layout modules provide namespace
context. Macro-expanded code, `#[path]` module remapping, custom code generation,
conditional compilation and type aliases are not compiler-resolved. A package
with `#[path]` module remapping quarantines only the declared module namespace
and its physical source tree rather than asserting filename-derived targets.
Unrelated production modules remain eligible for resolution, including when a
remapped module is guarded by `#[cfg(test)]`. Binary sources beside a package library
retain unresolved targets until their separate Cargo target is modeled; distinct
packages sharing a library name are treated as ambiguous. Use compiler
or rust-analyzer tooling when those cases require exact target resolution.

The CLI resolves Rust project relations after all per-file cached results have
been assembled. Cache identities include path, source content and extractor
version and ancestor Cargo manifests, and the build output records its successful
extractor and package-context fingerprint. Copying,
renaming or upgrading the extractor therefore cannot silently reuse old source
identities or an older graph schema.

The CLI moves assembled extraction payloads into the graph instead of holding
multiple complete clones of the corpus. This retains all nodes, parallel call
sites and metadata while bounding graph-assembly memory by the input payload
and graph indexes rather than repeated copies of their JSON metadata.
