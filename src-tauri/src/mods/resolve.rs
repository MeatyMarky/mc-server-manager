//! Dependency resolution.
//!
//! Required dependencies are followed recursively and the whole tree is returned
//! for the user to confirm *before* anything is downloaded. Optional
//! dependencies are listed and never installed on their own. Two versions of the
//! same project in one plan is a conflict, and a conflict is refused with the
//! names in the message rather than resolved by guessing.

use std::collections::{BTreeMap, BTreeSet};

use serde::{Deserialize, Serialize};
use ts_rs::TS;

use crate::error::{AppError, AppResult};
use crate::mcversion::VersionIndex;

use super::source::{
    DependencyKind, Loader, ModSource, SourceId, SourceVersion, VersionFilter,
};

/// How deep a dependency chain may go before something is wrong with the data.
const MAX_DEPTH: usize = 12;

/// One entry of the plan the user confirms.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export, export_to = "../../src/lib/bindings/")]
pub struct PlannedMod {
    /// Where this came from. Two sources are in play, and a file reference from
    /// one is meaningless to the other.
    pub source: SourceId,
    pub project_id: String,
    pub project_title: String,
    pub version_id: String,
    pub version_number: String,
    pub file_name: String,
    #[ts(type = "number | null")]
    pub size: Option<u64>,
    /// 0 for what the user asked for, 1+ for what it pulled in.
    #[ts(type = "number")]
    pub depth: u32,
    /// Which project required this one, when it was not asked for directly.
    pub required_by: Option<String>,
    /// True when this exact file is already installed.
    pub already_installed: bool,
}

/// A dependency the user may want but that is never installed automatically.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export, export_to = "../../src/lib/bindings/")]
pub struct OptionalDependency {
    pub project_id: String,
    pub project_title: String,
    pub suggested_by: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export, export_to = "../../src/lib/bindings/")]
pub struct InstallPlan {
    /// Everything that would be installed, roots first.
    pub install: Vec<PlannedMod>,
    pub optional: Vec<OptionalDependency>,
    /// Projects the plan says are incompatible with something being installed.
    pub incompatible: Vec<String>,
    #[ts(type = "number")]
    pub total_size: u64,
}

/// What is already installed, so a plan can skip it and spot conflicts.
#[derive(Debug, Clone, Default)]
pub struct Installed {
    /// project id -> (version id, file name)
    pub by_project: BTreeMap<String, (String, String)>,
}

impl Installed {
    pub fn version_of(&self, project_id: &str) -> Option<&str> {
        self.by_project
            .get(project_id)
            .map(|(version, _)| version.as_str())
    }
}

/// Picks the version to install for a project: the newest release that suits the
/// instance, falling back to the newest of any channel.
pub fn pick_version(
    versions: &[SourceVersion],
    loader: Loader,
    mc_version: &str,
    index: &VersionIndex,
) -> Option<SourceVersion> {
    let accepted = loader.accepted();
    let mut suitable: Vec<SourceVersion> = versions
        .iter()
        .filter(|version| {
            version.game_versions.iter().any(|game| game == mc_version)
                && (version.loaders.is_empty()
                    || version
                        .loaders
                        .iter()
                        .any(|declared| accepted.iter().any(|ok| declared.eq_ignore_ascii_case(ok))))
        })
        .cloned()
        .collect();

    super::modrinth::sort_versions(&mut suitable, index);
    suitable
        .iter()
        .find(|version| version.is_stable())
        .or_else(|| suitable.first())
        .cloned()
}

/// Builds the plan for installing one version, following required dependencies.
pub async fn plan<S: ModSource>(
    source: &S,
    root: SourceVersion,
    loader: Loader,
    mc_version: &str,
    index: &VersionIndex,
    installed: &Installed,
) -> AppResult<InstallPlan> {
    let mut install: Vec<PlannedMod> = Vec::new();
    let mut optional: Vec<OptionalDependency> = Vec::new();
    let mut incompatible: Vec<String> = Vec::new();
    let mut seen: BTreeSet<String> = BTreeSet::new();

    // (version, depth, required_by)
    let mut queue: Vec<(SourceVersion, u32, Option<String>)> = vec![(root, 0, None)];

    while let Some((version, depth, required_by)) = queue.pop() {
        if depth as usize > MAX_DEPTH {
            return Err(AppError::Other(
                "this mod's dependencies nest deeper than makes sense; refusing to continue".into(),
            ));
        }

        // The same project twice in one plan, at two versions, cannot be resolved
        // by guessing: say so and stop.
        if let Some(existing) = install
            .iter()
            .find(|planned| planned.project_id == version.project_id)
        {
            if existing.version_id != version.id {
                return Err(AppError::Other(format!(
                    "conflict: {} is required at two different versions ({} and {}); \
                     install one of them on its own first",
                    existing.project_title, existing.version_number, version.version_number
                )));
            }
            continue;
        }
        if !seen.insert(version.project_id.clone()) {
            continue;
        }

        if let Some(installed_version) = installed.version_of(&version.project_id) {
            if installed_version != version.id && depth > 0 {
                return Err(AppError::Other(format!(
                    "conflict: a different version of {} is already installed; \
                     update or remove it first",
                    version.project_id
                )));
            }
        }

        let title = source
            .project(&version.project_id)
            .await
            .map(|project| project.title)
            .unwrap_or_else(|_| version.project_id.clone());

        let file = version.primary_file();
        install.push(PlannedMod {
            source: version.source,
            project_id: version.project_id.clone(),
            project_title: title.clone(),
            version_id: version.id.clone(),
            version_number: version.version_number.clone(),
            file_name: file
                .map(|file| file.file_name.clone())
                .unwrap_or_else(|| format!("{}.jar", version.project_id)),
            size: file.and_then(|file| file.size),
            depth,
            required_by,
            already_installed: installed.version_of(&version.project_id) == Some(version.id.as_str()),
        });

        for dependency in &version.dependencies {
            match dependency.kind {
                DependencyKind::Required => {
                    let child = match (&dependency.version_id, &dependency.project_id) {
                        // A pinned version wins: the author asked for that build.
                        (Some(version_id), _) => source.version(version_id).await?,
                        (None, Some(project_id)) => {
                            let versions = source
                                .versions(
                                    project_id,
                                    &VersionFilter {
                                        loaders: loader
                                            .accepted()
                                            .iter()
                                            .map(|loader| loader.to_string())
                                            .collect(),
                                        game_versions: vec![mc_version.to_string()],
                                    },
                                )
                                .await?;
                            match pick_version(&versions, loader, mc_version, index) {
                                Some(version) => version,
                                None => {
                                    return Err(AppError::Other(format!(
                                        "{title} requires {project_id}, which has no build for \
                                         {} on Minecraft {mc_version}",
                                        loader.as_str()
                                    )))
                                }
                            }
                        }
                        (None, None) => continue,
                    };
                    queue.push((child, depth + 1, Some(title.clone())));
                }
                DependencyKind::Optional => {
                    if let Some(project_id) = &dependency.project_id {
                        if !optional.iter().any(|entry| &entry.project_id == project_id) {
                            optional.push(OptionalDependency {
                                project_title: source
                                    .project(project_id)
                                    .await
                                    .map(|project| project.title)
                                    .unwrap_or_else(|_| project_id.clone()),
                                project_id: project_id.clone(),
                                suggested_by: title.clone(),
                            });
                        }
                    }
                }
                DependencyKind::Incompatible => {
                    if let Some(project_id) = &dependency.project_id {
                        if installed.by_project.contains_key(project_id)
                            && !incompatible.contains(project_id)
                        {
                            incompatible.push(project_id.clone());
                        }
                    }
                }
                DependencyKind::Embedded => {}
            }
        }
    }

    // Roots first, then by how deep they were pulled in.
    install.sort_by_key(|planned| planned.depth);
    let total_size = install
        .iter()
        .filter(|planned| !planned.already_installed)
        .filter_map(|planned| planned.size)
        .sum();

    if !incompatible.is_empty() {
        return Err(AppError::Other(format!(
            "this mod is incompatible with something already installed: {}",
            incompatible.join(", ")
        )));
    }

    Ok(InstallPlan {
        install,
        optional,
        incompatible,
        total_size,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mods::source::{
        Dependency, Project, SearchQuery, SourceFile, SourceId, SourceVersion,
    };

    /// A source backed by fixtures and hand-built graphs, so resolution is
    /// tested without a network.
    struct FakeSource {
        versions: BTreeMap<String, Vec<SourceVersion>>,
        titles: BTreeMap<String, String>,
    }

    impl FakeSource {
        fn new() -> Self {
            Self {
                versions: BTreeMap::new(),
                titles: BTreeMap::new(),
            }
        }

        fn with(mut self, title: &str, version: SourceVersion) -> Self {
            self.titles
                .insert(version.project_id.clone(), title.to_string());
            self.versions
                .entry(version.project_id.clone())
                .or_default()
                .push(version);
            self
        }
    }

    impl ModSource for FakeSource {
        fn id(&self) -> SourceId {
            SourceId::Modrinth
        }

        async fn search(&self, query: &SearchQuery) -> AppResult<crate::mods::SearchPage> {
            Ok(crate::mods::SearchPage {
                projects: Vec::new(),
                total: Some(0),
                offset: query.page_offset(),
                limit: query.page_size(),
            })
        }

        async fn categories(
            &self,
            _content_type: crate::mods::ContentType,
        ) -> AppResult<Vec<crate::mods::Category>> {
            Ok(Vec::new())
        }

        async fn project(&self, project_id: &str) -> AppResult<Project> {
            Ok(Project {
                updated: None,
                content_type: Some(crate::mods::ContentType::Mod),
                downloadable: true,
                source: SourceId::Modrinth,
                id: project_id.to_string(),
                slug: project_id.to_string(),
                title: self
                    .titles
                    .get(project_id)
                    .cloned()
                    .unwrap_or_else(|| project_id.to_string()),
                description: String::new(),
                author: None,
                downloads: None,
                icon_url: None,
                page_url: None,
                categories: vec![],
                loaders: vec![],
            })
        }

        async fn versions(
            &self,
            project_id: &str,
            _filter: &VersionFilter,
        ) -> AppResult<Vec<SourceVersion>> {
            Ok(self.versions.get(project_id).cloned().unwrap_or_default())
        }

        async fn version(&self, version_id: &str) -> AppResult<SourceVersion> {
            self.versions
                .values()
                .flatten()
                .find(|version| version.id == version_id)
                .cloned()
                .ok_or_else(|| AppError::Other(format!("no version {version_id}")))
        }
    }

    fn version(project: &str, id: &str, dependencies: Vec<Dependency>) -> SourceVersion {
        SourceVersion {
            source: SourceId::Modrinth,
            id: id.to_string(),
            project_id: project.to_string(),
            name: format!("{project} {id}"),
            version_number: id.to_string(),
            channel: "release".to_string(),
            published: Some("2026-01-01T00:00:00Z".to_string()),
            game_versions: vec!["1.21.4".to_string()],
            loaders: vec!["fabric".to_string()],
            files: vec![SourceFile {
                url: format!("https://cdn.modrinth.com/{project}-{id}.jar"),
                file_name: format!("{project}-{id}.jar"),
                sha1: None,
                sha512: Some("0".repeat(128)),
                size: Some(1_000),
                primary: true,
            }],
            dependencies,
        }
    }

    fn required(project: &str) -> Dependency {
        Dependency {
            kind: DependencyKind::Required,
            project_id: Some(project.to_string()),
            version_id: None,
        }
    }

    fn index() -> VersionIndex {
        let body = std::fs::read_to_string(
            std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("tests/fixtures/vanilla_version_manifest_v2.json"),
        )
        .unwrap();
        VersionIndex::from_entries(
            crate::providers::vanilla::parse_manifest_entries(&body).unwrap(),
        )
    }

    #[tokio::test]
    async fn required_dependencies_are_followed_recursively() {
        let source = FakeSource::new()
            .with("Waystones", version("waystones", "w1", vec![required("balm")]))
            .with("Balm", version("balm", "b1", vec![required("fabric-api")]))
            .with("Fabric API", version("fabric-api", "f1", vec![]));

        let root = source.version("w1").await.unwrap();
        let plan = plan(
            &source,
            root,
            Loader::Fabric,
            "1.21.4",
            &index(),
            &Installed::default(),
        )
        .await
        .unwrap();

        let titles: Vec<&str> = plan
            .install
            .iter()
            .map(|planned| planned.project_title.as_str())
            .collect();
        assert_eq!(titles, vec!["Waystones", "Balm", "Fabric API"]);
        assert_eq!(plan.install[0].depth, 0);
        assert_eq!(plan.install[1].depth, 1);
        assert_eq!(plan.install[2].depth, 2);
        assert_eq!(plan.install[1].required_by.as_deref(), Some("Waystones"));
        assert_eq!(plan.total_size, 3_000);
    }

    #[tokio::test]
    async fn optional_dependencies_are_listed_and_never_installed() {
        let source = FakeSource::new()
            .with(
                "Waystones",
                version(
                    "waystones",
                    "w1",
                    vec![Dependency {
                        kind: DependencyKind::Optional,
                        project_id: Some("journeymap".into()),
                        version_id: None,
                    }],
                ),
            )
            .with("JourneyMap", version("journeymap", "j1", vec![]));

        let root = source.version("w1").await.unwrap();
        let plan = plan(&source, root, Loader::Fabric, "1.21.4", &index(), &Installed::default())
            .await
            .unwrap();

        assert_eq!(plan.install.len(), 1, "only the root is installed");
        assert_eq!(plan.optional.len(), 1);
        assert_eq!(plan.optional[0].project_title, "JourneyMap");
        assert_eq!(plan.optional[0].suggested_by, "Waystones");
    }

    #[tokio::test]
    async fn a_diamond_dependency_is_installed_once() {
        let source = FakeSource::new()
            .with(
                "Root",
                version("root", "r1", vec![required("left"), required("right")]),
            )
            .with("Left", version("left", "l1", vec![required("shared")]))
            .with("Right", version("right", "r2", vec![required("shared")]))
            .with("Shared", version("shared", "s1", vec![]));

        let root = source.version("r1").await.unwrap();
        let plan = plan(&source, root, Loader::Fabric, "1.21.4", &index(), &Installed::default())
            .await
            .unwrap();

        let shared: Vec<_> = plan
            .install
            .iter()
            .filter(|planned| planned.project_id == "shared")
            .collect();
        assert_eq!(shared.len(), 1, "the shared dependency appears once");
        assert_eq!(plan.install.len(), 4);
    }

    #[tokio::test]
    async fn two_versions_of_the_same_project_is_a_refusal_with_both_named() {
        let source = FakeSource::new()
            .with(
                "Root",
                version("root", "r1", vec![
                    Dependency {
                        kind: DependencyKind::Required,
                        project_id: None,
                        version_id: Some("lib-1".into()),
                    },
                    Dependency {
                        kind: DependencyKind::Required,
                        project_id: None,
                        version_id: Some("lib-2".into()),
                    },
                ]),
            )
            .with("Library", version("lib", "lib-1", vec![]))
            .with("Library", version("lib", "lib-2", vec![]));

        let root = source.version("r1").await.unwrap();
        let err = plan(&source, root, Loader::Fabric, "1.21.4", &index(), &Installed::default())
            .await
            .unwrap_err();

        let message = err.to_string();
        assert!(message.contains("conflict"), "{message}");
        assert!(message.contains("lib-1") && message.contains("lib-2"), "{message}");
    }

    #[tokio::test]
    async fn a_dependency_with_no_suitable_build_names_the_project() {
        let source = FakeSource::new()
            .with("Root", version("root", "r1", vec![required("missing")]));

        let root = source.version("r1").await.unwrap();
        let err = plan(&source, root, Loader::Fabric, "1.21.4", &index(), &Installed::default())
            .await
            .unwrap_err();
        assert!(err.to_string().contains("missing"), "{err}");
        assert!(err.to_string().contains("1.21.4"), "{err}");
    }

    #[tokio::test]
    async fn what_is_already_installed_is_marked_and_not_counted_twice() {
        let source = FakeSource::new()
            .with("Root", version("root", "r1", vec![required("balm")]))
            .with("Balm", version("balm", "b1", vec![]));

        let mut installed = Installed::default();
        installed
            .by_project
            .insert("balm".into(), ("b1".into(), "balm-b1.jar".into()));

        let root = source.version("r1").await.unwrap();
        let plan = plan(&source, root, Loader::Fabric, "1.21.4", &index(), &installed)
            .await
            .unwrap();

        let balm = plan
            .install
            .iter()
            .find(|planned| planned.project_id == "balm")
            .unwrap();
        assert!(balm.already_installed);
        assert_eq!(plan.total_size, 1_000, "only the root counts toward the download");
    }

    #[tokio::test]
    async fn a_different_installed_version_of_a_dependency_is_a_conflict() {
        let source = FakeSource::new()
            .with("Root", version("root", "r1", vec![required("balm")]))
            .with("Balm", version("balm", "b2", vec![]));

        let mut installed = Installed::default();
        installed
            .by_project
            .insert("balm".into(), ("b1".into(), "balm-b1.jar".into()));

        let root = source.version("r1").await.unwrap();
        let err = plan(&source, root, Loader::Fabric, "1.21.4", &index(), &installed)
            .await
            .unwrap_err();
        assert!(err.to_string().contains("already installed"), "{err}");
    }

    #[test]
    fn version_selection_prefers_a_release_that_fits_the_instance() {
        let mut beta = version("mod", "beta", vec![]);
        beta.channel = "beta".into();
        let release = version("mod", "release", vec![]);
        let mut wrong_loader = version("mod", "forge-build", vec![]);
        wrong_loader.loaders = vec!["forge".into()];
        let mut wrong_version = version("mod", "old", vec![]);
        wrong_version.game_versions = vec!["1.20.1".into()];

        let versions = vec![beta, release, wrong_loader, wrong_version];
        let picked = pick_version(&versions, Loader::Fabric, "1.21.4", &index()).unwrap();
        assert_eq!(picked.id, "release");
    }

    #[test]
    fn a_beta_is_used_when_there_is_no_release() {
        let mut beta = version("mod", "beta", vec![]);
        beta.channel = "beta".into();
        let picked = pick_version(&[beta], Loader::Fabric, "1.21.4", &index()).unwrap();
        assert_eq!(picked.id, "beta");
    }

    #[test]
    fn nothing_suitable_returns_none_rather_than_a_wrong_build() {
        let wrong = version("mod", "v", vec![]);
        assert!(pick_version(&[wrong], Loader::Paper, "1.21.4", &index()).is_none());
    }

    /// The recorded Modrinth fixtures, resolved end to end through the same
    /// translation the live client uses.
    #[tokio::test]
    async fn the_recorded_waystones_tree_resolves() {
        let read = |name: &str| {
            std::fs::read_to_string(
                std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
                    .join("tests/fixtures")
                    .join(name),
            )
            .unwrap()
        };

        let mut source = FakeSource::new();
        for (title, file) in [
            ("Waystones", "modrinth_versions_waystones.json"),
            ("Fabric API", "modrinth_versions_fabric_api.json"),
            ("Balm", "modrinth_versions_balm.json"),
        ] {
            for version in crate::mods::modrinth::parse_versions(&read(file)).unwrap() {
                source = source.with(title, version);
            }
        }

        let waystones = crate::mods::modrinth::parse_versions(&read(
            "modrinth_versions_waystones.json",
        ))
        .unwrap();
        let root = pick_version(&waystones, Loader::Fabric, "1.21.4", &index())
            .expect("a Waystones build for 1.21.4");

        let plan = plan(&source, root, Loader::Fabric, "1.21.4", &index(), &Installed::default())
            .await
            .unwrap();

        let titles: BTreeSet<&str> = plan
            .install
            .iter()
            .map(|planned| planned.project_title.as_str())
            .collect();
        assert!(titles.contains("Waystones"));
        assert!(titles.contains("Balm"), "{titles:?}");
        assert!(titles.contains("Fabric API"), "{titles:?}");
        assert!(plan.install.iter().all(|planned| planned.file_name.ends_with(".jar")));
        assert!(plan.total_size > 0);
    }
}
