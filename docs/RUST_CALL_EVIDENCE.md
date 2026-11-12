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

Cargo package/lib names and conventional source-layout modules provide namespace
context. Macro-expanded code, `#[path]` module remapping, custom code generation,
conditional compilation and type aliases are not compiler-resolved. A package
with `#[path]` module remapping conservatively retains unresolved calls rather
than asserting filename-derived targets. Binary sources beside a package library
retain unresolved targets until their separate Cargo target is modeled; distinct
packages sharing a library name are treated as ambiguous. Use compiler
or rust-analyzer tooling when those cases require exact target resolution.

The CLI resolves Rust project relations after all per-file cached results have
been assembled. Cache identities include path, source content and extractor
version and ancestor Cargo manifests, and the build output records its successful
extractor and package-context fingerprint. Copying,
renaming or upgrading the extractor therefore cannot silently reuse old source
identities or an older graph schema.
