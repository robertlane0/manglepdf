//! Reading the workspace as Cargo sees it.
//!
//! Everything the dependency rules need is already in `cargo metadata`, so there is
//! no second TOML parser to keep in step with the manifests.

use std::collections::BTreeMap;
use std::fmt::Write as _;
use std::path::{Path, PathBuf};
use std::process::Command;

/// One first-party crate, as Cargo describes it.
#[derive(Debug, Clone)]
pub(crate) struct Member {
    pub name: String,
    pub dir: PathBuf,
    /// Every target: lib, bin, test, bench, example and any `build.rs`.
    pub targets: Vec<Target>,
    pub dependencies: Vec<Dependency>,
}

#[derive(Debug, Clone)]
pub(crate) struct Target {
    pub name: String,
    /// `lib`, `bin`, `test`, `bench`, `example`, `custom-build`.
    pub kind: String,
    /// The crate root file.
    pub src_path: PathBuf,
}

#[derive(Debug, Clone)]
pub(crate) struct Dependency {
    pub name: String,
    pub version: String,
    /// `null` for a path or workspace dependency.
    pub source: Option<String>,
    pub is_dev: bool,
    pub is_build: bool,
}

/// The whole workspace, plus the transitive third-party graph.
#[derive(Debug)]
pub(crate) struct Workspace {
    pub root: PathBuf,
    pub members: Vec<Member>,
    /// Third-party packages, keyed by name. First-party crates are not included.
    pub external: BTreeMap<String, ExternalPackage>,
}

#[derive(Debug, Clone)]
pub(crate) struct ExternalPackage {
    pub name: String,
    pub version: String,
    pub source: Option<String>,
    pub description: Option<String>,
    pub repository: Option<String>,
    pub keywords: Vec<String>,
    /// Dependency names, to walk the graph from the workspace roots.
    pub deps: Vec<String>,
}

impl Workspace {
    /// Load the workspace by asking Cargo. Fails only if Cargo itself fails.
    pub(crate) fn load() -> Result<Self, String> {
        let root = repo_root()?;
        let out = Command::new(cargo())
            .current_dir(&root)
            .args([
                "metadata",
                "--format-version",
                "1",
                "--all-features",
                "--locked",
            ])
            .output()
            .map_err(|e| format!("could not run cargo metadata: {e}"))?;
        if !out.status.success() {
            return Err(format!(
                "cargo metadata failed:\n{}",
                String::from_utf8_lossy(&out.stderr)
            ));
        }
        let json: serde_json::Value =
            serde_json::from_slice(&out.stdout).map_err(|e| format!("metadata: {e}"))?;
        Self::from_metadata(&root, &json)
    }

    fn from_metadata(root: &Path, json: &serde_json::Value) -> Result<Self, String> {
        let packages = json
            .get("packages")
            .and_then(serde_json::Value::as_array)
            .ok_or("metadata has no packages")?;
        let workspace_members: Vec<&str> = json
            .get("workspace_members")
            .and_then(serde_json::Value::as_array)
            .map(|a| a.iter().filter_map(|v| v.as_str()).collect())
            .unwrap_or_default();

        let first_party = |pkg: &serde_json::Value| -> bool {
            pkg.get("id")
                .and_then(serde_json::Value::as_str)
                .is_some_and(|id| workspace_members.contains(&id))
        };

        let mut members = Vec::new();
        let mut external: BTreeMap<String, ExternalPackage> = BTreeMap::new();

        for pkg in packages {
            let name = pkg
                .get("name")
                .and_then(serde_json::Value::as_str)
                .ok_or("a package has no name")?
                .to_string();
            if first_party(pkg) {
                members.push(Member {
                    name: name.clone(),
                    dir: PathBuf::from(
                        pkg.get("manifest_path")
                            .and_then(serde_json::Value::as_str)
                            .unwrap_or_default(),
                    )
                    .parent()
                    .unwrap_or(root)
                    .to_path_buf(),
                    targets: pkg
                        .get("targets")
                        .and_then(serde_json::Value::as_array)
                        .map(|ts| {
                            ts.iter()
                                .map(|t| Target {
                                    name: t
                                        .get("name")
                                        .and_then(serde_json::Value::as_str)
                                        .unwrap_or_default()
                                        .to_string(),
                                    kind: t
                                        .get("kind")
                                        .and_then(serde_json::Value::as_array)
                                        .and_then(|k| k.first())
                                        .and_then(serde_json::Value::as_str)
                                        .unwrap_or("lib")
                                        .to_string(),
                                    src_path: PathBuf::from(
                                        t.get("src_path")
                                            .and_then(serde_json::Value::as_str)
                                            .unwrap_or_default(),
                                    ),
                                })
                                .collect()
                        })
                        .unwrap_or_default(),
                    dependencies: pkg
                        .get("dependencies")
                        .and_then(serde_json::Value::as_array)
                        .map(|ds| {
                            ds.iter()
                                .map(|d| Dependency {
                                    name: d
                                        .get("name")
                                        .and_then(serde_json::Value::as_str)
                                        .unwrap_or_default()
                                        .to_string(),
                                    version: d
                                        .get("req")
                                        .and_then(serde_json::Value::as_str)
                                        .unwrap_or_default()
                                        .to_string(),
                                    source: d
                                        .get("source")
                                        .and_then(serde_json::Value::as_str)
                                        .map(str::to_string),
                                    is_dev: d
                                        .get("kind")
                                        .and_then(serde_json::Value::as_array)
                                        .is_some_and(|k| k.iter().any(|x| x == "dev")),
                                    is_build: d
                                        .get("kind")
                                        .and_then(serde_json::Value::as_array)
                                        .is_some_and(|k| k.iter().any(|x| x == "build")),
                                })
                                .collect()
                        })
                        .unwrap_or_default(),
                });
            } else {
                let deps = pkg
                    .get("dependencies")
                    .and_then(serde_json::Value::as_array)
                    .map(|ds| {
                        ds.iter()
                            .filter_map(|d| d.get("name").and_then(serde_json::Value::as_str))
                            .map(str::to_string)
                            .collect()
                    })
                    .unwrap_or_default();
                external.insert(
                    name.clone(),
                    ExternalPackage {
                        name: name.clone(),
                        version: pkg
                            .get("version")
                            .and_then(serde_json::Value::as_str)
                            .unwrap_or_default()
                            .to_string(),
                        source: pkg
                            .get("source")
                            .and_then(serde_json::Value::as_str)
                            .map(str::to_string),
                        description: pkg
                            .get("description")
                            .and_then(serde_json::Value::as_str)
                            .map(str::to_string),
                        repository: pkg
                            .get("repository")
                            .and_then(serde_json::Value::as_str)
                            .map(str::to_string),
                        keywords: pkg
                            .get("keywords")
                            .and_then(serde_json::Value::as_array)
                            .map(|k| {
                                k.iter()
                                    .filter_map(serde_json::Value::as_str)
                                    .map(str::to_string)
                                    .collect()
                            })
                            .unwrap_or_default(),
                        deps,
                    },
                );
            }
        }

        members.sort_by(|a, b| a.name.cmp(&b.name));
        Ok(Self {
            root: root.to_path_buf(),
            members,
            external,
        })
    }

    /// Every transitive third-party package reachable from the workspace.
    #[must_use]
    pub(crate) fn third_party_closure(&self) -> Vec<ExternalPackage> {
        let mut seen: Vec<String> = Vec::new();
        let mut stack: Vec<String> = self
            .members
            .iter()
            .flat_map(|m| m.dependencies.iter().map(|d| d.name.clone()))
            .collect();
        while let Some(name) = stack.pop() {
            if seen.contains(&name) {
                continue;
            }
            seen.push(name.clone());
            if let Some(p) = self.external.get(&name) {
                stack.extend(p.deps.iter().cloned());
            }
        }
        seen.iter()
            .filter_map(|n| self.external.get(n).cloned())
            .collect()
    }
}

/// The repository root: the nearest ancestor holding a `Cargo.toml` workspace.
pub(crate) fn repo_root() -> Result<PathBuf, String> {
    let manifest = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    manifest
        .parent()
        .map(Path::to_path_buf)
        .ok_or_else(|| "xtask has no parent directory".to_string())
}

/// The cargo binary, honouring `CARGO` so `cargo xtask` works under wrappers.
#[must_use]
pub(crate) fn cargo() -> PathBuf {
    std::env::var_os("CARGO").map_or_else(|| PathBuf::from("cargo"), PathBuf::from)
}

/// The third-party dependency tree, one line per package, for the audit trail in
/// `docs/DEPENDENCIES.md`.
#[must_use]
pub(crate) fn dependency_report(ws: &Workspace) -> String {
    let mut out = String::from("| crate | version | why it is here |\n|---|---|---|\n");
    for m in &ws.members {
        for d in &m.dependencies {
            if ws.members.iter().any(|o| o.name == d.name) {
                continue;
            }
            let kind = if d.is_build {
                "build"
            } else if d.is_dev {
                "dev"
            } else {
                "runtime"
            };
            let origin = d
                .source
                .as_deref()
                .and_then(|s| s.rsplit('/').next())
                .unwrap_or("this repository");
            let _ = writeln!(
                out,
                "| `{}` | `{}` | {} dependency of `{}` ({}) |",
                d.name, d.version, kind, m.name, origin
            );
        }
    }
    out
}
