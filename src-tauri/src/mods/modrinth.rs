//! Modrinth, the one [`ModSource`] implementation today.
//!
//! Everything Modrinth-shaped stays inside this file: its facet syntax, its
//! field names, its id format. Callers get [`Project`], [`SourceVersion`] and
//! [`Dependency`], which is what makes CurseForge a second implementation rather
//! than a second code path.
//!
//! Modrinth asks API clients for a User-Agent that identifies the project and a
//! contact, and publishes a request budget in headers; both are honoured here.

use serde::Deserialize;

use crate::error::{AppError, AppResult};
use crate::mcversion::VersionIndex;

use super::ratelimit::{self, RateLimiter};
use super::source::{
    Dependency, DependencyKind, ModSource, Project, SearchQuery, SourceFile, SourceId,
    SourceVersion, VersionFilter,
};

pub const API: &str = "https://api.modrinth.com/v2";

/// Hosts a Modrinth download may come from. A `.mrpack` naming anything else is
/// rejected rather than fetched.
pub const ALLOWED_HOSTS: &[&str] = &[
    "cdn.modrinth.com",
    "github.com",
    "raw.githubusercontent.com",
    "gitlab.com",
];

/// Identifies this app to Modrinth, as their API docs require: project, version
/// and a contact address.
pub fn user_agent() -> String {
    format!(
        "mc-server-manager/{} (+https://github.com/mc-server-manager/mc-server-manager; desktop server manager)",
        env!("CARGO_PKG_VERSION")
    )
}

/// True when a URL points at a host Modrinth allows packs to reference.
pub fn host_allowed(url: &str) -> bool {
    let host = ratelimit::host_of(url);
    url.starts_with("https://")
        && ALLOWED_HOSTS
            .iter()
            .any(|allowed| host == *allowed || host.ends_with(&format!(".{allowed}")))
}

pub struct Modrinth {
    client: reqwest::Client,
    limiter: std::sync::Arc<RateLimiter>,
}

impl Modrinth {
    pub fn new(limiter: std::sync::Arc<RateLimiter>) -> AppResult<Self> {
        let client = reqwest::Client::builder()
            .user_agent(user_agent())
            .timeout(std::time::Duration::from_secs(30))
            .build()
            .map_err(|e| AppError::Network(format!("could not start the Modrinth client: {e}")))?;
        Ok(Self { client, limiter })
    }

    /// One GET, respecting the shared budget and retrying once after a 429.
    async fn get<T: serde::de::DeserializeOwned>(&self, url: &str) -> AppResult<T> {
        let host = ratelimit::host_of(url);

        for attempt in 0..2 {
            self.limiter.acquire(&host).await;

            let response = self
                .client
                .get(url)
                .send()
                .await
                .map_err(|e| AppError::Network(format!("{url} could not be reached: {e}")))?;

            let headers = response.headers().clone();
            let now = std::time::Instant::now();

            if response.status() == reqwest::StatusCode::TOO_MANY_REQUESTS {
                let wait = ratelimit::retry_after(&headers);
                self.limiter.observe_throttled(&host, wait, now);
                if attempt == 0 {
                    tracing::warn!(host = %host, seconds = wait.as_secs(), "Modrinth throttled us");
                    continue;
                }
                return Err(AppError::Network(format!(
                    "Modrinth is rate limiting this app; try again in {} seconds",
                    wait.as_secs()
                )));
            }

            if let Some(budget) = ratelimit::budget_from_headers(&headers) {
                self.limiter.observe(&host, budget, now);
            }

            let status = response.status();
            if status == reqwest::StatusCode::NOT_FOUND {
                return Err(AppError::Other(format!("{url} was not found on Modrinth")));
            }
            if !status.is_success() {
                return Err(AppError::Network(format!(
                    "{url} answered {} {}",
                    status.as_u16(),
                    status.canonical_reason().unwrap_or("")
                )));
            }

            let body = response
                .text()
                .await
                .map_err(|e| AppError::Network(format!("{url} returned an unreadable body: {e}")))?;
            return parse(&body, url);
        }

        Err(AppError::Network("Modrinth kept rate limiting this app".into()))
    }
}

fn parse<T: serde::de::DeserializeOwned>(body: &str, url: &str) -> AppResult<T> {
    serde_json::from_str(body)
        .map_err(|e| AppError::Other(format!("{url} returned JSON this build cannot read: {e}")))
}

// --- Modrinth's own shapes, private to this module -------------------------

#[derive(Debug, Deserialize)]
struct SearchResponse {
    hits: Vec<Hit>,
}

#[derive(Debug, Deserialize)]
struct Hit {
    project_id: String,
    slug: String,
    title: String,
    description: String,
    author: Option<String>,
    downloads: Option<i64>,
    icon_url: Option<String>,
    #[serde(default)]
    categories: Vec<String>,
}

#[derive(Debug, Deserialize)]
struct ApiProject {
    id: String,
    slug: String,
    title: String,
    description: String,
    #[serde(default)]
    categories: Vec<String>,
    #[serde(default)]
    loaders: Vec<String>,
    icon_url: Option<String>,
    downloads: Option<i64>,
}

#[derive(Debug, Deserialize)]
struct ApiVersion {
    id: String,
    project_id: String,
    name: String,
    version_number: String,
    version_type: String,
    date_published: Option<String>,
    #[serde(default)]
    game_versions: Vec<String>,
    #[serde(default)]
    loaders: Vec<String>,
    #[serde(default)]
    dependencies: Vec<ApiDependency>,
    #[serde(default)]
    files: Vec<ApiFile>,
}

#[derive(Debug, Deserialize)]
struct ApiDependency {
    project_id: Option<String>,
    version_id: Option<String>,
    dependency_type: String,
}

#[derive(Debug, Deserialize)]
struct ApiFile {
    url: String,
    filename: String,
    #[serde(default)]
    primary: bool,
    size: Option<u64>,
    #[serde(default)]
    hashes: std::collections::BTreeMap<String, String>,
}

// --- Translation into the neutral types ------------------------------------

fn to_project(hit: Hit) -> Project {
    Project {
        source: SourceId::Modrinth,
        page_url: Some(format!("https://modrinth.com/mod/{}", hit.slug)),
        id: hit.project_id,
        slug: hit.slug,
        title: hit.title,
        description: hit.description,
        author: hit.author,
        downloads: hit.downloads,
        icon_url: hit.icon_url,
        loaders: hit
            .categories
            .iter()
            .filter(|category| is_loader(category))
            .cloned()
            .collect(),
        categories: hit.categories,
    }
}

/// Modrinth mixes loader tags into `categories`; only some of them are loaders.
fn is_loader(category: &str) -> bool {
    matches!(
        category,
        "fabric" | "forge" | "neoforge" | "quilt" | "paper" | "spigot" | "bukkit" | "purpur" | "folia"
    )
}

fn project_from_api(project: ApiProject) -> Project {
    Project {
        source: SourceId::Modrinth,
        page_url: Some(format!("https://modrinth.com/mod/{}", project.slug)),
        id: project.id,
        slug: project.slug,
        title: project.title,
        description: project.description,
        author: None,
        downloads: project.downloads,
        icon_url: project.icon_url,
        categories: project.categories,
        loaders: project.loaders,
    }
}

fn dependency_kind(kind: &str) -> DependencyKind {
    match kind {
        "required" => DependencyKind::Required,
        "incompatible" => DependencyKind::Incompatible,
        "embedded" => DependencyKind::Embedded,
        // "optional" and anything new Modrinth invents are treated as optional,
        // which is the safe reading: listed, never auto-installed.
        _ => DependencyKind::Optional,
    }
}

fn to_version(version: ApiVersion) -> SourceVersion {
    SourceVersion {
        source: SourceId::Modrinth,
        id: version.id,
        project_id: version.project_id,
        name: version.name,
        version_number: version.version_number,
        channel: version.version_type,
        published: version.date_published,
        game_versions: version.game_versions,
        loaders: version.loaders,
        files: version
            .files
            .into_iter()
            .map(|file| SourceFile {
                url: file.url,
                file_name: file.filename,
                sha1: file.hashes.get("sha1").cloned(),
                sha512: file.hashes.get("sha512").cloned(),
                size: file.size,
                primary: file.primary,
            })
            .collect(),
        dependencies: version
            .dependencies
            .into_iter()
            .map(|dependency| Dependency {
                kind: dependency_kind(&dependency.dependency_type),
                project_id: dependency.project_id,
                version_id: dependency.version_id,
            })
            .collect(),
    }
}

/// Modrinth's facet syntax, built from the neutral query.
pub fn search_url(query: &SearchQuery) -> String {
    let mut facets: Vec<String> = Vec::new();
    if !query.loaders.is_empty() {
        facets.push(or_facet("categories", &query.loaders));
    }
    if !query.game_versions.is_empty() {
        facets.push(or_facet("versions", &query.game_versions));
    }

    let mut url = format!(
        "{API}/search?limit={}&offset={}",
        query.limit.unwrap_or(20).clamp(1, 100),
        query.offset.unwrap_or(0)
    );
    if !query.text.trim().is_empty() {
        url.push_str(&format!("&query={}", urlencode(query.text.trim())));
    }
    if !facets.is_empty() {
        url.push_str(&format!("&facets={}", urlencode(&format!("[{}]", facets.join(",")))));
    }
    url
}

/// One facet group: any of these values matches.
fn or_facet(name: &str, values: &[String]) -> String {
    let items: Vec<String> = values
        .iter()
        .map(|value| format!("\"{name}:{value}\""))
        .collect();
    format!("[{}]", items.join(","))
}

pub fn versions_url(project_id: &str, filter: &VersionFilter) -> String {
    let mut url = format!("{API}/project/{project_id}/version");
    let mut parameters: Vec<String> = Vec::new();
    if !filter.loaders.is_empty() {
        parameters.push(format!("loaders={}", urlencode(&json_array(&filter.loaders))));
    }
    if !filter.game_versions.is_empty() {
        parameters.push(format!(
            "game_versions={}",
            urlencode(&json_array(&filter.game_versions))
        ));
    }
    if !parameters.is_empty() {
        url.push('?');
        url.push_str(&parameters.join("&"));
    }
    url
}

fn json_array(values: &[String]) -> String {
    let items: Vec<String> = values.iter().map(|value| format!("\"{value}\"")).collect();
    format!("[{}]", items.join(","))
}

/// Percent-encoding for the characters that appear in facets and queries.
fn urlencode(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    for byte in value.bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(byte as char)
            }
            b' ' => out.push_str("%20"),
            other => out.push_str(&format!("%{other:02X}")),
        }
    }
    out
}

/// Newest first by release chronology of the *game* versions they support,
/// falling back to publication date. String comparison would put 1.21.11 above
/// 26.2, which is exactly the trap the version index exists to avoid.
pub fn sort_versions(versions: &mut [SourceVersion], index: &VersionIndex) {
    versions.sort_by(|a, b| {
        let newest = |version: &SourceVersion| -> Option<String> {
            let mut ids = version.game_versions.clone();
            index.sort_newest_first(&mut ids);
            ids.first().cloned()
        };

        match (newest(a), newest(b)) {
            (Some(x), Some(y)) => index
                .compare(&y, &x)
                .then_with(|| b.published.cmp(&a.published)),
            _ => b.published.cmp(&a.published),
        }
    });
}

impl ModSource for Modrinth {
    fn id(&self) -> SourceId {
        SourceId::Modrinth
    }

    async fn search(&self, query: &SearchQuery) -> AppResult<Vec<Project>> {
        let response: SearchResponse = self.get(&search_url(query)).await?;
        Ok(response.hits.into_iter().map(to_project).collect())
    }

    async fn project(&self, project_id: &str) -> AppResult<Project> {
        let project: ApiProject = self.get(&format!("{API}/project/{project_id}")).await?;
        Ok(project_from_api(project))
    }

    async fn versions(
        &self,
        project_id: &str,
        filter: &VersionFilter,
    ) -> AppResult<Vec<SourceVersion>> {
        let versions: Vec<ApiVersion> = self.get(&versions_url(project_id, filter)).await?;
        Ok(versions.into_iter().map(to_version).collect())
    }

    async fn version(&self, version_id: &str) -> AppResult<SourceVersion> {
        let version: ApiVersion = self.get(&format!("{API}/version/{version_id}")).await?;
        Ok(to_version(version))
    }
}

/// Parses a recorded version list. Public so tests and the fixture-backed source
/// share exactly the translation the live client uses.
pub fn parse_versions(body: &str) -> AppResult<Vec<SourceVersion>> {
    let versions: Vec<ApiVersion> = parse(body, "versions")?;
    Ok(versions.into_iter().map(to_version).collect())
}

pub fn parse_search(body: &str) -> AppResult<Vec<Project>> {
    let response: SearchResponse = parse(body, "search")?;
    Ok(response.hits.into_iter().map(to_project).collect())
}

pub fn parse_project(body: &str) -> AppResult<Project> {
    Ok(project_from_api(parse(body, "project")?))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture(name: &str) -> String {
        std::fs::read_to_string(
            std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("tests/fixtures")
                .join(name),
        )
        .unwrap()
    }

    #[test]
    fn the_user_agent_identifies_this_app_and_a_contact() {
        let agent = user_agent();
        assert!(agent.starts_with("mc-server-manager/"));
        assert!(agent.contains("https://"), "a contact URL is required: {agent}");
        assert!(agent.contains(env!("CARGO_PKG_VERSION")));
    }

    #[test]
    fn search_urls_carry_loader_and_version_facets() {
        let url = search_url(&SearchQuery {
            text: "fabric api".into(),
            loaders: vec!["fabric".into()],
            game_versions: vec!["1.21.4".into()],
            limit: Some(10),
            offset: None,
        });

        assert!(url.starts_with("https://api.modrinth.com/v2/search?limit=10"));
        assert!(url.contains("query=fabric%20api"));
        // [["categories:fabric"],["versions:1.21.4"]], percent-encoded.
        assert!(url.contains("facets=%5B%5B%22categories%3Afabric%22%5D%2C%5B%22versions%3A1.21.4%22%5D%5D"), "{url}");
    }

    #[test]
    fn a_search_without_filters_asks_for_everything() {
        let url = search_url(&SearchQuery::default());
        assert!(!url.contains("facets"));
        assert!(!url.contains("query="));
    }

    #[test]
    fn version_urls_filter_by_loader_and_game_version() {
        let url = versions_url(
            "AANobbMI",
            &VersionFilter {
                loaders: vec!["fabric".into()],
                game_versions: vec!["1.21.4".into()],
            },
        );
        assert!(url.starts_with("https://api.modrinth.com/v2/project/AANobbMI/version?"));
        assert!(url.contains("loaders=%5B%22fabric%22%5D"));
        assert!(url.contains("game_versions=%5B%221.21.4%22%5D"));
    }

    #[test]
    fn search_results_translate_into_neutral_projects() {
        let projects = parse_search(&fixture("modrinth_search.json")).unwrap();
        assert!(!projects.is_empty());

        let first = &projects[0];
        assert_eq!(first.source, SourceId::Modrinth);
        assert!(!first.id.is_empty());
        assert!(!first.title.is_empty());
        assert!(first.page_url.as_deref().unwrap().starts_with("https://modrinth.com/mod/"));
        // Loader tags are lifted out of Modrinth's mixed category list.
        assert!(first.loaders.iter().all(|loader| is_loader(loader)));
    }

    #[test]
    fn versions_translate_with_files_hashes_and_dependencies() {
        let versions = parse_versions(&fixture("modrinth_versions_waystones.json")).unwrap();
        let version = &versions[0];

        assert_eq!(version.source, SourceId::Modrinth);
        assert!(version.game_versions.contains(&"1.21.4".to_string()));
        assert!(version.loaders.contains(&"fabric".to_string()));

        let file = version.primary_file().expect("a primary file");
        assert!(file.url.starts_with("https://cdn.modrinth.com/"));
        assert_eq!(file.sha512.as_ref().map(|hash| hash.len()), Some(128));
        assert!(file.size.unwrap() > 0);

        // Waystones requires Fabric API and Balm.
        let required: Vec<&Dependency> = version
            .dependencies
            .iter()
            .filter(|dependency| dependency.kind == DependencyKind::Required)
            .collect();
        assert_eq!(required.len(), 2, "{:?}", version.dependencies);
        assert!(required.iter().all(|dependency| dependency.project_id.is_some()));
    }

    #[test]
    fn dependency_kinds_map_conservatively() {
        assert_eq!(dependency_kind("required"), DependencyKind::Required);
        assert_eq!(dependency_kind("optional"), DependencyKind::Optional);
        assert_eq!(dependency_kind("incompatible"), DependencyKind::Incompatible);
        assert_eq!(dependency_kind("embedded"), DependencyKind::Embedded);
        // Anything Modrinth adds later must not be treated as required.
        assert_eq!(dependency_kind("something-new"), DependencyKind::Optional);
    }

    #[test]
    fn projects_translate_from_the_detail_endpoint() {
        let project = parse_project(&fixture("modrinth_project_balm.json")).unwrap();
        assert_eq!(project.source, SourceId::Modrinth);
        assert_eq!(project.id, "MBAkmtvl");
        assert!(!project.loaders.is_empty());
    }

    #[test]
    fn only_modrinths_allowlisted_hosts_are_accepted() {
        assert!(host_allowed("https://cdn.modrinth.com/data/AANobbMI/versions/x/mod.jar"));
        assert!(host_allowed("https://github.com/owner/repo/releases/download/v1/mod.jar"));
        assert!(host_allowed("https://raw.githubusercontent.com/owner/repo/main/mod.jar"));

        assert!(!host_allowed("https://example.com/mod.jar"));
        assert!(!host_allowed("http://cdn.modrinth.com/mod.jar"), "plain HTTP is refused");
        assert!(!host_allowed("https://evil.com/cdn.modrinth.com/mod.jar"));
        assert!(!host_allowed("https://notmodrinth.com/mod.jar"));
    }

    #[test]
    fn versions_sort_by_release_chronology_not_by_string() {
        let index = crate::mcversion::VersionIndex::from_entries(
            crate::providers::vanilla::parse_manifest_entries(&fixture(
                "vanilla_version_manifest_v2.json",
            ))
            .unwrap(),
        );

        let make = |id: &str, game: &str, published: &str| SourceVersion {
            source: SourceId::Modrinth,
            id: id.into(),
            project_id: "p".into(),
            name: id.into(),
            version_number: id.into(),
            channel: "release".into(),
            published: Some(published.into()),
            game_versions: vec![game.into()],
            loaders: vec!["fabric".into()],
            files: vec![],
            dependencies: vec![],
        };

        let mut versions = vec![
            make("for-1.20.4", "1.20.4", "2023-12-08T00:00:00Z"),
            make("for-26.2", "26.2", "2026-08-05T00:00:00Z"),
            make("for-1.21.4", "1.21.4", "2024-12-04T00:00:00Z"),
        ];
        sort_versions(&mut versions, &index);

        let order: Vec<&str> = versions.iter().map(|version| version.id.as_str()).collect();
        assert_eq!(order, vec!["for-26.2", "for-1.21.4", "for-1.20.4"]);
    }
}
