//! Dependency resolution + ABI check + topological load order.
//!
//! Spec: PLUGIN_MANIFEST.md §17 rules 7 (ABI) and 8 (plugin deps), plus the
//! implicit rule that a plugin must load after its dependencies so the
//! kernel can wire cross-plugin references during init.

use std::collections::{BTreeMap, BTreeSet, HashMap};

use semver::{Version, VersionReq};
use thiserror::Error;

use super::manifest::Manifest;

/// The ABI versions this kernel supports. Plugins whose `[abi]` declares
/// a version outside these ranges are refused at load time.
///
/// Bump these when an ABI becomes incompatible — PLUGIN_MANIFEST.md §18
/// governs schema versioning.
pub struct KernelAbiSupport {
    pub skill: Version,
    pub agent: Version,
    pub sancho_task: Version,
    pub mcp_tool: Version,
    pub mascot: Version,
    pub bootstrap_fragment: Version,
}

/// Phase C: every ABI ships at 0.1.0. Phase E will lock these to 1.0.0
/// after dogfood confirms the shape is right.
pub const KERNEL_ABI_SUPPORT: KernelAbiSupport = KernelAbiSupport {
    skill: Version::new(0, 1, 0),
    agent: Version::new(0, 1, 0),
    sancho_task: Version::new(0, 1, 0),
    mcp_tool: Version::new(0, 1, 0),
    mascot: Version::new(0, 1, 0),
    bootstrap_fragment: Version::new(0, 1, 0),
};

#[derive(Debug, Error)]
pub enum ResolverError {
    #[error(
        "plugin {plugin:?} declares {abi_kind} = {req} but kernel supports {abi_kind} = {kernel}"
    )]
    AbiMismatch {
        plugin: String,
        abi_kind: &'static str,
        req: VersionReq,
        kernel: Version,
    },
    #[error("plugin {plugin:?} depends on {dep:?} {req} — dependency not installed")]
    MissingDependency {
        plugin: String,
        dep: String,
        req: VersionReq,
    },
    #[error(
        "plugin {plugin:?} depends on {dep:?} {req} — installed version {installed} incompatible"
    )]
    IncompatibleDependency {
        plugin: String,
        dep: String,
        req: VersionReq,
        installed: Version,
    },
    #[error("dependency cycle detected involving plugins: {involved:?}")]
    Cycle { involved: Vec<String> },
    #[error("invalid dependency spec {spec:?} in plugin {plugin:?}: {msg}")]
    InvalidDepSpec {
        plugin: String,
        spec: String,
        msg: String,
    },
}

/// Parse an entry from `[depends].plugins` — shape is `"name constraint"`,
/// e.g. `"brain ^1.0"`. The constraint is optional; missing means `*`.
pub(crate) fn parse_dep_spec(
    plugin: &str,
    spec: &str,
) -> Result<(String, VersionReq), ResolverError> {
    let trimmed = spec.trim();
    let mut parts = trimmed.splitn(2, char::is_whitespace);
    let name = parts
        .next()
        .ok_or_else(|| ResolverError::InvalidDepSpec {
            plugin: plugin.to_string(),
            spec: spec.to_string(),
            msg: "empty dep spec".into(),
        })?
        .to_string();
    let req = match parts.next() {
        None => VersionReq::STAR,
        Some(raw) => raw
            .trim()
            .parse::<VersionReq>()
            .map_err(|e| ResolverError::InvalidDepSpec {
                plugin: plugin.to_string(),
                spec: spec.to_string(),
                msg: format!("bad version constraint: {e}"),
            })?,
    };
    Ok((name, req))
}

/// Check the plugin's [abi] block against kernel support (rule 7).
pub fn check_abi(m: &Manifest) -> Result<(), ResolverError> {
    let checks: &[(&'static str, Option<&VersionReq>, &Version)] = &[
        ("skill", m.abi.skill.as_ref(), &KERNEL_ABI_SUPPORT.skill),
        ("agent", m.abi.agent.as_ref(), &KERNEL_ABI_SUPPORT.agent),
        (
            "sancho-task",
            m.abi.sancho_task.as_ref(),
            &KERNEL_ABI_SUPPORT.sancho_task,
        ),
        (
            "mcp-tool",
            m.abi.mcp_tool.as_ref(),
            &KERNEL_ABI_SUPPORT.mcp_tool,
        ),
        ("mascot", m.abi.mascot.as_ref(), &KERNEL_ABI_SUPPORT.mascot),
        (
            "bootstrap-fragment",
            m.abi.bootstrap_fragment.as_ref(),
            &KERNEL_ABI_SUPPORT.bootstrap_fragment,
        ),
    ];
    for (kind, req_opt, kernel) in checks {
        if let Some(req) = req_opt {
            if !req.matches(kernel) {
                return Err(ResolverError::AbiMismatch {
                    plugin: m.plugin.name.clone(),
                    abi_kind: kind,
                    req: (*req).clone(),
                    kernel: (*kernel).clone(),
                });
            }
        }
    }
    Ok(())
}

/// Given the set of installed plugin manifests, return them in a valid
/// load order (deps before dependents). Fails on missing dep, version
/// mismatch, ABI mismatch, or cycle.
pub fn resolve_load_order(manifests: &[Manifest]) -> Result<Vec<Manifest>, ResolverError> {
    // Step 1: ABI check every plugin up front.
    for m in manifests {
        check_abi(m)?;
    }

    // Build name → manifest lookup. Duplicates are a registry-level concern,
    // not a resolver concern — assume unique here.
    let by_name: HashMap<&str, &Manifest> =
        manifests.iter().map(|m| (m.plugin.name.as_str(), m)).collect();

    // Step 2: resolve every [depends].plugins entry and build an adjacency
    // list (plugin → deps) keyed by name.
    let mut adj: BTreeMap<String, Vec<String>> = BTreeMap::new();
    for m in manifests {
        let name = m.plugin.name.clone();
        let mut deps: Vec<String> = Vec::new();
        for spec in &m.depends.plugins {
            let (dep_name, req) = parse_dep_spec(&name, spec)?;
            let dep = by_name
                .get(dep_name.as_str())
                .ok_or_else(|| ResolverError::MissingDependency {
                    plugin: name.clone(),
                    dep: dep_name.clone(),
                    req: req.clone(),
                })?;
            if !req.matches(&dep.plugin.version) {
                return Err(ResolverError::IncompatibleDependency {
                    plugin: name.clone(),
                    dep: dep_name.clone(),
                    req,
                    installed: dep.plugin.version.clone(),
                });
            }
            deps.push(dep_name);
        }
        adj.insert(name, deps);
    }

    // Step 3: Kahn's algorithm topological sort. A plugin's in-degree is
    // the number of dependencies it still needs emitted. Alphabetical
    // tie-break inside the ready set keeps output deterministic.
    let mut in_degree: BTreeMap<String, usize> = adj
        .iter()
        .map(|(name, deps)| (name.clone(), deps.len()))
        .collect();

    // Ready set: plugins with no remaining deps.
    let mut ready: BTreeSet<String> = in_degree
        .iter()
        .filter(|(_, &d)| d == 0)
        .map(|(n, _)| n.clone())
        .collect();

    // Reverse adjacency: dep → list of plugins that depend on it. When a
    // plugin gets emitted, we decrement each dependent's in-degree.
    let mut reverse: BTreeMap<String, Vec<String>> = BTreeMap::new();
    for (name, deps) in &adj {
        for d in deps {
            reverse.entry(d.clone()).or_default().push(name.clone());
        }
    }

    let mut order: Vec<String> = Vec::new();
    while let Some(next) = ready.iter().next().cloned() {
        ready.remove(&next);
        order.push(next.clone());
        if let Some(dependents) = reverse.get(&next) {
            for dep in dependents {
                if let Some(deg) = in_degree.get_mut(dep) {
                    *deg = deg.saturating_sub(1);
                    if *deg == 0 {
                        ready.insert(dep.clone());
                    }
                }
            }
        }
    }

    if order.len() != manifests.len() {
        let involved: Vec<String> = in_degree
            .iter()
            .filter(|(_, &d)| d > 0)
            .map(|(n, _)| n.clone())
            .collect();
        return Err(ResolverError::Cycle { involved });
    }

    // Materialize the manifests in order.
    let sorted: Vec<Manifest> = order
        .into_iter()
        .filter_map(|name| by_name.get(name.as_str()).map(|m| (*m).clone()))
        .collect();
    Ok(sorted)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn make(name: &str, version: &str, deps: &[&str]) -> Manifest {
        let deps_line = if deps.is_empty() {
            String::new()
        } else {
            let items: Vec<String> = deps.iter().map(|d| format!("{d:?}")).collect();
            format!("\n[depends]\nplugins = [{}]", items.join(", "))
        };
        let body = format!(
            r#"
[plugin]
name = "{name}"
version = "{version}"
kind = "skill"
language = "python"

[source]
path = "local/{name}"

[abi]
skill = "^0.1"

[entrypoint]
run = "true"
{deps_line}
"#
        );
        Manifest::parse(&body, &PathBuf::from("test.toml")).unwrap().0
    }

    #[test]
    fn topo_sort_linear_chain() {
        let manifests = vec![
            make("cc", "1.0.0", &["bb ^1"]),
            make("aa", "1.0.0", &[]),
            make("bb", "1.0.0", &["aa ^1"]),
        ];
        let ordered = resolve_load_order(&manifests).unwrap();
        let names: Vec<&str> = ordered.iter().map(|m| m.plugin.name.as_str()).collect();
        assert_eq!(names, vec!["aa", "bb", "cc"]);
    }

    #[test]
    fn topo_sort_parallel_deps() {
        // Both bb and cc depend on aa, no order between bb and cc. Kahn
        // with alphabetical tie-break → aa, bb, cc.
        let manifests = vec![
            make("aa", "1.0.0", &[]),
            make("bb", "1.0.0", &["aa ^1"]),
            make("cc", "1.0.0", &["aa ^1"]),
        ];
        let ordered = resolve_load_order(&manifests).unwrap();
        let names: Vec<&str> = ordered.iter().map(|m| m.plugin.name.as_str()).collect();
        assert_eq!(names, vec!["aa", "bb", "cc"]);
    }

    #[test]
    fn cycle_detected() {
        let manifests = vec![
            make("aa", "1.0.0", &["bb ^1"]),
            make("bb", "1.0.0", &["aa ^1"]),
        ];
        let err = resolve_load_order(&manifests).unwrap_err();
        match err {
            ResolverError::Cycle { involved } => {
                assert!(involved.contains(&"aa".to_string()));
                assert!(involved.contains(&"bb".to_string()));
            }
            e => panic!("expected Cycle, got {e:?}"),
        }
    }

    #[test]
    fn missing_dependency_fails() {
        let manifests = vec![make("aa", "1.0.0", &["bb ^1"])];
        let err = resolve_load_order(&manifests).unwrap_err();
        assert!(matches!(err, ResolverError::MissingDependency { .. }));
    }

    #[test]
    fn version_incompatible_dep_fails() {
        let manifests = vec![
            make("aa", "0.9.0", &[]),
            make("bb", "1.0.0", &["aa ^1"]),
        ];
        let err = resolve_load_order(&manifests).unwrap_err();
        assert!(matches!(err, ResolverError::IncompatibleDependency { .. }));
    }

    #[test]
    fn abi_mismatch_fails() {
        let body = r#"
[plugin]
name = "xx"
version = "1.0.0"
kind = "skill"
language = "python"

[source]
path = "local/x"

[abi]
skill = "^2.0"

[entrypoint]
run = "true"
"#;
        let m = Manifest::parse(body, &PathBuf::from("t.toml")).unwrap().0;
        let err = resolve_load_order(&[m]).unwrap_err();
        assert!(matches!(err, ResolverError::AbiMismatch { .. }));
    }

    #[test]
    fn parse_dep_spec_forms() {
        let (n, r) = parse_dep_spec("x", "brain ^1.0").unwrap();
        assert_eq!(n, "brain");
        assert!(r.matches(&Version::new(1, 2, 3)));

        let (n, r) = parse_dep_spec("x", "alone").unwrap();
        assert_eq!(n, "alone");
        assert_eq!(r, VersionReq::STAR);
    }
}
