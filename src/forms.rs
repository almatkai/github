use crate::{
    api::{self, text, Api},
    model::{account, repo_name, Repo, Row},
};
use reqwest::Method;
use serde::{Deserialize, Serialize};
use serde_json::{json, Map, Value};
use std::path::Path;
use tezbar_extension_sdk::ExtensionContext;

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Input {
    pub name: String,
    pub description: String,
    pub kind: String,
    pub required: bool,
    pub choices: Vec<String>,
    pub default: String,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub enum Kind {
    Pull(Repo),
    Issue(Repo),
    Branch(Repo),
    Release(Repo),
    Repository(Vec<String>),
    Dispatch(Repo, Row, Vec<Input>, String),
    Download,
    Comment(Row),
    EditTitle(Row),
    LookupRepo(crate::model::Purpose),
}
impl Kind {
    pub fn title(&self) -> &str {
        match self {
            Self::Pull(_) => "Create Pull Request",
            Self::Issue(_) => "Create Issue",
            Self::Branch(_) => "Create Branch",
            Self::Release(_) => "Create Release",
            Self::Repository(_) => "Create Repository",
            Self::Dispatch(..) => "Dispatch Workflow",
            Self::Download => "Download ZIP",
            Self::Comment(_) => "Add Comment",
            Self::EditTitle(_) => "Save Title",
            Self::LookupRepo(_) => "Find Repository",
        }
    }
}

pub fn reference(raw: &str) -> Result<&str, String> {
    let raw = raw.trim();
    if raw.is_empty()
        || raw.len() > 240
        || raw.contains("..")
        || raw.contains("@{")
        || raw == "@"
        || raw.starts_with('-')
        || raw.ends_with('.')
        || raw
            .split('/')
            .any(|p| p.is_empty() || p == "." || p.starts_with('.') || p.ends_with(".lock"))
        || raw
            .chars()
            .any(|c| c.is_control() || c.is_whitespace() || "~^:?*[\\".contains(c))
    {
        return Err("Enter a valid Git ref or branch name.".into());
    }
    Ok(raw)
}
pub fn sha(raw: &str) -> Result<&str, String> {
    if raw.len() == 40 && raw.bytes().all(|c| c.is_ascii_hexdigit()) {
        Ok(raw)
    } else {
        Err("GitHub did not return a valid commit SHA.".into())
    }
}
fn list(raw: &str, field: &str) -> Result<Vec<String>, String> {
    let items: Vec<_> = raw
        .split(',')
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .take(26)
        .map(str::to_owned)
        .collect();
    if items.len() > 25 {
        return Err(format!("Enter at most 25 {field}."));
    }
    Ok(items)
}
fn validate_people(raw: &str, field: &str) -> Result<Vec<String>, String> {
    let people = list(raw, field)?;
    if people.iter().all(|user| account(user)) {
        Ok(people)
    } else {
        Err("Enter valid comma-separated GitHub user names.".into())
    }
}
pub fn required<'a>(value: &'a Value, key: &str, max: usize) -> Result<&'a str, String> {
    let value = text(value, key).trim();
    if value.is_empty() || value.len() > max {
        return Err(format!("Enter a {key} of up to {max} UTF-8 bytes."));
    }
    Ok(value)
}

pub async fn confirm(ctx: &ExtensionContext, title: &str, message: &str) -> Result<bool, String> {
    let reply = ctx
        .host(
            "host.dialog.confirm",
            json!({"title":title,"message":message,"buttons":["Cancel",title],"type":"warning"}),
        )
        .await?;
    Ok(reply["response"] == 1)
}
fn repo_metadata(values: &Value) -> Result<Map<String, Value>, String> {
    let mut out = Map::new();
    let people = validate_people(text(values, "assignees"), "assignees")?;
    if !people.is_empty() {
        out.insert("assignees".into(), json!(people));
    }
    let labels = list(text(values, "labels"), "labels")?;
    if !labels.is_empty() {
        out.insert("labels".into(), json!(labels));
    }
    let milestone = text(values, "milestone").trim();
    if !milestone.is_empty() {
        let milestone = milestone
            .parse::<u64>()
            .ok()
            .filter(|n| *n > 0)
            .ok_or("Enter the GitHub milestone number.")?;
        out.insert("milestone".into(), json!(milestone));
    }
    Ok(out)
}
fn journal(raw: &Value, title: &str, url: String) -> Outcome {
    Outcome {
        title: title.into(),
        url: crate::model::web(text(raw, "html_url")).unwrap_or(url),
        note: String::new(),
        path: String::new(),
    }
}
pub struct Outcome {
    pub title: String,
    pub url: String,
    pub note: String,
    pub path: String,
}
async fn repo_default(api: &Api, repo: &Repo) -> Result<(String, Value), String> {
    let info = api.get(&repo.path(), &[]).await?;
    let base = reference(text(&info, "default_branch"))?.to_owned();
    Ok((base, info))
}
pub async fn defaults(
    api: &Api,
    repo: &Repo,
    purpose: &crate::model::Purpose,
) -> Result<Value, String> {
    use crate::model::Purpose::*;
    let (base, _) = repo_default(api, repo).await?;
    Ok(match purpose {
        Pull => json!({"base":base,"draft":false,"maintainerCanModify":true}),
        Branch => json!({"base":base}),
        Release => json!({"target":base,"draft":false,"prerelease":false}),
        _ => json!({}),
    })
}

fn scalar(raw: &serde_yaml::Value) -> String {
    raw.as_str()
        .map(str::to_owned)
        .or_else(|| raw.as_bool().map(|v| v.to_string()))
        .or_else(|| raw.as_f64().map(|v| v.to_string()))
        .unwrap_or_default()
}
pub fn parse_workflow(raw: &str) -> Result<Vec<Input>, String> {
    let yaml: serde_yaml::Value =
        serde_yaml::from_str(raw).map_err(|_| "GitHub workflow YAML could not be read.")?;
    let triggers = &yaml["on"];
    let dispatch = triggers.as_str() == Some("workflow_dispatch")
        || triggers.as_sequence().is_some_and(|rows| {
            rows.iter()
                .any(|row| row.as_str() == Some("workflow_dispatch"))
        })
        || triggers
            .as_mapping()
            .is_some_and(|map| map.contains_key("workflow_dispatch"));
    if !dispatch {
        return Err("This workflow does not define workflow_dispatch on the chosen ref.".into());
    }
    let Some(inputs) = triggers["workflow_dispatch"]["inputs"].as_mapping() else {
        return Ok(vec![]);
    };
    if inputs.len() > 25 {
        return Err("This workflow defines more than 25 inputs.".into());
    }
    let mut fields = Vec::new();
    for (key, spec) in inputs {
        let name = key
            .as_str()
            .filter(|name| {
                !name.is_empty()
                    && name.len() <= 100
                    && name
                        .bytes()
                        .all(|c| c.is_ascii_alphanumeric() || c == b'_' || c == b'-')
            })
            .ok_or("Unsupported workflow input name.")?;
        let kind = spec["type"].as_str().unwrap_or("string");
        if !matches!(
            kind,
            "string" | "choice" | "boolean" | "number" | "environment"
        ) {
            return Err("Unsupported workflow input type.".into());
        }
        let choices = spec["options"]
            .as_sequence()
            .into_iter()
            .flatten()
            .map(scalar)
            .filter(|v| !v.is_empty())
            .take(100)
            .collect();
        fields.push(Input {
            name: name.into(),
            description: spec["description"]
                .as_str()
                .unwrap_or_default()
                .chars()
                .take(240)
                .collect(),
            kind: kind.into(),
            required: spec["required"].as_bool().unwrap_or(false),
            choices,
            default: scalar(&spec["default"]),
        });
    }
    Ok(fields)
}
pub async fn load_inputs(
    api: &Api,
    repo: &Repo,
    workflow: &Row,
    raw_ref: &str,
) -> Result<Vec<Input>, String> {
    let reference = reference(raw_ref)?;
    let file = text(&workflow.detail, "path");
    let leaf = file
        .strip_prefix(".github/workflows/")
        .filter(|p| repo_name(p) && (p.ends_with(".yml") || p.ends_with(".yaml")))
        .ok_or("GitHub did not return a valid workflow YAML path.")?;
    let body = api
        .raw(
            &repo.for_path(&["contents", ".github", "workflows", leaf]),
            &[("ref", reference.into())],
        )
        .await?;
    parse_workflow(&body)
}
pub fn inputs(fields: &[Input], values: &Value) -> Result<Value, String> {
    let mut result = Map::new();
    for field in fields {
        let key = format!("workflow:{}", field.name);
        let value = values[&key].as_str().unwrap_or_default();
        let value = if value.is_empty() {
            field.default.as_str()
        } else {
            value
        };
        if value.is_empty() {
            if field.required {
                return Err(format!("Enter a value for {}.", field.name));
            }
            continue;
        }
        let valid = match field.kind.as_str() {
            "boolean" => matches!(value, "true" | "false"),
            "choice" => field.choices.iter().any(|v| v == value),
            "number" => value.parse::<f64>().is_ok_and(f64::is_finite),
            _ => value.len() <= 32_768,
        };
        if !valid {
            return Err(format!("Enter a valid value for {}.", field.name));
        }
        result.insert(field.name.clone(), json!(value));
    }
    let result = Value::Object(result);
    if serde_json::to_vec(&result).map_or(true, |v| v.len() > 65_535) {
        return Err("Workflow inputs exceed 65,535 bytes.".into());
    }
    Ok(result)
}

fn download_spec(raw: &str) -> Result<(Repo, Option<String>), String> {
    if let Ok(repo) = Repo::parse(raw) {
        return Ok((repo, None));
    }
    let parsed =
        reqwest::Url::parse(raw).map_err(|_| "Enter a GitHub repository URL or owner/name.")?;
    if crate::model::web(raw).is_none()
        || !matches!(parsed.host_str(), Some("github.com" | "www.github.com"))
    {
        return Err("Enter a github.com repository URL or owner/name.".into());
    }
    let parts = parsed
        .path_segments()
        .ok_or("Enter a GitHub repository URL.")?
        .filter(|part| !part.is_empty())
        .collect::<Vec<_>>();
    if parts.len() < 2 {
        return Err("Enter a GitHub repository URL.".into());
    }
    let repo = Repo::parse(&format!(
        "{}/{}",
        parts[0],
        parts[1].trim_end_matches(".git")
    ))?;
    let reference = match parts.as_slice() {
        [_, _] => None,
        [_, _, "tree", rest @ ..] if !rest.is_empty() => {
            Some(reference(&rest.join("/"))?.to_owned())
        }
        _ => return Err("Enter a repository root or branch URL.".into()),
    };
    Ok((repo, reference))
}
fn checked_number(row: &Row) -> Result<String, String> {
    if row.number == 0 {
        Err("GitHub did not return an item number.".into())
    } else {
        Ok(row.number.to_string())
    }
}

pub async fn write(
    kind: &Kind,
    api: &Api,
    ctx: &ExtensionContext,
    values: &Value,
    login: &str,
) -> Result<Option<Outcome>, String> {
    let result = match kind {
        Kind::Issue(repo) | Kind::Pull(repo) => {
            let title = required(values, "title", 256)?;
            let body = text(values, "body");
            if body.len() > 65_536 {
                return Err("Shorten the description to 65,536 UTF-8 bytes.".into());
            }
            let mut properties = repo_metadata(values)?;
            let headers = json!({"title":title,"body":body});
            let pull = matches!(kind, Kind::Pull(_));
            let endpoint = if pull { "pulls" } else { "issues" };
            let (payload, reviewers) = if pull {
                let base = reference(required(values, "base", 240)?)?;
                let head = required(values, "head", 240)?;
                if let Some((owner, part)) = head.split_once(':') {
                    if !account(owner) {
                        return Err("Invalid fork owner.".into());
                    }
                    reference(part)?;
                } else {
                    reference(head)?;
                }
                let reviewers = validate_people(text(values, "reviewers"), "reviewers")?;
                (
                    json!({"title":title,"body":body,"base":base,"head":head,"draft":values["draft"]==true,"maintainer_can_modify":values["maintainerCanModify"]!=false}),
                    reviewers,
                )
            } else {
                properties.extend(headers.as_object().expect("form payload").clone());
                (Value::Object(properties.clone()), vec![])
            };
            if !confirm(
                ctx,
                kind.title(),
                &format!("Create {} in {}?", title, repo.full()),
            )
            .await?
            {
                return Ok(None);
            }
            let created = api
                .mutate(Method::POST, &repo.for_path(&[endpoint]), Some(payload))
                .await?;
            let number=api::number(&created,"number").ok_or("GitHub did not return the created item. Check the repository before resubmitting.")?.to_string();
            let target = format!(
                "{}/{}/{}",
                repo.web(),
                if pull { "pull" } else { "issues" },
                number
            );
            let mut result = journal(
                &created,
                if pull {
                    "Pull request created"
                } else {
                    "Issue created"
                },
                target,
            );
            if pull {
                let mut failed = vec![];
                if !properties.is_empty()
                    && api
                        .mutate(
                            Method::PATCH,
                            &repo.for_path(&["issues", &number]),
                            Some(Value::Object(properties)),
                        )
                        .await
                        .is_err()
                {
                    failed.push("assignees, labels or milestone");
                }
                if !reviewers.is_empty()
                    && api
                        .mutate(
                            Method::POST,
                            &repo.for_path(&["pulls", &number, "requested_reviewers"]),
                            Some(json!({"reviewers":reviewers})),
                        )
                        .await
                        .is_err()
                {
                    failed.push("review requests");
                }
                let merge = text(values, "autoMerge");
                if matches!(merge, "MERGE" | "SQUASH" | "REBASE") {
                    let node = text(&created, "node_id");
                    if node.is_empty() || api.graph("mutation EnableAutoMerge($id:ID!,$method:PullRequestMergeMethod!){enablePullRequestAutoMerge(input:{pullRequestId:$id,mergeMethod:$method}){pullRequest{id}}}",json!({"id":node,"method":merge})).await.is_err(){failed.push("auto-merge");}
                }
                if !failed.is_empty() {
                    result.note = format!(
                        "The pull request was created. Verify these updates on GitHub: {}.",
                        failed.join(", ")
                    );
                }
            }
            result
        }
        Kind::Repository(owners) => {
            let name = required(values, "name", 100)?;
            if !repo_name(name) {
                return Err("GitHub repository names use letters, numbers, periods, underscores and hyphens.".into());
            }
            let owner = required(values, "owner", 100)?;
            if !owners
                .iter()
                .any(|approved| approved.eq_ignore_ascii_case(owner))
            {
                return Err("Choose an accessible owner.".into());
            }
            let description = text(values, "description").trim();
            if description.len() > 350 {
                return Err("Shorten the description to 350 UTF-8 bytes.".into());
            }
            if !confirm(
                ctx,
                "Create Repository",
                &format!("Create {owner}/{name} on GitHub?"),
            )
            .await?
            {
                return Ok(None);
            }
            let route = if owner.eq_ignore_ascii_case(login) {
                vec!["user", "repos"]
            } else {
                vec!["orgs", owner, "repos"]
            };
            let raw=api.mutate(Method::POST,&route,Some(json!({"name":name,"description":description,"private":values["private"]!=false,"auto_init":values["autoInit"]==true}))).await?;
            if raw.get("id").is_none() {
                return Err("GitHub did not return the created repository. Review your repositories before resubmitting.".into());
            }
            journal(
                &raw,
                "Repository created",
                format!("https://github.com/{owner}/{name}"),
            )
        }
        Kind::Branch(repo) => {
            let base = reference(required(values, "base", 240)?)?;
            let mut path = repo.for_path(&["git", "ref", "heads"]);
            path.extend(base.split('/'));
            let info = api.get(&path, &[]).await?;
            let commit = sha(text(&info["object"], "sha"))?;
            let issue = text(values, "issue").trim();
            let name = text(values, "name").trim();
            let name = if name.is_empty() && !issue.is_empty() {
                ""
            } else {
                reference(name.trim_start_matches("refs/heads/"))?
            };
            if !confirm(
                ctx,
                "Create Branch",
                &format!("Create a branch in {} from {base}?", repo.full()),
            )
            .await?
            {
                return Ok(None);
            }
            let actual = if issue.is_empty() {
                let raw = api
                    .mutate(
                        Method::POST,
                        &repo.for_path(&["git", "refs"]),
                        Some(json!({"ref":format!("refs/heads/{name}"),"sha":commit})),
                    )
                    .await?;
                text(&raw,"ref").strip_prefix("refs/heads/").and_then(|value|reference(value).ok()).ok_or("GitHub did not return the branch. Check the repository before resubmitting.")?.to_owned()
            } else {
                let issue = issue
                    .parse::<u64>()
                    .ok()
                    .filter(|v| *v > 0)
                    .ok_or("Enter an issue number.")?
                    .to_string();
                let item = api.get(&repo.for_path(&["issues", &issue]), &[]).await?;
                if item.get("pull_request").is_some() {
                    return Err("Choose an issue to link to the branch.".into());
                }
                let repo_info = api.get(&repo.path(), &[]).await?;
                let repo_id = repo_info["node_id"]
                    .as_str()
                    .filter(|v| !v.is_empty())
                    .ok_or("GitHub did not return a repository node ID.")?;
                let issue_id = item["node_id"]
                    .as_str()
                    .filter(|v| !v.is_empty())
                    .ok_or("GitHub did not return an issue node ID.")?;
                let mut input = json!({"repositoryId":repo_id,"issueId":issue_id,"oid":commit});
                if !name.is_empty() {
                    input["name"] = json!(name);
                }
                let data=api.graph("mutation CreateLinkedBranch($input:CreateLinkedBranchInput!){createLinkedBranch(input:$input){linkedBranch{ref{name}}}}",json!({"input":input})).await?;
                reference(text(
                    &data["createLinkedBranch"]["linkedBranch"]["ref"],
                    "name",
                ))?
                .to_owned()
            };
            journal(
                &Value::Null,
                "Branch created",
                format!(
                    "{}/tree/{}",
                    repo.web(),
                    tezbar_extension_sdk::encode_component(&actual)
                ),
            )
        }
        Kind::Release(repo) => {
            let tag = reference(required(values, "tag", 240)?)?;
            let target = text(values, "target").trim();
            if !target.is_empty() {
                reference(target)?;
            }
            let body = text(values, "body");
            if body.len() > 125_000 {
                return Err("Shorten the release notes to 125,000 UTF-8 bytes.".into());
            }
            let draft = values["draft"] == true;
            if !confirm(
                ctx,
                if draft {
                    "Create Draft Release"
                } else {
                    "Publish Release"
                },
                &format!("Create release {tag} in {}?", repo.full()),
            )
            .await?
            {
                return Ok(None);
            }
            let mut data = json!({"tag_name":tag,"name":text(values,"title"),"body":body,"draft":draft,"prerelease":values["prerelease"]==true});
            if !target.is_empty() {
                data["target_commitish"] = json!(target);
            }
            let raw = api
                .mutate(Method::POST, &repo.for_path(&["releases"]), Some(data))
                .await?;
            if raw.get("id").is_none() {
                return Err("GitHub did not return the release. Check the releases page before resubmitting.".into());
            }
            journal(
                &raw,
                if draft {
                    "Draft release created"
                } else {
                    "Release published"
                },
                format!("{}/releases", repo.web()),
            )
        }
        Kind::Dispatch(repo, workflow, fields, loaded_ref) => {
            let selected = reference(required(values, "ref", 240)?)?;
            if selected != loaded_ref {
                return Err("Load the workflow inputs for the selected ref first.".into());
            }
            let values = inputs(fields, values)?;
            let workflow_id = workflow
                .id
                .parse::<u64>()
                .ok()
                .filter(|v| *v > 0)
                .ok_or("Unknown GitHub workflow ID.")?
                .to_string();
            if !confirm(
                ctx,
                "Dispatch Workflow",
                &format!("Run {} in {} on {selected}?", workflow.title, repo.full()),
            )
            .await?
            {
                return Ok(None);
            }
            api.mutate(
                Method::POST,
                &repo.for_path(&["actions", "workflows", &workflow_id, "dispatches"]),
                Some(json!({"ref":selected,"inputs":values})),
            )
            .await?;
            journal(&Value::Null, "Workflow dispatched", workflow.url.clone())
        }
        Kind::Download => {
            let (repo, detected) = download_spec(required(values, "repository", 2048)?)?;
            let supplied = text(values, "ref").trim();
            let selected = if supplied.is_empty() {
                detected.as_deref()
            } else {
                Some(reference(supplied)?)
            };
            let directory = required(values, "directory", 3000)?;
            let path = api
                .download(&repo.owner, &repo.name, selected, Path::new(directory))
                .await?;
            Outcome {
                title: "Repository ZIP downloaded".into(),
                url: repo.web(),
                note: String::new(),
                path: path.to_string_lossy().into(),
            }
        }
        Kind::Comment(row) | Kind::EditTitle(row) => {
            let repo = row.repo.as_ref().ok_or("Unknown GitHub repository.")?;
            let number = checked_number(row)?;
            let comment = matches!(kind, Kind::Comment(_));
            let (route, method, payload) = if comment {
                (
                    repo.for_path(&["issues", &number, "comments"]),
                    Method::POST,
                    json!({"body":required(values,"body",65_536)?}),
                )
            } else {
                (
                    repo.for_path(&[
                        if row.kind == "pull" {
                            "pulls"
                        } else {
                            "issues"
                        },
                        &number,
                    ]),
                    Method::PATCH,
                    json!({"title":required(values,"title",256)?}),
                )
            };
            if !confirm(
                ctx,
                kind.title(),
                &format!("Update #{} in {}?", number, repo.full()),
            )
            .await?
            {
                return Ok(None);
            }
            let raw = api.mutate(method, &route, Some(payload)).await?;
            journal(
                &raw,
                if comment {
                    "Comment added"
                } else {
                    "Title updated"
                },
                row.url.clone(),
            )
        }
        Kind::LookupRepo(_) => {
            return Err("Repository selection is unavailable in this form.".into())
        }
    };
    Ok(Some(result))
}

pub async fn generate_notes(api: &Api, repo: &Repo, values: &Value) -> Result<String, String> {
    let tag = reference(required(values, "tag", 240)?)?;
    let mut data = json!({"tag_name":tag});
    let base = text(values, "target").trim();
    if !base.is_empty() {
        data["target_commitish"] = json!(reference(base)?);
    }
    let raw = api
        .mutate(
            Method::POST,
            &repo.for_path(&["releases", "generate-notes"]),
            Some(data),
        )
        .await?;
    raw["body"]
        .as_str()
        .map(str::to_owned)
        .ok_or_else(|| "GitHub did not return release notes.".into())
}

pub async fn owners(api: &Api, login: &str) -> (Vec<String>, String) {
    let mut owners = vec![login.into()];
    let mut page = 1;
    while page <= 3 {
        match api.page(&["user", "orgs"], &[], page, 50).await {
            Ok((rows, more)) => {
                owners.extend(
                    rows.as_array()
                        .into_iter()
                        .flatten()
                        .map(|r| text(r, "login"))
                        .filter(|login| account(login))
                        .map(str::to_owned),
                );
                if !more {
                    break;
                }
            }
            Err(_) => {
                return (
                    owners,
                    "Organization owners could not be loaded. Personal repositories are available."
                        .into(),
                )
            }
        }
        page += 1;
    }
    owners.dedup();
    (owners, String::new())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn validated_workflow_inputs_and_reference() {
        let schema=parse_workflow("on:\n  workflow_dispatch:\n    inputs:\n      mode:\n        type: choice\n        required: true\n        options: [test, preview]\n      dry:\n        type: boolean\n        default: true\n").unwrap();
        let result = inputs(&schema, &json!({"workflow:mode":"test"})).unwrap();
        assert_eq!(result, json!({"mode":"test","dry":"true"}));
        assert!(inputs(&schema, &json!({"workflow:mode":"no"})).is_err());
        assert!(parse_workflow("on: [push, pull_request]").is_err());
        assert!(reference("branch/feature").is_ok());
        assert!(reference("refs/../escape").is_err());
    }
    #[test]
    fn parses_download_without_allowing_foreign_origin() {
        let (repo, branch) = download_spec("https://github.com/owner/repo/tree/feat/doc").unwrap();
        assert_eq!(repo.full(), "owner/repo");
        assert_eq!(branch.as_deref(), Some("feat/doc"));
        assert!(download_spec("https://evil.invalid/owner/repo").is_err());
    }
}
