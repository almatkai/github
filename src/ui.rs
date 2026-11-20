use crate::{
    forms::{self, Outcome},
    model::{self, List, Purpose, Repo, Row},
};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::collections::BTreeMap;

#[derive(Clone, Debug, Serialize, Deserialize)]
pub enum Screen {
    Setup,
    List(List),
    Form(forms::Kind, Value),
    Detail(Row),
    Done {
        title: String,
        url: String,
        path: String,
    },
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub enum Change {
    Open,
    Close,
    Merge,
    Star,
    Unstar,
    MarkRead,
    Done,
    Mute,
    Rerun,
    Cancel,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub enum Action {
    Settings,
    Back,
    Refresh,
    LoadMore,
    FieldChanged(String),
    Open(String),
    Copy(String),
    Reveal(String),
    Choose(Purpose, Repo),
    DirectRepo(Purpose),
    Workflow(Repo, Row),
    Details(Row),
    Versions(Row),
    CreateRelease(Repo),
    Comment(Row),
    EditTitle(Row),
    GenerateNotes,
    ReloadWorkflow,
    Submit,
    Unread(bool),
    Change(Row, Change),
    ReadAll,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Session {
    pub screen: Screen,
    pub history: Vec<Screen>,
    pub actions: BTreeMap<String, Action>,
    pub revision: u64,
    pub notice: String,
    pub login: String,
}
impl Session {
    pub fn new(screen: Screen, login: String) -> Self {
        Self {
            screen,
            history: vec![],
            actions: BTreeMap::new(),
            revision: 0,
            notice: String::new(),
            login,
        }
    }
    pub fn push(&mut self, screen: Screen) -> Result<(), String> {
        if self.history.len() >= 10 {
            return Err("Back out of a GitHub view before navigating deeper.".into());
        }
        self.history
            .push(std::mem::replace(&mut self.screen, screen));
        Ok(())
    }
    pub fn back(&mut self) {
        if let Some(screen) = self.history.pop() {
            self.screen = screen;
        }
    }
    pub fn finish(&mut self, outcome: Outcome) {
        self.notice = outcome.note;
        let next = Screen::Done {
            title: outcome.title,
            url: outcome.url,
            path: outcome.path,
        };
        if matches!(self.screen, Screen::Form(..)) {
            // A completed external write abandons the submitted form, so Back
            // cannot resurrect a successful request and send it again.
            self.screen = next;
        } else if self.history.len() < 10 {
            self.history.push(std::mem::replace(&mut self.screen, next));
        } else {
            self.screen = next;
        }
    }
}

struct Actions {
    revision: u64,
    wire: Vec<Value>,
    map: BTreeMap<String, Action>,
}
impl Actions {
    fn add(&mut self, title: &str, action: Action) -> String {
        let id = format!("github-{}-{}", self.revision, self.wire.len());
        let kind = if matches!(action, Action::Submit) {
            "submit-form"
        } else {
            "callback"
        };
        self.wire.push(json!({"id":id,"title":title,"kind":kind}));
        self.map.insert(id.clone(), action);
        id
    }
}

fn description(title: &str, value: &str) -> Value {
    json!({"type":"Form.Description","props":{"title":title,"text":value}})
}
fn field(kind: &str, id: &str, label: &str, values: &Value) -> Value {
    json!({"type":format!("Form.{kind}"),"props":{"id":id,"title":label,"value":values[id]}})
}
fn checkbox(id: &str, label: &str, values: &Value) -> Value {
    json!({"type":"Form.Checkbox","props":{"id":id,"label":label,"value":values[id].as_bool().unwrap_or(false)}})
}
fn dropdown(
    id: &str,
    label: &str,
    values: &Value,
    options: impl IntoIterator<Item = (String, String)>,
) -> Value {
    let choices=options.into_iter().map(|(value,name)|json!({"type":"Form.Dropdown.Item","props":{"title":name,"value":value}})).collect::<Vec<_>>();
    let mut node = field("Dropdown", id, label, values);
    node["children"] = json!(choices);
    node
}
fn opts(options: &[(&str, &str)]) -> Vec<(String, String)> {
    options
        .iter()
        .map(|(value, label)| (value.to_string(), label.to_string()))
        .collect()
}
fn form(kind: &forms::Kind, values: &Value, notice: &str, actions: &mut Actions) -> Value {
    use forms::Kind::*;
    let mut children = vec![];
    if !notice.is_empty() {
        children.push(description("GitHub", notice));
    }
    let mut ids = vec![actions.add(kind.title(), Action::Submit)];
    match kind {
        Issue(repo) | Pull(repo) => {
            children.push(description("Repository", &repo.full()));
            if matches!(kind, Pull(_)) {
                children.push(field(
                    "TextField",
                    "head",
                    "Head branch (or owner:branch)",
                    values,
                ));
                children.push(field("TextField", "base", "Base branch", values));
                children.push(checkbox("draft", "Create as draft", values));
                children.push(checkbox(
                    "maintainerCanModify",
                    "Allow maintainer edits",
                    values,
                ));
                children.push(dropdown(
                    "autoMerge",
                    "Auto-merge",
                    values,
                    opts(&[
                        ("", "Off"),
                        ("MERGE", "Merge"),
                        ("SQUASH", "Squash"),
                        ("REBASE", "Rebase"),
                    ]),
                ));
                children.push(field(
                    "TextField",
                    "reviewers",
                    "Reviewers (comma-separated usernames)",
                    values,
                ));
            }
            children.push(field("TextField", "title", "Title", values));
            children.push(field("TextArea", "body", "Description (Markdown)", values));
            children.push(field(
                "TextField",
                "assignees",
                "Assignees (comma-separated usernames)",
                values,
            ));
            children.push(field(
                "TextField",
                "labels",
                "Labels (comma-separated names)",
                values,
            ));
            children.push(field("TextField", "milestone", "Milestone number", values));
        }
        Branch(repo) => {
            children.push(description("Repository", &repo.full()));
            children.push(field("TextField", "name", "Branch name", values));
            children.push(field("TextField", "base", "Source branch", values));
            children.push(field(
                "TextField",
                "issue",
                "Linked issue number (optional)",
                values,
            ));
        }
        Release(repo) => {
            children.push(description("Repository", &repo.full()));
            children.push(field("TextField", "tag", "Tag", values));
            children.push(field("TextField", "target", "Target ref", values));
            children.push(field("TextField", "title", "Title", values));
            children.push(field(
                "TextArea",
                "body",
                "Release notes (Markdown)",
                values,
            ));
            children.push(checkbox("draft", "Save as draft", values));
            children.push(checkbox("prerelease", "Mark as prerelease", values));
            ids.push(actions.add("Generate Release Notes", Action::GenerateNotes));
            ids.push(actions.add(
                "Open Releases in Browser",
                Action::Open(format!("{}/releases", repo.web())),
            ));
        }
        Repository(owners) => {
            children.push(dropdown(
                "owner",
                "Owner",
                values,
                owners.iter().map(|owner| (owner.clone(), owner.clone())),
            ));
            children.push(field("TextField", "name", "Repository name", values));
            children.push(field("TextArea", "description", "Description", values));
            children.push(checkbox("private", "Private repository", values));
            children.push(checkbox("autoInit", "Initialize with a README", values));
        }
        Dispatch(repo, workflow, fields, loaded) => {
            children.push(description(
                "Workflow",
                &format!("{} ? {}", repo.full(), workflow.title),
            ));
            children.push(field("TextField", "ref", "Branch or tag", values));
            if !loaded.is_empty() {
                children.push(description("Inputs loaded for", loaded));
            }
            ids.push(actions.add("Load Inputs for Ref", Action::ReloadWorkflow));
            for input in fields {
                let id = format!("workflow:{}", input.name);
                let label = if input.required {
                    format!("{} (required)", input.name)
                } else {
                    input.name.clone()
                };
                let control = match input.kind.as_str() {
                    "choice" => dropdown(
                        &id,
                        &label,
                        values,
                        input.choices.iter().map(|c| (c.clone(), c.clone())),
                    ),
                    "boolean" => dropdown(
                        &id,
                        &label,
                        values,
                        opts(&[("", "Default"), ("true", "True"), ("false", "False")]),
                    ),
                    _ => field("TextField", &id, &label, values),
                };
                children.push(control);
                if !input.description.is_empty() {
                    children.push(description("", &input.description));
                }
            }
        }
        Download => {
            children.push(field(
                "TextField",
                "repository",
                "GitHub URL or owner/name",
                values,
            ));
            children.push(field(
                "TextField",
                "ref",
                "Branch, tag, or commit (optional)",
                values,
            ));
            children.push(field(
                "TextField",
                "directory",
                "Download directory (absolute path)",
                values,
            ));
        }
        Comment(row) => {
            children.push(description("GitHub item", &row.title));
            children.push(field("TextArea", "body", "Comment (Markdown)", values));
        }
        EditTitle(row) => {
            children.push(description("GitHub item", &row.title));
            children.push(field("TextField", "title", "New title", values));
        }
        LookupRepo(_) => children.push(field(
            "TextField",
            "repository",
            "Repository as owner/name",
            values,
        )),
    }
    if matches!(kind, Release(_) | Dispatch(..)) {
        for node in &mut children {
            if let Some(field) = node["props"]["id"].as_str().map(str::to_owned) {
                let id = format!("field-{}-{}", actions.revision, actions.map.len());
                actions.map.insert(id.clone(), Action::FieldChanged(field));
                node["props"]["actionId"] = json!(id);
            }
        }
    }
    json!({"type":"Form","props":{"actionIds":ids},"children":children})
}
fn open_actions(row: &Row, actions: &mut Actions) -> Vec<String> {
    if row.url.is_empty() {
        return vec![];
    }
    vec![
        actions.add("Open in Browser", Action::Open(row.url.clone())),
        actions.add("Copy URL", Action::Copy(row.url.clone())),
    ]
}
fn item_actions(row: &Row, actions: &mut Actions, prefs: &Value) -> Vec<String> {
    let mut ids = vec![];
    if prefs["isOpenInBrowser"] == true {
        ids.extend(open_actions(row, actions));
    }
    ids.push(actions.add("View Details", Action::Details(row.clone())));
    if prefs["isOpenInBrowser"] != true {
        ids.extend(open_actions(row, actions));
    }
    match row.kind.as_str() {
        "repository" => {
            if let Some(repo) = &row.repo {
                let clone = if prefs["repositoryCloneProtocol"] == "ssh" {
                    format!("git@github.com:{}.git", repo.full())
                } else {
                    format!("{}.git", repo.web())
                };
                ids.push(actions.add("Copy Clone URL", Action::Copy(clone)));
                ids.push(actions.add(
                    "Open in GitHub Codespaces Editor",
                    Action::Open(format!("https://github.dev/{}", repo.full())),
                ));
                ids.push(actions.add("Star Repository", Action::Change(row.clone(), Change::Star)));
                ids.push(actions.add(
                    "Unstar Repository",
                    Action::Change(row.clone(), Change::Unstar),
                ));
            }
        }
        "issue" | "pull" => {
            ids.push(actions.add("Add Comment", Action::Comment(row.clone())));
            ids.push(actions.add("Edit Title", Action::EditTitle(row.clone())));
            let (label, change) = if row.state == "open" {
                ("Close", Change::Close)
            } else {
                ("Reopen", Change::Open)
            };
            ids.push(actions.add(label, Action::Change(row.clone(), change)));
            if row.kind == "pull" && row.state == "open" {
                ids.push(actions.add(
                    "Merge Pull Request",
                    Action::Change(row.clone(), Change::Merge),
                ));
            }
        }
        "package" => ids.push(actions.add("Browse Versions", Action::Versions(row.clone()))),
        "notification" => {
            if row.detail["unread"] == true {
                ids.push(actions.add(
                    "Mark as Read",
                    Action::Change(row.clone(), Change::MarkRead),
                ));
            }
            ids.push(actions.add("Mark as Done", Action::Change(row.clone(), Change::Done)));
            ids.push(actions.add("Mute Thread", Action::Change(row.clone(), Change::Mute)));
        }
        "run" => {
            if row.detail["status"] == "completed" {
                ids.push(actions.add(
                    "Re-run Workflow",
                    Action::Change(row.clone(), Change::Rerun),
                ));
            } else {
                ids.push(actions.add(
                    "Cancel Workflow Run",
                    Action::Change(row.clone(), Change::Cancel),
                ));
            }
        }
        _ => {}
    }
    ids
}
fn list(list: &List, actions: &mut Actions, prefs: &Value, notice: &str) -> Value {
    let mut groups: Vec<(String, Vec<Value>)> = vec![];
    for row in &list.rows {
        let mut ids = match &list.kind {
            model::Kind::RepositoryPicker(purpose) => row
                .repo
                .as_ref()
                .map(|repo| {
                    vec![actions.add(
                        "Choose Repository",
                        Action::Choose(purpose.clone(), repo.clone()),
                    )]
                })
                .unwrap_or_default(),
            model::Kind::Workflows(repo) => vec![actions.add(
                "Configure Dispatch",
                Action::Workflow(repo.clone(), row.clone()),
            )],
            _ => item_actions(row, actions, prefs),
        };
        let node = json!({"type":"List.Item","props":{"id":row.id,"title":row.title,"subtitle":row.subtitle,
            "actionIds":ids,"accessories":if row.state.is_empty(){vec![]}else{vec![json!({"text":row.state})]}}});
        if let Some((_, entries)) = groups
            .iter_mut()
            .find(|(section, _)| section == &row.section)
        {
            entries.push(node);
        } else {
            groups.push((row.section.clone(), vec![node]));
        }
    }
    let mut children = Vec::new();
    for message in [notice, list.notice.as_str()]
        .into_iter()
        .filter(|message| !message.is_empty())
    {
        children.push(json!({"type":"List.Item","props":{"id":format!("notice-{}",children.len()),"title":message,"actionIds":[]}}));
    }
    for (section, entries) in groups {
        if section.is_empty() {
            children.extend(entries);
        } else {
            children
                .push(json!({"type":"List.Section","props":{"title":section},"children":entries}));
        }
    }
    if list.rows.is_empty() {
        children.push(json!({"type":"List.EmptyView","props":{"title":"No GitHub results","description":if list.uses_search(){"Enter a search or check your repository access."}else{"Refresh to check for new items."}}}));
    }
    let mut global = vec![];
    match &list.kind {
        model::Kind::RepositoryPicker(purpose) => {
            global.push(actions.add("Enter Repository Name", Action::DirectRepo(purpose.clone())))
        }
        model::Kind::Releases(repo) => {
            global.push(actions.add("Create Release", Action::CreateRelease(repo.clone())))
        }
        model::Kind::Notifications { unread } => {
            global.push(actions.add(
                if *unread {
                    "Show All Notifications"
                } else {
                    "Show Unread Notifications"
                },
                Action::Unread(!unread),
            ));
            if list.rows.iter().any(|r| r.detail["unread"] == true) {
                global.push(actions.add("Mark All as Read", Action::ReadAll));
            }
        }
        _ => {}
    }
    if list.has_more() {
        global.push(actions.add("Load More", Action::LoadMore));
    }
    global.push(actions.add("Refresh", Action::Refresh));
    json!({"type":"List","props":{"searchText":list.query,"filtering":!list.uses_search(),"__hasServerSearch":list.uses_search(),
        "pagination":{"hasMore":list.has_more()},"actionIds":global},"children":children})
}
fn detail(markdown: &str, ids: Vec<String>) -> Value {
    json!({"type":"Detail","props":{"markdown":markdown,"actionIds":ids}})
}

pub fn render(session: &mut Session, prefs: &Value) -> Value {
    session.revision = session.revision.wrapping_add(1);
    let mut actions = Actions {
        revision: session.revision,
        wire: vec![],
        map: BTreeMap::new(),
    };
    let (title, mut root) = match &session.screen {
        Screen::Setup => {
            let ids = vec![actions.add("Open Extension Preferences", Action::Settings)];
            let mut message="Set **Personal Access Token** in GitHub extension preferences. Reopen this command after saving.".to_owned();
            if !session.notice.is_empty() {
                message.push_str(&format!("\n\n{}", session.notice));
            }
            ("GitHub", detail(&message, ids))
        }
        Screen::List(state) => (
            state.title(),
            list(state, &mut actions, prefs, &session.notice),
        ),
        Screen::Form(kind, value) => (
            kind.title(),
            form(kind, value, &session.notice, &mut actions),
        ),
        Screen::Detail(row) => {
            let mut ids = item_actions(row, &mut actions, prefs);
            ids.retain(|id| !matches!(actions.map[id], Action::Details(_)));
            let markdown = format!("{}\n\n{}\n\n{}", row.subtitle, row.body, session.notice);
            (row.title.as_str(), detail(&markdown, ids))
        }
        Screen::Done { title, url, path } => {
            let mut ids = vec![];
            if !url.is_empty() {
                ids.push(actions.add("Open in Browser", Action::Open(url.clone())));
                ids.push(actions.add("Copy URL", Action::Copy(url.clone())));
            }
            if !path.is_empty() {
                ids.push(actions.add("Reveal ZIP", Action::Reveal(path.clone())));
                ids.push(actions.add("Copy ZIP Path", Action::Copy(path.clone())));
            }
            (
                title.as_str(),
                detail(&format!("{title}\n\n{path}\n\n{}", session.notice), ids),
            )
        }
    };
    if !session.history.is_empty() {
        let back = actions.add("Back", Action::Back);
        root["props"]["backActionId"] = json!(back);
        if let Some(ids) = root["props"]["actionIds"].as_array_mut() {
            ids.push(json!(back));
        }
    }
    if root["type"] == "List" {
        let global = root["props"]["actionIds"]
            .as_array()
            .cloned()
            .unwrap_or_default();
        fn attach(node: &mut Value, global: &[Value]) {
            if node["type"] == "List.Item" {
                if let Some(ids) = node["props"]["actionIds"].as_array_mut() {
                    ids.extend_from_slice(global);
                }
            } else if let Some(children) = node["children"].as_array_mut() {
                for child in children {
                    attach(child, global);
                }
            }
        }
        attach(&mut root, &global);
    }
    let mut view = tezbar_extension_sdk::view(title, root, actions.wire);
    view["refreshMs"] = Value::Null;
    if let Screen::List(list) = &session.screen {
        view["hasMore"] = json!(list.has_more());
    }
    session.actions = actions.map;
    view
}
