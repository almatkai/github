use crate::api::{number, text, Api};
use chrono::{Duration, Utc};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::collections::HashSet;

pub const PACKAGE_TYPES: &[&str] = &["container", "npm", "maven", "docker", "nuget", "rubygems"];
pub const LIMIT: usize = 220;

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct Repo {
    pub owner: String,
    pub name: String,
}
impl Repo {
    pub fn parse(raw: &str) -> Result<Self, String> {
        let raw = raw.trim();
        let (owner, name) = raw
            .split_once('/')
            .ok_or("Enter a repository as owner/name.")?;
        if !account(owner) || !repo_name(name) {
            return Err("Enter a valid GitHub repository as owner/name.".into());
        }
        Ok(Self {
            owner: owner.into(),
            name: name.into(),
        })
    }
    pub fn full(&self) -> String {
        format!("{}/{}", self.owner, self.name)
    }
    pub fn web(&self) -> String {
        format!("https://github.com/{}", self.full())
    }
    pub fn path(&self) -> [&str; 3] {
        ["repos", &self.owner, &self.name]
    }
    pub fn for_path<'a>(&'a self, suffix: &[&'a str]) -> Vec<&'a str> {
        self.path()
            .into_iter()
            .chain(suffix.iter().copied())
            .collect()
    }
}

pub fn account(raw: &str) -> bool {
    !raw.is_empty()
        && raw.len() <= 100
        && raw
            .bytes()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, b'-' | b'_' | b'[' | b']'))
}
pub fn repo_name(raw: &str) -> bool {
    !raw.is_empty()
        && raw.len() <= 100
        && !matches!(raw, "." | "..")
        && raw
            .bytes()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, b'-' | b'_' | b'.'))
}
pub fn web(raw: &str) -> Option<String> {
    let url = reqwest::Url::parse(raw).ok()?;
    if url.scheme() != "https"
        || url.port_or_known_default() != Some(443)
        || !url.username().is_empty()
        || url.password().is_some()
        || !matches!(
            url.host_str(),
            Some(
                "github.com"
                    | "www.github.com"
                    | "github.dev"
                    | "gist.github.com"
                    | "api.github.com"
            )
        )
    {
        return None;
    }
    Some(url.to_string())
}

pub fn field(raw: &Value, key: &str, max: usize) -> String {
    text(raw, key)
        .chars()
        .filter(|c| !c.is_control() || matches!(c, '\n' | '\t'))
        .take(max)
        .collect()
}
pub fn id(raw: &Value) -> String {
    raw["id"]
        .as_str()
        .map(str::to_owned)
        .or_else(|| raw["id"].as_u64().map(|id| id.to_string()))
        .unwrap_or_default()
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct Row {
    pub id: String,
    pub title: String,
    pub subtitle: String,
    pub url: String,
    pub repo: Option<Repo>,
    pub number: u64,
    pub kind: String,
    pub state: String,
    pub body: String,
    pub detail: Value,
    pub section: String,
}

fn repository_of(raw: &Value) -> Option<Repo> {
    for key in ["full_name", "nameWithOwner"] {
        if let Ok(repo) = Repo::parse(text(raw, key)) {
            return Some(repo);
        }
    }
    let url = text(raw, "repository_url");
    if let Some(raw) = url.strip_prefix("https://api.github.com/repos/") {
        if let Ok(repo) = Repo::parse(raw) {
            return Some(repo);
        }
    }
    for path in [
        &raw["repository"]["full_name"],
        &raw["repository"]["nameWithOwner"],
    ] {
        if let Some(raw) = path.as_str() {
            if let Ok(repo) = Repo::parse(raw) {
                return Some(repo);
            }
        }
    }
    None
}

fn row(raw: &Value, kind: &str, repo: Option<&Repo>, section: &str) -> Option<Row> {
    let id = id(raw);
    if id.is_empty() {
        return None;
    }
    let repository = repo.cloned().or_else(|| repository_of(raw));
    let mut url = web(text(raw, "html_url"))
        .or_else(|| web(text(raw, "url")).filter(|url| !url.starts_with("https://api.github.com/")))
        .unwrap_or_default();
    let n = number(raw, "number").unwrap_or_default();
    let mut title = raw["title"]
        .as_str()
        .map(str::to_owned)
        .unwrap_or_else(|| field(raw, "name", 300));
    let mut subtitle = repository.as_ref().map(Repo::full).unwrap_or_default();
    let mut state = field(raw, "state", 40);
    let mut body = raw["body"]
        .as_str()
        .map(|v| v.chars().take(28_000).collect::<String>())
        .unwrap_or_default();
    let mut detail = json!({});
    match kind {
        "repository" => {
            if let Some(repo) = &repository {
                title = repo.full();
                url = repo.web();
            }
            state = if raw["private"] == true {
                "Private".into()
            } else {
                "Public".into()
            };
            subtitle = format!(
                "{} ? {} stars",
                state,
                raw["stargazers_count"].as_u64().unwrap_or_default()
            );
            body = field(raw, "description", 4000);
            detail =
                json!({"ref":raw["default_branch"],"fork":raw["fork"],"archived":raw["archived"]});
        }
        "issue" | "pull" => {
            if n > 0 {
                subtitle = format!("{} #{} ? {}", subtitle, n, state);
            }
            detail = json!({"comments":raw["comments"],"author":raw["user"]["login"]});
        }
        "notification" => {
            let subject = &raw["subject"];
            title = field(subject, "title", 300);
            state = if raw["unread"] == true {
                "Unread".into()
            } else {
                "Read".into()
            };
            subtitle = format!("{subtitle} ? {}", field(raw, "reason", 40));
            detail = json!({"unread":raw["unread"],"subjectType":subject["type"]});
            if let Some(repo) = &repository {
                let subject_url = text(subject, "url");
                let leaf = subject_url
                    .rsplit('/')
                    .next()
                    .filter(|v| v.bytes().all(|c| c.is_ascii_digit()));
                let destination = match text(subject, "type") {
                    "Issue" => "issues",
                    "PullRequest" => "pull",
                    "Discussion" => "discussions",
                    "Release" => "releases",
                    "CheckSuite" | "WorkflowRun" => "actions",
                    _ => "",
                };
                let number = leaf.and_then(|leaf| leaf.parse::<u64>().ok());
                url = match (destination, number) {
                    ("issues" | "pull" | "discussions", Some(number)) => {
                        format!("{}/{destination}/{number}", repo.web())
                    }
                    ("", _) => repo.web(),
                    _ => format!("{}/{destination}", repo.web()),
                };
            }
            if url.is_empty() {
                url = "https://github.com/notifications".into();
            }
        }
        "discussion" => {
            subtitle = format!(
                "{subtitle} ? {} comments",
                raw["comments"]["totalCount"].as_u64().unwrap_or_default()
            );
        }
        "project" => {
            state = if raw["closed"] == true {
                "Closed".into()
            } else {
                "Open".into()
            };
            subtitle = format!("Project #{} ? {state}", n);
            body = field(raw, "shortDescription", 4000);
        }
        "package" => {
            subtitle = format!(
                "{} ? {}",
                field(raw, "package_type", 40),
                field(raw, "visibility", 40)
            );
            detail = json!({"type":raw["package_type"],"name":raw["name"],"owner":raw["owner"]["login"],"updated":raw["updated_at"]});
        }
        "version" => {
            if url.is_empty() {
                url = web(text(raw, "package_html_url")).unwrap_or_default();
            }
            let tags = raw["metadata"]["container"]["tags"]
                .as_array()
                .or_else(|| raw["metadata"]["docker"]["tag"].as_array());
            subtitle = tags
                .into_iter()
                .flatten()
                .filter_map(Value::as_str)
                .take(15)
                .collect::<Vec<_>>()
                .join(", ");
            if subtitle.is_empty() {
                subtitle = field(raw, "updated_at", 40);
            }
        }
        "workflow" => {
            subtitle = field(raw, "path", 300);
            detail = json!({"path":raw["path"]});
        }
        "run" => {
            state = raw["conclusion"]
                .as_str()
                .filter(|s| !s.is_empty())
                .unwrap_or_else(|| text(raw, "status"))
                .into();
            subtitle = format!(
                "{} ? {} ? {}",
                state,
                field(raw, "head_branch", 130),
                field(raw, "event", 70)
            );
            detail = json!({"status":raw["status"],"branch":raw["head_branch"],"runNumber":raw["run_number"]});
        }
        "release" => {
            if title.is_empty() {
                title = field(raw, "tag_name", 250);
            }
            state = if raw["draft"] == true {
                "Draft"
            } else if raw["prerelease"] == true {
                "Prerelease"
            } else {
                "Published"
            }
            .into();
            subtitle = format!("{} ? {state}", field(raw, "tag_name", 150));
        }
        _ => {}
    }
    Some(Row {
        id,
        title: if title.trim().is_empty() {
            "Untitled".into()
        } else {
            title.chars().take(300).collect()
        },
        subtitle,
        url,
        repo: repository,
        number: n,
        kind: kind.into(),
        state,
        body,
        detail,
        section: section.into(),
    })
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub enum Purpose {
    Pull,
    Issue,
    Branch,
    Release,
    Runs,
    Dispatch,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub enum Kind {
    Issues { pull: bool, mine: bool },
    Repositories(RepoMode),
    RepositoryPicker(Purpose),
    Notifications { unread: bool },
    Discussions(bool),
    Projects,
    Packages,
    Versions(Row),
    Runs(Repo),
    Workflows(Repo),
    Releases(Repo),
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub enum RepoMode {
    Search,
    Latest,
    Starred,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct List {
    pub kind: Kind,
    pub query: String,
    pub rows: Vec<Row>,
    pub next: Value,
    pub page_count: u16,
    pub notice: String,
}
impl List {
    pub fn new(kind: Kind) -> Self {
        Self {
            kind,
            query: String::new(),
            rows: vec![],
            next: Value::Null,
            page_count: 0,
            notice: String::new(),
        }
    }
    pub fn has_more(&self) -> bool {
        !self.next.is_null() && self.rows.len() < LIMIT && self.page_count < 30
    }
    pub fn uses_search(&self) -> bool {
        matches!(
            self.kind,
            Kind::Issues { .. }
                | Kind::Repositories(RepoMode::Search)
                | Kind::RepositoryPicker(_)
                | Kind::Discussions(_)
        )
    }
    pub fn title(&self) -> &str {
        match self.kind {
            Kind::Issues {
                pull: true,
                mine: true,
            } => "My Pull Requests",
            Kind::Issues {
                pull: true,
                mine: false,
            } => "Search Pull Requests",
            Kind::Issues {
                pull: false,
                mine: true,
            } => "My Issues",
            Kind::Issues {
                pull: false,
                mine: false,
            } => "Search Issues",
            Kind::Repositories(RepoMode::Search) => "Search Repositories",
            Kind::Repositories(RepoMode::Latest) => "My Latest Repositories",
            Kind::Repositories(RepoMode::Starred) => "My Starred Repositories",
            Kind::RepositoryPicker(_) => "Choose Repository",
            Kind::Notifications { unread: true } => "Unread Notifications",
            Kind::Notifications { unread: false } => "Notifications",
            Kind::Discussions(false) => "Search Discussions",
            Kind::Discussions(true) => "My Discussions",
            Kind::Projects => "My Projects",
            Kind::Packages => "My Packages",
            Kind::Versions(_) => "Package Versions",
            Kind::Runs(_) => "Workflow Runs",
            Kind::Workflows(_) => "Choose Workflow",
            Kind::Releases(_) => "Releases",
        }
    }
    pub async fn load(
        &mut self,
        api: &Api,
        prefs: &Value,
        login: &str,
        more: bool,
    ) -> Result<(), String> {
        if more && !self.has_more() {
            return Ok(());
        }
        let old = if more { self.next.clone() } else { Value::Null };
        let (mut rows, next, notice) = load(
            api,
            prefs,
            login,
            &self.kind,
            &self.query,
            old.clone(),
            more,
        )
        .await?;
        if more && next.is_string() && next == old {
            return Err("GitHub repeated a pagination cursor.".into());
        }
        if more {
            self.page_count += 1;
            self.rows.append(&mut rows);
        } else {
            self.page_count = 1;
            self.rows = rows;
        }
        let mut seen = HashSet::new();
        self.rows
            .retain(|r| seen.insert(format!("{}:{}", r.kind, r.id)));
        self.rows.truncate(LIMIT);
        self.next = next;
        self.notice = notice;
        Ok(())
    }
}

fn rest_cursor(value: &Value) -> u16 {
    value
        .as_u64()
        .and_then(|v| u16::try_from(v).ok())
        .unwrap_or(1)
}
fn rest_next(current: u16, next: bool) -> Value {
    if next && current < 30 {
        json!(current + 1)
    } else {
        Value::Null
    }
}
fn size(prefs: &Value) -> u16 {
    prefs["numberOfResults"]
        .as_u64()
        .or_else(|| text(prefs, "numberOfResults").parse().ok())
        .unwrap_or(25)
        .clamp(5, 50) as u16
}
fn preferred(prefs: &Value, key: &str, default: bool) -> bool {
    prefs[key].as_bool().unwrap_or(default)
}
fn options(raw: &Value) -> Vec<&Value> {
    raw.as_array()
        .map(|a| a.iter().collect())
        .unwrap_or_default()
}
fn as_rows(value: &Value, kind: &str, repo: Option<&Repo>, section: &str) -> Vec<Row> {
    options(value)
        .into_iter()
        .filter_map(|v| row(v, kind, repo, section))
        .collect()
}
fn repositories(prefs: &Value) -> Vec<Repo> {
    text(prefs, "repositoryList")
        .split(|c: char| c == ',' || c.is_whitespace())
        .filter_map(|v| Repo::parse(v).ok())
        .take(24)
        .collect()
}
fn visible_repo(row: &Row, prefs: &Value, configured: &[Repo]) -> bool {
    if configured.is_empty() {
        return true;
    }
    let included = row.repo.as_ref().is_some_and(|repo| {
        configured
            .iter()
            .any(|r| r.full().eq_ignore_ascii_case(&repo.full()))
    });
    match text(prefs, "repositoryFilterMode") {
        "include" => included,
        "exclude" => !included,
        _ => true,
    }
}

fn issue_queries(
    prefs: &Value,
    login: &str,
    pull: bool,
    mine: bool,
    query: &str,
) -> Vec<(String, String)> {
    let kind = if pull { "is:pr" } else { "is:issue" };
    if !mine {
        return vec![(String::new(), format!("{kind} {query}"))];
    }
    let mut groups = vec![];
    if pull || preferred(prefs, "showCreated", true) {
        groups.push(("Created", "author"));
    }
    if preferred(
        prefs,
        if pull {
            "includeAssigned"
        } else {
            "showAssigned"
        },
        false,
    ) {
        groups.push(("Assigned", "assignee"));
    }
    if preferred(
        prefs,
        if pull {
            "includeMentioned"
        } else {
            "showMentioned"
        },
        false,
    ) {
        groups.push(("Mentioned", "mentions"));
    }
    if pull && preferred(prefs, "includeReviewRequests", true) {
        groups.push((
            "Review Requests",
            if preferred(prefs, "includeTeamReviewRequests", false) {
                "review-requested"
            } else {
                "user-review-requested"
            },
        ));
    }
    if pull && preferred(prefs, "includeReviewed", false) {
        groups.push(("Reviewed", "reviewed-by"));
    }
    let draft = if pull && !preferred(prefs, "includeDrafts", true) {
        " draft:false"
    } else {
        ""
    };
    let closed = preferred(
        prefs,
        if pull {
            "includeRecentlyClosed"
        } else {
            "showRecentlyClosed"
        },
        false,
    );
    let date = (Utc::now() - Duration::days(if pull { 14 } else { 60 })).format("%Y-%m-%d");
    let repos = repositories(prefs);
    let filter = match text(prefs, "repositoryFilterMode") {
        "include" => repos
            .iter()
            .map(|repo| format!(" repo:{}", repo.full()))
            .collect::<String>(),
        "exclude" => repos
            .iter()
            .map(|repo| format!(" -repo:{}", repo.full()))
            .collect::<String>(),
        _ => String::new(),
    };
    let mut queries = vec![];
    for (section, qualifier) in groups {
        queries.push((
            section.into(),
            format!("{kind} {qualifier}:{login} is:open archived:false{draft}{filter} {query}"),
        ));
        if closed {
            queries.push(("Recently Closed".into(),format!("{kind} {qualifier}:{login} is:closed archived:false updated:>{date}{filter} {query}")));
        }
    }
    queries
}

type Listing = (Vec<Row>, Value, String);

async fn multi_rest(
    api: &Api,
    lanes: Vec<(String, String)>,
    old: Value,
    more: bool,
    page_size: u16,
    kind: &str,
) -> Result<Listing, String> {
    let mut jobs = tokio::task::JoinSet::new();
    let count = lanes.len();
    let mut pages = vec![Value::Null; count];
    for (index, (section, query)) in lanes.into_iter().enumerate() {
        if more && old[index].is_null() {
            continue;
        }
        let page = rest_cursor(&old[index]);
        let api = api.clone();
        let package = kind == "package";
        jobs.spawn(async move {
            let args = if package {
                vec![("package_type", query)]
            } else {
                vec![
                    ("q", query),
                    ("sort", "updated".into()),
                    ("order", "desc".into()),
                ]
            };
            let path = if package {
                ["user", "packages"]
            } else {
                ["search", "issues"]
            };
            (
                index,
                section,
                page,
                api.page(&path, &args, page, page_size).await,
            )
        });
    }
    let mut result = vec![vec![]; count];
    let mut failures = 0;
    let mut first = None;
    let mut passed = 0;
    while let Some(job) = jobs.join_next().await {
        let (index, section, page, response) =
            job.map_err(|_| "GitHub list request was interrupted.")?;
        match response {
            Ok((raw, has_more)) => {
                passed += 1;
                result[index] = as_rows(
                    if kind == "package" {
                        &raw
                    } else {
                        &raw["items"]
                    },
                    kind,
                    None,
                    &section,
                );
                pages[index] = rest_next(page, has_more);
            }
            Err(error) => {
                failures += 1;
                if first.is_none() {
                    first = Some(error);
                }
            }
        }
    }
    if passed == 0 && failures > 0 {
        return Err(first.expect("first error").into());
    }
    let next = if pages.iter().any(|value| !value.is_null()) {
        json!(pages)
    } else {
        Value::Null
    };
    Ok((
        result.into_iter().flatten().collect(),
        next,
        if failures > 0 {
            format!("{failures} GitHub sections could not be loaded. Refresh to retry.")
        } else {
            String::new()
        },
    ))
}

async fn load(
    api: &Api,
    prefs: &Value,
    login: &str,
    kind: &Kind,
    query: &str,
    old: Value,
    more: bool,
) -> Result<Listing, String> {
    let page = rest_cursor(&old);
    let per_page = size(prefs);
    let (mut rows, next) = match kind {
        Kind::Issues { pull, mine } => {
            if !mine && query.trim().is_empty() {
                return Ok((vec![], Value::Null, String::new()));
            }
            let (mut rows, next, notice) = multi_rest(
                api,
                issue_queries(prefs, login, *pull, *mine, query),
                old,
                more,
                per_page,
                if *pull { "pull" } else { "issue" },
            )
            .await?;
            if *mine {
                let configured = repositories(prefs);
                rows.retain(|row| visible_repo(row, prefs, &configured));
            }
            return Ok((rows, next, notice));
        }
        Kind::Packages => {
            let lanes = PACKAGE_TYPES
                .iter()
                .map(|kind| (kind.to_string(), kind.to_string()))
                .collect();
            return multi_rest(api, lanes, old, more, per_page, "package").await;
        }
        Kind::Discussions(mine) => {
            if !mine && query.trim().is_empty() {
                return Ok((vec![], Value::Null, String::new()));
            }
            let query = if *mine {
                format!("author:{login} {query}")
            } else {
                query.to_owned()
            };
            let reply=api.graph("query SearchDiscussions($q:String!,$count:Int!,$after:String){search(query:$q,type:DISCUSSION,first:$count,after:$after){pageInfo{hasNextPage endCursor} nodes{... on Discussion{id title url number body comments{totalCount} repository{nameWithOwner}}}}}",json!({"q":query,"count":per_page,"after":old})).await?;
            graph_rows(&reply["search"], "discussion")?
        }
        Kind::Projects => {
            let reply=api.graph("query MyProjects($count:Int!,$after:String){viewer{projectsV2(first:$count,after:$after,orderBy:{field:UPDATED_AT,direction:DESC}){pageInfo{hasNextPage endCursor} nodes{id number title url closed shortDescription}}}}",json!({"count":per_page,"after":old})).await?;
            graph_rows(&reply["viewer"]["projectsV2"], "project")?
        }
        Kind::Repositories(RepoMode::Search) | Kind::RepositoryPicker(_)
            if !query.trim().is_empty() =>
        {
            let mut search = query.to_owned();
            if !preferred(prefs, "includeForks", false) {
                search.push_str(" fork:false");
            } else {
                search.push_str(" fork:true");
            }
            if !preferred(prefs, "includeArchived", false) {
                search.push_str(" archived:false");
            }
            let (data, more) = api
                .page(
                    &["search", "repositories"],
                    &[("q", search), ("sort", "updated".into())],
                    page,
                    per_page,
                )
                .await?;
            (
                as_rows(&data["items"], "repository", None, ""),
                rest_next(page, more),
            )
        }
        Kind::Repositories(RepoMode::Starred) => {
            let (data, more) = api
                .page(
                    &["user", "starred"],
                    &[("sort", "updated".into()), ("direction", "desc".into())],
                    page,
                    per_page,
                )
                .await?;
            (
                as_rows(&data, "repository", None, ""),
                rest_next(page, more),
            )
        }
        Kind::Repositories(_) | Kind::RepositoryPicker(_) => {
            let (data, more) = api
                .page(
                    &["user", "repos"],
                    &[("sort", "pushed".into())],
                    page,
                    per_page,
                )
                .await?;
            (
                as_rows(&data, "repository", None, ""),
                rest_next(page, more),
            )
        }
        Kind::Notifications { unread } => {
            let (data, more) = api
                .page(
                    &["notifications"],
                    &[("all", (!unread).to_string())],
                    page,
                    per_page,
                )
                .await?;
            let mut rows = as_rows(&data, "notification", None, "");
            let configured = repositories(prefs);
            rows.retain(|row| visible_repo(row, prefs, &configured));
            (rows, rest_next(page, more))
        }
        Kind::Versions(package) => {
            let package_type = text(&package.detail, "type");
            let name = text(&package.detail, "name");
            if !PACKAGE_TYPES.contains(&package_type) || name.is_empty() {
                return Err("GitHub did not supply a valid package identity.".into());
            }
            let (data, more) = api
                .page(
                    &["user", "packages", package_type, name, "versions"],
                    &[],
                    page,
                    per_page,
                )
                .await?;
            (as_rows(&data, "version", None, ""), rest_next(page, more))
        }
        Kind::Runs(repo) | Kind::Workflows(repo) | Kind::Releases(repo) => {
            let (suffix, kind, key): (&[&str], _, _) = match kind {
                Kind::Runs(_) => (&["actions", "runs"], "run", "workflow_runs"),
                Kind::Workflows(_) => (&["actions", "workflows"], "workflow", "workflows"),
                _ => (&["releases"], "release", ""),
            };
            let (data, more) = api
                .page(&repo.for_path(suffix), &[], page, per_page)
                .await?;
            let rows = as_rows(
                if key.is_empty() { &data } else { &data[key] },
                kind,
                Some(repo),
                "",
            );
            (rows, rest_next(page, more))
        }
    };
    if preferred(prefs, "displayOwnerName", true) == false {
        for row in &mut rows {
            if row.kind == "repository" {
                if let Some(repo) = &row.repo {
                    row.title = repo.name.clone();
                }
            }
        }
    }
    Ok((rows, next, String::new()))
}
fn graph_rows(connection: &Value, kind: &str) -> Result<(Vec<Row>, Value), String> {
    let info = &connection["pageInfo"];
    let next = match info["hasNextPage"].as_bool() {
        Some(true) => info["endCursor"]
            .as_str()
            .filter(|s| !s.is_empty() && s.len() <= 4096)
            .map(|s| json!(s))
            .ok_or("GitHub returned an invalid cursor.")?,
        Some(false) => Value::Null,
        _ => return Err("GitHub returned an invalid connection.".into()),
    };
    Ok((as_rows(&connection["nodes"], kind, None, ""), next))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn filters_identifiers_and_translates_notification_links() {
        assert!(Repo::parse("owner/../escape").is_err());
        assert!(web("https://github.com@evil.invalid/path").is_none());
        let n=row(&json!({"id":"42","reason":"mention","unread":true,"subject":{"title":"Talk","type":"Issue","url":"https://api.github.com/repos/o/r/issues/9"},"repository":{"full_name":"o/r"}}),"notification",None,"").unwrap();
        assert_eq!(n.url, "https://github.com/o/r/issues/9");
    }
    #[test]
    fn selected_mine_lanes_honor_user_preferences() {
        let queries = issue_queries(
            &json!({"includeAssigned":true,"includeRecentlyClosed":true,"includeTeamReviewRequests":false,"includeDrafts":false,"repositoryFilterMode":"include","repositoryList":"o/r garbage"}),
            "fixture",
            true,
            true,
            "keyword",
        );
        assert!(queries
            .iter()
            .any(|(section, query)| section == "Review Requests"
                && query.contains("user-review-requested:fixture")));
        assert!(queries.iter().all(|(_, query)| query.contains("repo:o/r")));
        assert!(queries
            .iter()
            .any(|(_, query)| query.contains("draft:false")));
    }
}
