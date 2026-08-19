//! CurseForge, as a second implementation of `ModSource`.
//!
//! Nothing CurseForge-shaped leaves this file: its class ids, its numeric sort
//! fields and its loader enum are translated at the boundary, the same way
//! Modrinth's facets are.
//!
//! Two things about this API shape the code. It needs a key, which the user has
//! to request themselves — so an absent key is a state the UI can explain, not
//! an error. And an author may forbid third-party downloads, in which case the
//! API returns a file with no URL; that project is still worth showing, with
//! "download it from the site" rather than a failure at install time.

use std::time::Duration;

use serde::Deserialize;

use crate::error::{AppError, AppResult};

use super::ratelimit::{self, RateLimiter};
use super::source::{
    Category, ContentType, Dependency, DependencyKind, ModSource, Project, SearchPage, SearchQuery,
    SortBy, SourceFile, SourceId, SourceVersion, VersionFilter,
};

pub const API: &str = "https://api.curseforge.com/v1";
/// Where a user gets a key, shown next to the field in Settings.
pub const KEY_URL: &str = "https://console.curseforge.com/";
/// The settings key holding it.
pub const KEY_SETTING: &str = "curseforge_api_key";
/// Minecraft.
pub const GAME_ID: i64 = 432;

/// CurseForge's class ids for the kinds of content this app browses.
pub fn class_id(content_type: ContentType) -> i64 {
    match content_type {
        ContentType::Mod => 6,
        ContentType::Plugin => 5,
        ContentType::Modpack => 4471,
        ContentType::DataPack => 6945,
        ContentType::ResourcePack => 12,
        ContentType::Shader => 6552,
    }
}

/// CurseForge's `sortField` numbers.
pub fn sort_field(sort: SortBy) -> i64 {
    match sort {
        SortBy::Relevance => 1,
        SortBy::Popularity => 2,
        SortBy::Downloads => 6,
        SortBy::RecentlyUpdated => 3,
        SortBy::Newest => 11,
    }
}

/// CurseForge's `modLoaderType` numbers. `None` for loaders it does not model.
pub fn mod_loader_type(loader: &str) -> Option<i64> {
    match loader {
        "forge" => Some(1),
        "fabric" => Some(4),
        "quilt" => Some(5),
        "neoforge" => Some(6),
        _ => None,
    }
}

pub fn search_url(query: &SearchQuery) -> String {
    let mut url = format!(
        "{API}/mods/search?gameId={GAME_ID}&classId={}&sortField={}&sortOrder=desc&pageSize={}&index={}",
        class_id(query.content_type),
        sort_field(query.sort),
        query.page_size(),
        query.page_offset()
    );

    if !query.text.trim().is_empty() {
        url.push_str(&format!("&searchFilter={}", urlencode(query.text.trim())));
    }
    // CurseForge takes one loader and one game version, not a set.
    if let Some(loader) = query.loaders.iter().find_map(|loader| mod_loader_type(loader)) {
        url.push_str(&format!("&modLoaderType={loader}"));
    }
    if let Some(version) = query.game_versions.first() {
        url.push_str(&format!("&gameVersion={}", urlencode(version)));
    }
    if let Some(category) = query.categories.first() {
        url.push_str(&format!("&categoryId={}", urlencode(category)));
    }
    url
}

pub fn categories_url(content_type: ContentType) -> String {
    format!(
        "{API}/categories?gameId={GAME_ID}&classId={}",
        class_id(content_type)
    )
}

fn urlencode(value: &str) -> String {
    value
        .bytes()
        .map(|byte| match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                (byte as char).to_string()
            }
            b' ' => "%20".to_string(),
            other => format!("%{other:02X}"),
        })
        .collect()
}

/// The client. Holds the key, so a key change means a new client.
pub struct CurseForge {
    client: reqwest::Client,
    api_key: String,
    limiter: std::sync::Arc<RateLimiter>,
}

impl CurseForge {
    pub fn new(api_key: String, limiter: std::sync::Arc<RateLimiter>) -> AppResult<Self> {
        let client = reqwest::Client::builder()
            .user_agent(super::modrinth::user_agent())
            .timeout(Duration::from_secs(30))
            .build()
            .map_err(|e| AppError::Network(format!("could not start the CurseForge client: {e}")))?;
        Ok(Self {
            client,
            api_key,
            limiter,
        })
    }

    async fn get<T: serde::de::DeserializeOwned>(&self, url: &str) -> AppResult<T> {
        let host = ratelimit::host_of(url);
        self.limiter.acquire(&host).await;

        let response = self
            .client
            .get(url)
            .header("x-api-key", &self.api_key)
            .header("Accept", "application/json")
            .send()
            .await
            .map_err(|e| crate::error::from_reqwest(url, &e))?;

        if response.status() == reqwest::StatusCode::TOO_MANY_REQUESTS {
            let wait = ratelimit::retry_after(response.headers());
            self.limiter
                .observe_throttled(&host, wait, std::time::Instant::now());
            return Err(AppError::RateLimited {
                host,
                retry_after_s: wait.as_secs(),
            });
        }
        // A key that is wrong or has been revoked is the one failure a user can
        // do something about, so it says so rather than "403".
        if matches!(
            response.status(),
            reqwest::StatusCode::UNAUTHORIZED | reqwest::StatusCode::FORBIDDEN
        ) {
            return Err(AppError::Other(
                "CurseForge rejected the API key. Check it in Settings, or request a new one."
                    .into(),
            ));
        }
        if !response.status().is_success() {
            return Err(AppError::Network(format!(
                "{url} returned {}",
                response.status()
            )));
        }

        let body = response
            .text()
            .await
            .map_err(|e| AppError::Network(format!("{url} returned an unreadable body: {e}")))?;
        parse_body(&body, url)
    }
}

/// Kept out of the client so the shapes can be tested without a key.
pub fn parse_body<T: serde::de::DeserializeOwned>(body: &str, url: &str) -> AppResult<T> {
    serde_json::from_str(body)
        .map_err(|e| AppError::Other(format!("{url} returned JSON this build cannot read: {e}")))
}

impl ModSource for CurseForge {
    fn id(&self) -> SourceId {
        SourceId::CurseForge
    }

    async fn search(&self, query: &SearchQuery) -> AppResult<SearchPage> {
        let response: SearchResponse = self.get(&search_url(query)).await?;
        Ok(to_page(response, query))
    }

    async fn categories(&self, content_type: ContentType) -> AppResult<Vec<Category>> {
        let response: CategoriesResponse = self.get(&categories_url(content_type)).await?;
        Ok(to_categories(response))
    }

    async fn project(&self, project_id: &str) -> AppResult<Project> {
        let response: ModResponse = self.get(&format!("{API}/mods/{project_id}")).await?;
        Ok(to_project(response.data))
    }

    async fn versions(
        &self,
        project_id: &str,
        filter: &VersionFilter,
    ) -> AppResult<Vec<SourceVersion>> {
        let mut url = format!("{API}/mods/{project_id}/files?pageSize=50");
        if let Some(version) = filter.game_versions.first() {
            url.push_str(&format!("&gameVersion={}", urlencode(version)));
        }
        if let Some(loader) = filter.loaders.iter().find_map(|loader| mod_loader_type(loader)) {
            url.push_str(&format!("&modLoaderType={loader}"));
        }

        let response: FilesResponse = self.get(&url).await?;
        Ok(response.data.into_iter().map(to_version).collect())
    }

    async fn version(&self, version_id: &str) -> AppResult<SourceVersion> {
        // CurseForge addresses a file by project and file together; callers
        // hold "<project>/<file>" from `versions`.
        let (project_id, file_id) = version_id.split_once('/').ok_or_else(|| {
            AppError::Other(format!("{version_id} is not a CurseForge file reference"))
        })?;
        let response: FileResponse = self
            .get(&format!("{API}/mods/{project_id}/files/{file_id}"))
            .await?;
        Ok(to_version(response.data))
    }
}

// --- The API's own shapes, private to this file ----------------------------

#[derive(Debug, Deserialize)]
pub struct SearchResponse {
    data: Vec<ApiMod>,
    pagination: Option<Pagination>,
}

#[derive(Debug, Deserialize)]
struct Pagination {
    #[serde(rename = "totalCount")]
    total_count: Option<i64>,
}

#[derive(Debug, Deserialize)]
struct ModResponse {
    data: ApiMod,
}

#[derive(Debug, Deserialize)]
pub struct CategoriesResponse {
    data: Vec<ApiCategory>,
}

#[derive(Debug, Deserialize)]
struct ApiCategory {
    id: i64,
    name: String,
    #[serde(rename = "isClass", default)]
    is_class: bool,
}

#[derive(Debug, Deserialize)]
struct ApiMod {
    id: i64,
    name: String,
    slug: String,
    summary: Option<String>,
    #[serde(rename = "downloadCount")]
    download_count: Option<f64>,
    #[serde(rename = "dateModified")]
    date_modified: Option<String>,
    #[serde(rename = "classId")]
    class_id: Option<i64>,
    /// False when the author forbids downloads through the API.
    #[serde(rename = "allowModDistribution")]
    allow_mod_distribution: Option<bool>,
    logo: Option<ApiLogo>,
    links: Option<ApiLinks>,
    #[serde(default)]
    authors: Vec<ApiAuthor>,
    #[serde(default)]
    categories: Vec<ApiCategoryRef>,
    #[serde(rename = "latestFilesIndexes", default)]
    latest_files_indexes: Vec<ApiFileIndex>,
}

#[derive(Debug, Deserialize)]
struct ApiLogo {
    url: Option<String>,
}

#[derive(Debug, Deserialize)]
struct ApiLinks {
    #[serde(rename = "websiteUrl")]
    website_url: Option<String>,
}

#[derive(Debug, Deserialize)]
struct ApiAuthor {
    name: String,
}

#[derive(Debug, Deserialize)]
struct ApiCategoryRef {
    name: String,
}

#[derive(Debug, Deserialize)]
struct ApiFileIndex {
    #[serde(rename = "modLoader")]
    mod_loader: Option<i64>,
}

#[derive(Debug, Deserialize)]
pub struct FilesResponse {
    data: Vec<ApiFile>,
}

#[derive(Debug, Deserialize)]
struct FileResponse {
    data: ApiFile,
}

#[derive(Debug, Deserialize)]
struct ApiFile {
    id: i64,
    #[serde(rename = "modId")]
    mod_id: i64,
    #[serde(rename = "displayName")]
    display_name: String,
    #[serde(rename = "fileName")]
    file_name: String,
    #[serde(rename = "releaseType")]
    release_type: Option<i64>,
    #[serde(rename = "fileDate")]
    file_date: Option<String>,
    #[serde(rename = "downloadUrl")]
    download_url: Option<String>,
    #[serde(rename = "fileLength")]
    file_length: Option<u64>,
    #[serde(default)]
    hashes: Vec<ApiHash>,
    #[serde(rename = "gameVersions", default)]
    game_versions: Vec<String>,
    #[serde(default)]
    dependencies: Vec<ApiDependency>,
}

#[derive(Debug, Deserialize)]
struct ApiHash {
    value: String,
    algo: i64,
}

#[derive(Debug, Deserialize)]
struct ApiDependency {
    #[serde(rename = "modId")]
    mod_id: i64,
    #[serde(rename = "relationType")]
    relation_type: i64,
}

// --- Translation into the neutral types ------------------------------------

pub fn to_page(response: SearchResponse, query: &SearchQuery) -> SearchPage {
    SearchPage {
        total: response.pagination.and_then(|page| page.total_count),
        projects: response.data.into_iter().map(to_project).collect(),
        offset: query.page_offset(),
        limit: query.page_size(),
    }
}

pub fn to_categories(response: CategoriesResponse) -> Vec<Category> {
    let mut categories: Vec<Category> = response
        .data
        .into_iter()
        // The class itself ("Mods") is in the same list and is not a filter.
        .filter(|category| !category.is_class)
        .map(|category| Category {
            id: category.id.to_string(),
            name: category.name,
        })
        .collect();
    categories.sort_by(|a, b| a.name.cmp(&b.name));
    categories
}

fn to_project(item: ApiMod) -> Project {
    let content_type = match item.class_id {
        Some(6) => Some(ContentType::Mod),
        Some(5) => Some(ContentType::Plugin),
        Some(4471) => Some(ContentType::Modpack),
        Some(6945) => Some(ContentType::DataPack),
        Some(12) => Some(ContentType::ResourcePack),
        Some(6552) => Some(ContentType::Shader),
        _ => None,
    };

    Project {
        source: SourceId::CurseForge,
        id: item.id.to_string(),
        slug: item.slug,
        title: item.name,
        description: item.summary.unwrap_or_default(),
        author: item.authors.first().map(|author| author.name.clone()),
        downloads: item.download_count.map(|count| count as i64),
        icon_url: item.logo.and_then(|logo| logo.url),
        page_url: item.links.and_then(|links| links.website_url),
        categories: item
            .categories
            .into_iter()
            .map(|category| category.name)
            .collect(),
        loaders: item
            .latest_files_indexes
            .iter()
            .filter_map(|index| loader_name(index.mod_loader?))
            .map(str::to_string)
            .collect::<std::collections::BTreeSet<_>>()
            .into_iter()
            .collect(),
        updated: item.date_modified,
        content_type,
        // `allowModDistribution: false` means the API will hand back files with
        // no URL. Better said on the card than discovered at install time.
        downloadable: item.allow_mod_distribution.unwrap_or(true),
    }
}

fn loader_name(mod_loader: i64) -> Option<&'static str> {
    match mod_loader {
        1 => Some("forge"),
        4 => Some("fabric"),
        5 => Some("quilt"),
        6 => Some("neoforge"),
        _ => None,
    }
}

fn to_version(file: ApiFile) -> SourceVersion {
    let channel = match file.release_type {
        Some(2) => "beta",
        Some(3) => "alpha",
        _ => "release",
    };

    // The loader tags travel in `gameVersions` alongside the Minecraft ones.
    let (loaders, game_versions): (Vec<String>, Vec<String>) = file
        .game_versions
        .into_iter()
        .partition(|value| is_loader_tag(value));

    SourceVersion {
        source: SourceId::CurseForge,
        // A file is addressed by project and file together, and callers keep
        // this opaque — which is exactly what the trait promises.
        id: format!("{}/{}", file.mod_id, file.id),
        project_id: file.mod_id.to_string(),
        name: file.display_name,
        version_number: file.file_name.clone(),
        channel: channel.to_string(),
        published: file.file_date,
        game_versions,
        loaders: loaders
            .into_iter()
            .map(|loader| loader.to_ascii_lowercase())
            .collect(),
        files: match file.download_url {
            Some(url) => vec![SourceFile {
                url,
                file_name: file.file_name,
                sha1: hash_of(&file.hashes, 1),
                sha512: None,
                size: file.file_length,
                primary: true,
            }],
            // No URL: the author forbade third-party downloads. An empty file
            // list is what `primary_file()` already treats as "nothing to
            // install", and the project carries the explanation.
            None => Vec::new(),
        },
        dependencies: file
            .dependencies
            .into_iter()
            .map(|dependency| Dependency {
                kind: dependency_kind(dependency.relation_type),
                project_id: Some(dependency.mod_id.to_string()),
                version_id: None,
            })
            .collect(),
    }
}

/// CurseForge hash algorithms: 1 is SHA-1, 2 is MD5.
fn hash_of(hashes: &[ApiHash], algo: i64) -> Option<String> {
    hashes
        .iter()
        .find(|hash| hash.algo == algo)
        .map(|hash| hash.value.to_ascii_lowercase())
}

fn is_loader_tag(value: &str) -> bool {
    matches!(
        value.to_ascii_lowercase().as_str(),
        "forge" | "fabric" | "quilt" | "neoforge" | "bukkit" | "spigot" | "paper" | "folia"
    )
}

/// CurseForge relation types: 3 is required, 2 optional, 5 incompatible,
/// 1 embedded, 4 tool, 6 include.
fn dependency_kind(relation: i64) -> DependencyKind {
    match relation {
        3 => DependencyKind::Required,
        5 => DependencyKind::Incompatible,
        1 => DependencyKind::Embedded,
        // Anything else is listed and never installed on its own, which is the
        // safe reading of a relation this build does not know.
        _ => DependencyKind::Optional,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture(name: &str) -> String {
        std::fs::read_to_string(
            std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("tests")
                .join("fixtures")
                .join(name),
        )
        .unwrap_or_else(|e| panic!("fixture {name} is missing: {e}"))
    }

    #[test]
    fn the_search_url_carries_class_sort_loader_and_page() {
        let query = SearchQuery {
            text: "just enough items".into(),
            loaders: vec!["fabric".into()],
            game_versions: vec!["1.21.4".into()],
            limit: Some(20),
            offset: Some(40),
            sort: SortBy::Downloads,
            categories: vec!["423".into()],
            content_type: ContentType::Mod,
        };

        let url = search_url(&query);
        assert!(url.starts_with(API));
        assert!(url.contains("gameId=432"), "{url}");
        assert!(url.contains("classId=6"), "{url}");
        assert!(url.contains("sortField=6"), "downloads: {url}");
        assert!(url.contains("sortOrder=desc"), "{url}");
        assert!(url.contains("pageSize=20"), "{url}");
        assert!(url.contains("index=40"), "{url}");
        assert!(url.contains("modLoaderType=4"), "fabric: {url}");
        assert!(url.contains("gameVersion=1.21.4"), "{url}");
        assert!(url.contains("categoryId=423"), "{url}");
        assert!(url.contains("searchFilter=just%20enough%20items"), "{url}");
    }

    #[test]
    fn every_sort_and_class_maps_onto_curseforges_own_numbers() {
        assert_eq!(sort_field(SortBy::Relevance), 1);
        assert_eq!(sort_field(SortBy::Popularity), 2);
        assert_eq!(sort_field(SortBy::Downloads), 6);
        assert_eq!(sort_field(SortBy::RecentlyUpdated), 3);
        assert_eq!(sort_field(SortBy::Newest), 11);

        assert_eq!(class_id(ContentType::Mod), 6);
        assert_eq!(class_id(ContentType::Plugin), 5);
        assert_eq!(class_id(ContentType::Modpack), 4471);
        assert_eq!(class_id(ContentType::ResourcePack), 12);
        assert_eq!(class_id(ContentType::Shader), 6552);

        assert_eq!(mod_loader_type("fabric"), Some(4));
        assert_eq!(mod_loader_type("neoforge"), Some(6));
        assert_eq!(mod_loader_type("paper"), None, "not a loader CurseForge models");
    }

    #[test]
    fn a_search_response_becomes_projects_and_a_total() {
        let response: SearchResponse =
            parse_body(&fixture("curseforge_search.json"), "test").unwrap();
        let query = SearchQuery {
            limit: Some(20),
            offset: Some(0),
            ..SearchQuery::default()
        };
        let page = to_page(response, &query);

        assert_eq!(page.total, Some(2), "pagination drives the pager");
        assert_eq!(page.limit, 20);
        assert_eq!(page.projects.len(), 2);

        let first = &page.projects[0];
        assert_eq!(first.source, SourceId::CurseForge);
        assert_eq!(first.title, "Just Enough Items");
        assert_eq!(first.author.as_deref(), Some("mezz"));
        assert_eq!(first.downloads, Some(412_000_000));
        assert!(first.icon_url.as_deref().unwrap().ends_with(".png"));
        assert_eq!(first.content_type, Some(ContentType::Mod));
        assert!(first.loaders.contains(&"forge".to_string()));
        assert!(first.loaders.contains(&"neoforge".to_string()));
        assert!(first.updated.is_some());
        assert!(first.downloadable, "this one may be downloaded");
    }

    #[test]
    fn a_project_that_forbids_api_downloads_says_so_rather_than_failing_later() {
        let response: SearchResponse =
            parse_body(&fixture("curseforge_search.json"), "test").unwrap();
        let page = to_page(response, &SearchQuery::default());

        let restricted = &page.projects[1];
        assert_eq!(restricted.title, "Restricted Mod");
        assert!(!restricted.downloadable);
        // The link to its page is what the UI offers instead of an install.
        assert!(restricted.page_url.as_deref().unwrap().starts_with("https://"));
    }

    #[test]
    fn a_file_with_no_url_produces_a_version_with_nothing_to_install() {
        let response: FilesResponse =
            parse_body(&fixture("curseforge_files.json"), "test").unwrap();
        let versions: Vec<SourceVersion> = response.data.into_iter().map(to_version).collect();

        let downloadable = &versions[0];
        assert_eq!(downloadable.channel, "release");
        assert_eq!(downloadable.id, "238222/5300000", "project and file together");
        assert_eq!(downloadable.project_id, "238222");
        assert!(downloadable.game_versions.contains(&"1.21.4".to_string()));
        assert!(downloadable.loaders.contains(&"neoforge".to_string()));
        assert_eq!(
            downloadable.primary_file().unwrap().sha1.as_deref(),
            Some("a1b2c3d4e5f60718293a4b5c6d7e8f9012345678")
        );

        let restricted = &versions[1];
        assert!(restricted.files.is_empty(), "no URL means nothing to install");
        assert!(restricted.primary_file().is_none());
        assert_eq!(restricted.channel, "beta");
    }

    #[test]
    fn dependencies_and_channels_translate() {
        let response: FilesResponse =
            parse_body(&fixture("curseforge_files.json"), "test").unwrap();
        let version = to_version(response.data.into_iter().next().unwrap());

        let kinds: Vec<DependencyKind> = version
            .dependencies
            .iter()
            .map(|dependency| dependency.kind)
            .collect();
        assert!(kinds.contains(&DependencyKind::Required));
        assert!(kinds.contains(&DependencyKind::Optional));
        assert_eq!(dependency_kind(5), DependencyKind::Incompatible);
        assert_eq!(dependency_kind(1), DependencyKind::Embedded);
        assert_eq!(dependency_kind(99), DependencyKind::Optional, "unknown is safe");
    }

    #[test]
    fn categories_drop_the_class_itself() {
        let response: CategoriesResponse =
            parse_body(&fixture("curseforge_categories.json"), "test").unwrap();
        let categories = to_categories(response);

        let names: Vec<&str> = categories.iter().map(|c| c.name.as_str()).collect();
        assert!(!names.contains(&"Mods"), "the class is not a filter: {names:?}");
        assert!(names.contains(&"Adventure and RPG"));
        assert!(names.contains(&"World Gen"));
        assert_eq!(names, {
            let mut sorted = names.clone();
            sorted.sort();
            sorted
        });
        assert!(categories.iter().all(|category| category.id.parse::<i64>().is_ok()));
    }

    #[test]
    fn the_key_url_is_where_a_user_actually_gets_one() {
        assert!(KEY_URL.starts_with("https://console.curseforge.com"));
        assert_eq!(KEY_SETTING, "curseforge_api_key");
    }
}
