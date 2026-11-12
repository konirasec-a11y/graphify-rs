//! Source-layout context, explicitly separate from compiler name resolution.
use std::{collections::BTreeMap, path::Path};

pub(super) struct Context {
    pub crate_name: String,
    pub package_name: String,
    pub resolution_disabled: bool,
    pub module: Vec<String>,
    pub aliases: BTreeMap<String, String>,
}

pub(super) fn context(path: &Path) -> Context {
    let absolute = path.canonicalize().unwrap_or_else(|_| path.to_owned());
    let found = absolute.ancestors().skip(1).find_map(|directory| {
        let manifest = directory.join("Cargo.toml");
        let text = std::fs::read_to_string(&manifest).ok()?;
        let value: toml::Value = toml::from_str(&text).ok()?;
        value.get("package")?.get("name")?.as_str()?;
        Some((directory.to_owned(), value))
    });
    if let Some((root, manifest)) = found {
        let package = manifest["package"]["name"].as_str().unwrap_or("crate");
        let crate_name = manifest
            .get("lib")
            .and_then(|v| v.get("name"))
            .and_then(toml::Value::as_str)
            .unwrap_or(package)
            .replace('-', "_");
        let lib_root = manifest
            .get("lib")
            .and_then(|v| v.get("path"))
            .and_then(toml::Value::as_str)
            .map(|path| root.join(path))
            .unwrap_or_else(|| root.join("src/lib.rs"));
        let lib_root = lib_root.canonicalize().unwrap_or(lib_root);
        let source_root = lib_root.parent().unwrap_or(&root).to_owned();
        let has_library = lib_root.is_file() || manifest.get("lib").is_some();
        let explicit_binary = manifest
            .get("bin")
            .and_then(toml::Value::as_array)
            .is_some_and(|targets| {
                targets.iter().any(|target| {
                    target
                        .get("path")
                        .and_then(toml::Value::as_str)
                        .is_some_and(|path| {
                            root.join(path).canonicalize().is_ok_and(|p| p == absolute)
                        })
                })
            });
        let resolution_disabled = has_library
            && (absolute == root.join("src/main.rs")
                || absolute.starts_with(root.join("src/bin"))
                || explicit_binary);
        let relative = absolute.strip_prefix(&source_root).ok();
        let module = if absolute == lib_root {
            Vec::new()
        } else {
            relative.map(module_components).unwrap_or_else(|| {
                // Examples/tests are separate crate roots, not modules of the library.
                // Give them a distinct scope rather than inventing a library binding.
                vec![format!(
                    "__target_{}",
                    absolute.file_stem().unwrap_or_default().to_string_lossy()
                )]
            })
        };
        let aliases = dependency_aliases(&manifest);
        Context {
            package_name: package.replace('-', "_"),
            resolution_disabled,
            crate_name,
            module,
            aliases,
        }
    } else {
        // Useful for snippets/fixtures. A path-derived root avoids linking unrelated
        // loose directories merely because their filenames happen to match.
        let src = absolute
            .ancestors()
            .find(|p| p.file_name().is_some_and(|n| n == "src"));
        let base = src.unwrap_or_else(|| absolute.parent().unwrap_or(Path::new("")));
        let crate_name = base
            .parent()
            .and_then(Path::file_name)
            .unwrap_or_default()
            .to_string_lossy()
            .replace('-', "_");
        Context {
            resolution_disabled: false,
            package_name: if crate_name.is_empty() {
                "crate".into()
            } else {
                crate_name.clone()
            },
            crate_name: if crate_name.is_empty() {
                "crate".into()
            } else {
                crate_name
            },
            module: module_components(absolute.strip_prefix(base).unwrap_or(&absolute)),
            aliases: BTreeMap::new(),
        }
    }
}

fn module_components(path: &Path) -> Vec<String> {
    let mut parts: Vec<String> = path
        .parent()
        .unwrap_or(Path::new(""))
        .components()
        .map(|c| c.as_os_str().to_string_lossy().into_owned())
        .collect();
    let stem = path.file_stem().unwrap_or_default().to_string_lossy();
    if !["mod", "lib", "main"].contains(&stem.as_ref()) {
        parts.push(stem.into_owned());
    }
    parts
}

fn dependency_aliases(manifest: &toml::Value) -> BTreeMap<String, String> {
    let mut aliases = BTreeMap::new();
    for section in ["dependencies", "dev-dependencies", "build-dependencies"] {
        if let Some(table) = manifest.get(section).and_then(toml::Value::as_table) {
            add_aliases(table, &mut aliases);
        }
    }
    if let Some(targets) = manifest.get("target").and_then(toml::Value::as_table) {
        for target in targets.values() {
            for section in ["dependencies", "dev-dependencies", "build-dependencies"] {
                if let Some(table) = target.get(section).and_then(toml::Value::as_table) {
                    add_aliases(table, &mut aliases);
                }
            }
        }
    }
    aliases
}

fn add_aliases(
    table: &toml::map::Map<String, toml::Value>,
    aliases: &mut BTreeMap<String, String>,
) {
    for (alias, value) in table {
        let package = value
            .get("package")
            .and_then(toml::Value::as_str)
            .unwrap_or(alias);
        aliases.insert(alias.replace('-', "_"), package.replace('-', "_"));
    }
}
