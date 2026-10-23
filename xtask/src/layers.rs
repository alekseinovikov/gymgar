//! Dependency layering rules (CLAUDE.md › Workspace map).
//!
//! A crate may depend only on crates in lower layers; standalone crates depend on no workspace
//! crate; only the UI crate (and the apps) may use a UI toolkit. The rule engine works on a small
//! model so it can be unit-tested; [`from_metadata`] builds that model from `cargo metadata`.

use std::collections::HashSet;

use serde_json::Value;

/// Where a workspace crate sits.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Class {
    /// Regular layered crate.
    Layer(u8),
    /// Layer 0 that additionally may depend on no workspace crate at all.
    Standalone,
    /// Binaries and build tooling: exempt from the rules.
    Exempt,
}

impl Class {
    fn layer(self) -> Option<u8> {
        match self {
            Class::Layer(l) => Some(l),
            Class::Standalone => Some(0),
            Class::Exempt => None,
        }
    }
}

/// The layering table. Names are package names without the `gymgar-` prefix. A new crate must be
/// registered here, or `cargo xtask layers` fails.
pub const TABLE: &[(&str, Class)] = &[
    ("domain", Class::Layer(0)),
    ("garmin", Class::Standalone),
    // Secret storage (system keyring + file); standalone so the apps can wrap it in engine adapters.
    ("secrets", Class::Standalone),
    ("store", Class::Layer(1)),
    ("sync", Class::Layer(2)),
    ("engine", Class::Layer(3)),
    ("ui-egui", Class::Layer(4)),
    ("automation", Class::Layer(4)),
    // apps and tooling
    ("gymgar", Class::Exempt),
    ("xtask", Class::Exempt),
];

/// The only library crates allowed to depend on a UI toolkit.
const UI_CRATES: &[&str] = &["ui-egui"];

/// UI toolkit and windowing crates.
const TOOLKIT: &[&str] = &["egui", "eframe", "egui_extras", "egui_plot", "egui-wgpu", "egui-winit", "egui_kittest", "winit", "wgpu", "rfd"];

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Package {
    pub name: String,
    pub deps: Vec<Dep>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Dep {
    pub name: String,
    /// Dev-dependencies (tests only) are not checked.
    pub dev: bool,
}

fn short(name: &str) -> &str {
    name.strip_prefix("gymgar-").unwrap_or(name)
}

fn class_of(name: &str) -> Option<Class> {
    TABLE.iter().find(|(n, _)| *n == short(name)).map(|(_, c)| *c)
}

/// Every violation in `packages` (the workspace members), as readable messages.
pub fn check(packages: &[Package]) -> Vec<String> {
    let members: HashSet<&str> = packages.iter().map(|p| p.name.as_str()).collect();
    let mut errors = Vec::new();
    for p in packages {
        let Some(class) = class_of(&p.name) else {
            errors.push(format!("`{}` is not registered in xtask/src/layers.rs", p.name));
            continue;
        };
        let Some(layer) = class.layer() else { continue };
        for dep in p.deps.iter().filter(|d| !d.dev) {
            if members.contains(dep.name.as_str()) {
                if class == Class::Standalone {
                    errors.push(format!("`{}` is standalone and may not depend on workspace crate `{}`", p.name, dep.name));
                    continue;
                }
                // An unregistered dependency is reported on its own entry.
                let Some(dep_class) = class_of(&dep.name) else { continue };
                match dep_class.layer() {
                    Some(dep_layer) if dep_layer < layer => {}
                    Some(dep_layer) => errors.push(format!("`{}` (L{layer}) may not depend on `{}` (L{dep_layer}): only lower layers", p.name, dep.name)),
                    None => errors.push(format!("`{}` may not depend on app/tooling crate `{}`", p.name, dep.name)),
                }
            } else if TOOLKIT.contains(&dep.name.as_str()) && !UI_CRATES.contains(&short(&p.name)) {
                errors.push(format!("`{}` may not depend on UI toolkit crate `{}`: only {UI_CRATES:?} and the apps may", p.name, dep.name));
            }
        }
    }
    errors
}

/// The workspace members and their declared dependencies, from `cargo metadata --no-deps`.
pub fn from_metadata(meta: &Value) -> Result<Vec<Package>, String> {
    let packages = meta.get("packages").and_then(Value::as_array).ok_or("cargo metadata: no `packages` array")?;
    packages
        .iter()
        .map(|p| {
            let name = p.get("name").and_then(Value::as_str).ok_or("cargo metadata: package without a name")?;
            let deps = p
                .get("dependencies")
                .and_then(Value::as_array)
                .map(|deps| {
                    deps.iter()
                        .filter_map(|d| {
                            let name = d.get("name").and_then(Value::as_str)?;
                            Some(Dep { name: name.to_string(), dev: d.get("kind").and_then(Value::as_str) == Some("dev") })
                        })
                        .collect()
                })
                .unwrap_or_default();
            Ok(Package { name: name.to_string(), deps })
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    fn pkg(name: &str, deps: &[(&str, bool)]) -> Package {
        Package { name: name.into(), deps: deps.iter().map(|(n, dev)| Dep { name: (*n).into(), dev: *dev }).collect() }
    }

    fn workspace(extra: Package) -> Vec<Package> {
        let mut all: Vec<Package> = TABLE.iter().map(|(n, _)| pkg(&format!("gymgar-{n}"), &[])).collect();
        all.retain(|p| p.name != extra.name);
        all.push(extra);
        all
    }

    #[test]
    fn lower_layer_dependency_is_allowed() {
        let errors = check(&workspace(pkg("gymgar-engine", &[("gymgar-sync", false), ("gymgar-garmin", false), ("serde", false)])));
        assert_eq!(errors, Vec::<String>::new());
    }

    #[test]
    fn same_or_higher_layer_is_rejected() {
        let errors = check(&workspace(pkg("gymgar-sync", &[("gymgar-engine", false)])));
        assert_eq!(errors.len(), 1, "{errors:?}");
        let errors = check(&workspace(pkg("gymgar-automation", &[("gymgar-ui-egui", false)])));
        assert_eq!(errors.len(), 1, "{errors:?}");
    }

    #[test]
    fn standalone_depends_on_no_workspace_crate() {
        let errors = check(&workspace(pkg("gymgar-garmin", &[("gymgar-domain", false)])));
        assert_eq!(errors.len(), 1, "{errors:?}");
    }

    #[test]
    fn toolkit_only_in_ui_crate_and_apps() {
        assert_eq!(check(&workspace(pkg("gymgar-engine", &[("egui", false)]))).len(), 1);
        assert!(check(&workspace(pkg("gymgar-ui-egui", &[("egui", false), ("eframe", false)]))).is_empty());
        assert!(check(&workspace(pkg("gymgar", &[("eframe", false)]))).is_empty());
    }

    #[test]
    fn dev_dependencies_are_not_checked() {
        assert!(check(&workspace(pkg("gymgar-domain", &[("gymgar-engine", true), ("egui_kittest", true)]))).is_empty());
    }

    #[test]
    fn unregistered_crate_is_reported() {
        let errors = check(&workspace(pkg("gymgar-mystery", &[])));
        assert_eq!(errors, vec!["`gymgar-mystery` is not registered in xtask/src/layers.rs".to_string()]);
    }

    #[test]
    fn reads_cargo_metadata() {
        let meta = json!({"packages": [{"name": "gymgar-engine", "dependencies": [
            {"name": "gymgar-sync", "kind": null},
            {"name": "egui_kittest", "kind": "dev"}
        ]}]});
        let packages = from_metadata(&meta).unwrap();
        assert_eq!(packages, vec![pkg("gymgar-engine", &[("gymgar-sync", false), ("egui_kittest", true)])]);
    }
}
