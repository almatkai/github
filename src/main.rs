mod api;
mod forms;
mod model;
mod tray;
mod ui;

use api::{text, Api, Transport};
use forms::Kind as Form;
use model::{Kind, List, Purpose, Repo, RepoMode, Row};
use reqwest::Method;
use serde_json::{json, Value};
use std::sync::OnceLock;
use tezbar_extension_sdk::{
    self as sdk, BoxFuture, CommandConcurrency, CoreResult, Extension, ExtensionContext,
};
use ui::{Action, Change, Screen, Session};

const ID: &str = "com.tezbar.native.github";
const COMMANDS: &[&str] = &[
    "my-pull-requests",
    "search-pull-requests",
    "create-pull-request",
    "my-issues",
    "search-issues",
    "create-issue",
    "create-branch",
    "create-release",
    "create-repository",
    "search-repositories",
    "my-latest-repositories",
    "my-starred-repositories",
    "my-packages",
    "workflow-runs",
    "dispatch-workflow",
    "notifications",
    "unread-notifications",
    "search-discussions",
    "my-discussions",
    "my-projects",
    "my-issues-menu",
    "my-stats-menu",
    "my-pull-requests-menu",
    "my-packages-menu",
    "download-repository",
];

struct Github {
    transport: Transport,
    trays: tray::Trays,
    root: OnceLock<ExtensionContext>,
    menu_auth: tokio::sync::Mutex<()>,
}

fn command_list(command: &str) -> Option<Kind> {
    Some(match command {
        "my-pull-requests" => Kind::Issues {
            pull: true,
            mine: true,
        },
        "search-pull-requests" => Kind::Issues {
            pull: true,
            mine: false,
        },
        "my-issues" => Kind::Issues {
            pull: false,
            mine: true,
        },
        "search-issues" => Kind::Issues {
            pull: false,
            mine: false,
        },
        "create-pull-request" => Kind::RepositoryPicker(Purpose::Pull),
        "create-issue" => Kind::RepositoryPicker(Purpose::Issue),
        "create-branch" => Kind::RepositoryPicker(Purpose::Branch),
        "create-release" => Kind::RepositoryPicker(Purpose::Release),
        "workflow-runs" => Kind::RepositoryPicker(Purpose::Runs),
        "dispatch-workflow" => Kind::RepositoryPicker(Purpose::Dispatch),
        "search-repositories" => Kind::Repositories(RepoMode::Search),
        "my-latest-repositories" => Kind::Repositories(RepoMode::Latest),
        "my-starred-repositories" => Kind::Repositories(RepoMode::Starred),
        "my-packages" => Kind::Packages,
        "notifications" => Kind::Notifications { unread: true },
        "search-discussions" => Kind::Discussions(false),
        "my-discussions" => Kind::Discussions(true),
        "my-projects" => Kind::Projects,
        _ => return None,
    })
}

async fn settings(ctx: &ExtensionContext) -> CoreResult {
    ctx.host(
        "host.settings.open",
        json!({"section":"settings-extensions"}),
    )
    .await
}
async fn open(ctx: &ExtensionContext, raw: &str) -> CoreResult {
    let url = model::web(raw).ok_or("GitHub did not return a supported web link.")?;
    sdk::open(ctx, &url).await
}

async fn chosen(
    api: &Api,
    prefs: &Value,
    session: &mut Session,
    purpose: Purpose,
    repo: Repo,
) -> Result<(), String> {
    let screen = match purpose {
        Purpose::Runs | Purpose::Dispatch | Purpose::Release => {
            let kind = match purpose {
                Purpose::Runs => Kind::Runs(repo),
                Purpose::Dispatch => Kind::Workflows(repo),
                _ => Kind::Releases(repo),
            };
            let mut list = List::new(kind);
            list.load(api, prefs, &session.login, false).await?;
            Screen::List(list)
        }
        _ => {
            let values = forms::defaults(api, &repo, &purpose).await?;
            let kind = match purpose {
                Purpose::Pull => Form::Pull(repo),
                Purpose::Issue => Form::Issue(repo),
                _ => Form::Branch(repo),
            };
            Screen::Form(kind, values)
        }
    };
    session.push(screen)
}

fn number(row: &Row) -> Result<String, String> {
    if row.number == 0 {
        return Err("GitHub did not return an issue or pull request number.".into());
    }
    Ok(row.number.to_string())
}
fn numeric_id(value: &str) -> Result<&str, String> {
    if value.parse::<u64>().ok().is_none_or(|id| id == 0) {
        Err("GitHub did not return a valid numeric ID.".into())
    } else {
        Ok(value)
    }
}
async fn details(api: &Api, mut row: Row) -> Result<Row, String> {
    if matches!(row.kind.as_str(), "issue" | "pull") {
        let repo = row
            .repo
            .as_ref()
            .ok_or("GitHub did not return the repository.")?;
        let number = number(&row)?;
        let suffix = if row.kind == "pull" {
            "pulls"
        } else {
            "issues"
        };
        let item = api.get(&repo.for_path(&[suffix, &number]), &[]).await?;
        row.body = model::field(&item, "body", 25_000);
        row.state = model::field(&item, "state", 32);
        row.title = model::field(&item, "title", 300);
        if let Ok((remarks, _)) = api
            .page(&repo.for_path(&["issues", &number, "comments"]), &[], 1, 10)
            .await
        {
            for remark in remarks.as_array().into_iter().flatten() {
                let author = model::field(&remark["user"], "login", 60);
                let body = model::field(remark, "body", 3000);
                row.body
                    .push_str(&format!("\n\n---\n\n**{author}**\n\n{body}"));
            }
        }
    }
    if row.kind == "run" {
        let repo = row
            .repo
            .as_ref()
            .ok_or("GitHub did not return the workflow repository.")?;
        let id = numeric_id(&row.id)?;
        let (data, _) = api
            .page(&repo.for_path(&["actions", "runs", id, "jobs"]), &[], 1, 20)
            .await?;
        let mut body = String::new();
        for job in data["jobs"].as_array().into_iter().flatten() {
            let state = job["conclusion"]
                .as_str()
                .filter(|v| !v.is_empty())
                .unwrap_or_else(|| text(job, "status"));
            body.push_str(&format!(
                "- {} ? {}\n",
                model::field(job, "name", 120),
                state
            ));
        }
        row.body = body;
    }
    Ok(row)
}

async fn change(
    ctx: &ExtensionContext,
    api: &Api,
    session: &mut Session,
    row: Row,
    change: Change,
) -> Result<(), String> {
    let label = match change {
        Change::Open => "Reopen Item",
        Change::Close => "Close Item",
        Change::Merge => "Merge Pull Request",
        Change::Star => "Star Repository",
        Change::Unstar => "Unstar Repository",
        Change::MarkRead => "Mark as Read",
        Change::Done => "Mark as Done",
        Change::Mute => "Mute Thread",
        Change::Rerun => "Re-run Workflow",
        Change::Cancel => "Cancel Workflow Run",
    };
    let merge_sha = if matches!(change, Change::Merge) {
        let repo = row
            .repo
            .as_ref()
            .ok_or("GitHub did not return the repository.")?;
        let n = number(&row)?;
        let info = api.get(&repo.for_path(&["pulls", &n]), &[]).await?;
        Some(forms::sha(text(&info["head"], "sha"))?.to_owned())
    } else {
        None
    };
    let message = if let Some(sha) = &merge_sha {
        format!("{label} ? {} at commit {}?", row.title, &sha[..12])
    } else {
        format!("{label} ? {}?", row.title)
    };
    if !matches!(change, Change::Star | Change::MarkRead)
        && !forms::confirm(ctx, label, &message).await?
    {
        return Ok(());
    }
    match change {
        Change::Star | Change::Unstar => {
            let repo = row
                .repo
                .as_ref()
                .ok_or("GitHub did not return the repository.")?;
            let method = if matches!(change, Change::Star) {
                Method::PUT
            } else {
                Method::DELETE
            };
            api.mutate(method, &["user", "starred", &repo.owner, &repo.name], None)
                .await?;
            if matches!(change, Change::Unstar) {
                if let Screen::List(list) = &mut session.screen {
                    if matches!(list.kind, Kind::Repositories(RepoMode::Starred)) {
                        list.rows.retain(|r| r.id != row.id);
                    }
                }
            }
        }
        Change::Open | Change::Close | Change::Merge => {
            let repo = row
                .repo
                .as_ref()
                .ok_or("GitHub did not return the repository.")?;
            let n = number(&row)?;
            if matches!(change, Change::Merge) {
                let reply = api
                    .mutate(
                        Method::PUT,
                        &repo.for_path(&["pulls", &n, "merge"]),
                        Some(json!({"merge_method":"merge","sha":merge_sha})),
                    )
                    .await?;
                if reply["merged"] != true {
                    return Err(
                        "GitHub declined the merge. Inspect the pull request before trying again."
                            .into(),
                    );
                }
            } else {
                let state = if matches!(change, Change::Close) {
                    "closed"
                } else {
                    "open"
                };
                let suffix = if row.kind == "pull" {
                    "pulls"
                } else {
                    "issues"
                };
                let reply = api
                    .mutate(
                        Method::PATCH,
                        &repo.for_path(&[suffix, &n]),
                        Some(json!({"state":state})),
                    )
                    .await?;
                if reply["state"] != state {
                    return Err(
                        "GitHub did not confirm the change. Check the item before trying again."
                            .into(),
                    );
                }
            }
            let state = match change {
                Change::Open => "open",
                Change::Merge => "merged",
                _ => "closed",
            };
            if let Screen::Detail(item) = &mut session.screen {
                item.state = state.into();
            }
            update_row_state(&mut session.screen, &row, state);
            for screen in &mut session.history {
                update_row_state(screen, &row, state);
            }
        }
        Change::MarkRead | Change::Done | Change::Mute => {
            let id = numeric_id(&row.id)?;
            notify_thread(api, id, &change).await?;
            let remove = matches!(change, Change::Done | Change::Mute);
            update_notification(&mut session.screen, &row.id, remove);
            for screen in &mut session.history {
                update_notification(screen, &row.id, remove);
            }
        }
        Change::Rerun | Change::Cancel => {
            let repo = row
                .repo
                .as_ref()
                .ok_or("GitHub did not return the workflow repository.")?;
            let id = numeric_id(&row.id)?;
            let op = if matches!(change, Change::Rerun) {
                "rerun"
            } else {
                "cancel"
            };
            api.mutate(
                Method::POST,
                &repo.for_path(&["actions", "runs", id, op]),
                None,
            )
            .await?;
        }
    }
    session.notice = format!("{label} request completed.");
    Ok(())
}

fn update_row_state(screen: &mut Screen, row: &Row, state: &str) {
    if let Screen::List(list) = screen {
        if let Some(item) = list
            .rows
            .iter_mut()
            .find(|item| item.id == row.id && item.kind == row.kind)
        {
            item.state = state.into();
        }
    }
}
fn update_notification(screen: &mut Screen, id: &str, remove: bool) {
    if let Screen::List(list) = screen {
        if let Kind::Notifications { unread } = list.kind {
            if remove || unread {
                list.rows.retain(|item| item.id != id);
            } else if let Some(item) = list.rows.iter_mut().find(|item| item.id == id) {
                item.state = "Read".into();
                item.detail["unread"] = json!(false);
            }
        }
    }
    if let Screen::Detail(item) = screen {
        if item.kind == "notification" && item.id == id {
            item.state = "Read".into();
            item.detail["unread"] = json!(false);
        }
    }
}
async fn notify_thread(api: &Api, id: &str, change: &Change) -> Result<(), String> {
    let mut path = vec!["notifications", "threads", id];
    let (method, data) = match change {
        Change::MarkRead => (Method::PATCH, None),
        Change::Done => (Method::DELETE, None),
        _ => {
            path.push("subscription");
            (
                Method::PUT,
                Some(json!({"ignored":true,"subscribed":false})),
            )
        }
    };
    api.mutate(method, &path, data)
        .await
        .map_err(String::from)?;
    Ok(())
}

async fn perform(
    ctx: &ExtensionContext,
    api: &Api,
    prefs: &Value,
    session: &mut Session,
    action: Action,
) -> Result<(), String> {
    match action {
        Action::Settings => {
            settings(ctx).await?;
        }
        Action::Back => session.back(),
        Action::Open(target) => {
            open(ctx, &target).await?;
        }
        Action::Copy(text) => {
            sdk::copy(ctx, &text).await?;
        }
        Action::Reveal(path) => {
            ctx.host("host.shell.reveal", json!({"path":path})).await?;
        }
        Action::FieldChanged(_) => unreachable!("field change handled before rendering"),
        Action::Refresh | Action::LoadMore => {
            if let Screen::List(list) = &mut session.screen {
                let mut next = list.clone();
                next.load(
                    api,
                    prefs,
                    &session.login,
                    matches!(action, Action::LoadMore),
                )
                .await?;
                *list = next;
            }
        }
        Action::Choose(purpose, repo) => chosen(api, prefs, session, purpose, repo).await?,
        Action::DirectRepo(purpose) => {
            session.push(Screen::Form(Form::LookupRepo(purpose), json!({})))?
        }
        Action::CreateRelease(repo) => {
            let values = forms::defaults(api, &repo, &Purpose::Release).await?;
            session.push(Screen::Form(Form::Release(repo), values))?;
        }
        Action::Workflow(repo, row) => {
            let info = api.get(&repo.path(), &[]).await?;
            let reference = forms::reference(text(&info, "default_branch"))?.to_owned();
            let (fields, loaded) = match forms::load_inputs(api, &repo, &row, &reference).await {
                Ok(fields) => (fields, reference.clone()),
                Err(error) => {
                    session.notice = error;
                    (vec![], String::new())
                }
            };
            session.push(Screen::Form(
                Form::Dispatch(repo, row, fields, loaded),
                json!({"ref":reference}),
            ))?;
        }
        Action::Details(row) => session.push(Screen::Detail(details(api, row).await?))?,
        Action::Versions(row) => {
            let mut list = List::new(Kind::Versions(row));
            list.load(api, prefs, &session.login, false).await?;
            session.push(Screen::List(list))?;
        }
        Action::Comment(row) => session.push(Screen::Form(Form::Comment(row), json!({})))?,
        Action::EditTitle(row) => {
            let value = json!({"title":row.title});
            session.push(Screen::Form(Form::EditTitle(row), value))?;
        }
        Action::GenerateNotes => {
            if let Screen::Form(Form::Release(repo), values) = &mut session.screen {
                let notes = forms::generate_notes(api, repo, values).await?;
                values["body"] = json!(notes);
            }
        }
        Action::ReloadWorkflow => {
            if let Screen::Form(Form::Dispatch(repo, row, old, loaded), values) =
                &mut session.screen
            {
                let reference = forms::reference(text(values, "ref"))?.to_owned();
                let current = forms::load_inputs(api, repo, row, &reference).await?;
                // Input definitions may change by ref. Preserve only values
                // that remain present in the validated input schema.
                let valid = current
                    .iter()
                    .map(|input| format!("workflow:{}", input.name))
                    .collect::<Vec<_>>();
                if let Some(values) = values.as_object_mut() {
                    values.retain(|key, _| !key.starts_with("workflow:") || valid.contains(key));
                }
                *old = current;
                *loaded = reference;
            }
        }
        Action::Unread(unread) => {
            let mut list = List::new(Kind::Notifications { unread });
            list.load(api, prefs, &session.login, false).await?;
            session.screen = Screen::List(list);
        }
        Action::Change(row, op) => change(ctx, api, session, row, op).await?,
        Action::ReadAll => {
            if forms::confirm(
                ctx,
                "Mark All as Read",
                "Mark all GitHub notifications as read, including notifications outside this view?",
            )
            .await?
            {
                let stamp = chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Secs, true);
                let status = api.mark_all_read(&stamp).await?;
                if status == 202 {
                    session.notice = "GitHub queued the request. Refresh to check for remaining unread notifications.".into();
                    if let Screen::List(list) = &mut session.screen {
                        let mut next = list.clone();
                        if next.load(api, prefs, &session.login, false).await.is_ok() {
                            *list = next;
                        }
                    }
                    return Ok(());
                }
                if let Screen::List(list) = &mut session.screen {
                    if matches!(list.kind, Kind::Notifications { unread: true }) {
                        list.rows.clear();
                        list.next = Value::Null;
                    } else {
                        for row in &mut list.rows {
                            row.state = "Read".into();
                            row.detail["unread"] = json!(false);
                        }
                    }
                }
                session.notice = "GitHub notifications marked as read.".into();
            }
        }
        Action::Submit => {
            let Screen::Form(kind, draft) = session.screen.clone() else {
                return Err("GitHub form has ended.".into());
            };
            if let Form::LookupRepo(purpose) = kind {
                let repo = Repo::parse(text(&draft, "repository"))?;
                chosen(api, prefs, session, purpose, repo).await?;
                if session
                    .history
                    .last()
                    .is_some_and(|screen| matches!(screen, Screen::Form(Form::LookupRepo(_), _)))
                {
                    session.history.pop();
                }
            } else if let Some(outcome) =
                forms::write(&kind, api, ctx, &draft, &session.login).await?
            {
                let url = outcome.url.clone();
                let open_after =
                    matches!(kind, Form::Issue(_)) && prefs["openInBrowserAfterCreate"] == true;
                session.finish(outcome);
                if open_after && open(ctx, &url).await.is_err() {
                    session.notice =
                        "The issue was created. Tezbar could not open the browser.".into();
                }
            }
        }
    }
    Ok(())
}

fn preserve_draft(session: &mut Session, form: &Value) {
    if let (Screen::Form(_, draft), Some(values)) = (&mut session.screen, form.as_object()) {
        for (key, value) in values {
            if key.len() <= 150 && value.as_str().is_some_and(|value| value.len() <= 160_000)
                || key.len() <= 150 && value.is_boolean()
            {
                draft[key] = value.clone();
            }
        }
    }
}
async fn initial(
    app: &Github,
    ctx: &ExtensionContext,
    command: &str,
    payload: &Value,
) -> Result<Session, Value> {
    let prefs = &payload["preferences"];
    let token = text(prefs, "personalAccessToken");
    let menu_auth = app.menu_auth.lock().await;
    let candidate = app.transport.auth(token);
    app.trays
        .invalidate_changed(ctx, candidate.as_ref().ok())
        .await;
    // Serialize a menu install with credential rotation across all five lanes.
    let _menu_install = if tray::MENUS.contains(&command) {
        Some(menu_auth)
    } else {
        drop(menu_auth);
        None
    };
    let api = match candidate {
        Ok(api) => api,
        Err(error) => {
            let mut setup = Session::new(Screen::Setup, String::new());
            if !token.trim().is_empty() {
                setup.notice = error.to_string();
            }
            return Ok(setup);
        }
    };
    let identity = match api.get(&["user"], &[]).await {
        Ok(identity) => identity,
        Err(error) => {
            let mut setup = Session::new(Screen::Setup, String::new());
            setup.notice = error.to_string();
            return Ok(setup);
        }
    };
    let login = text(&identity, "login");
    if !model::account(login) {
        let mut setup = Session::new(Screen::Setup, String::new());
        setup.notice = "GitHub did not return the authenticated account.".into();
        return Ok(setup);
    }
    if tray::MENUS.contains(&command) {
        let result = if let Some(root) = app.root.get() {
            app.trays
                .run(ctx, root, command, api, login.into(), prefs)
                .await
        } else {
            Err("GitHub background context is unavailable.".into())
        };
        return Err(result.unwrap_or_else(|error| sdk::no_view(&error)));
    }
    let mut notice = String::new();
    let screen = if command == "create-repository" {
        let (owners, status) = forms::owners(&api, login).await;
        notice = status;
        Screen::Form(
            Form::Repository(owners),
            json!({"owner":login,"private":true,"autoInit":false}),
        )
    } else if command == "download-repository" {
        let clipboard = ctx
            .host("host.clipboard.read", json!({"format":"text"}))
            .await
            .ok();
        let value = clipboard
            .as_ref()
            .and_then(Value::as_str)
            .filter(|value| value.starts_with("https://github.com/"))
            .unwrap_or_default();
        Screen::Form(
            Form::Download,
            json!({"repository":value,"directory":text(prefs,"baseClonePath")}),
        )
    } else if let Some(kind) = command_list(command) {
        let mut list = List::new(kind);
        let initial_query = text(payload, "searchText");
        if !initial_query.is_empty() {
            list.query = initial_query.chars().take(500).collect();
        } else if matches!(
            command,
            "search-discussions" | "search-pull-requests" | "search-issues"
        ) {
            list.query = text(prefs, "defaultSearchTerms")
                .chars()
                .take(500)
                .collect();
        }
        if let Err(error) = list.load(&api, prefs, login, false).await {
            notice = error;
        }
        Screen::List(list)
    } else {
        Screen::Setup
    };
    let mut session = Session::new(screen, login.into());
    session.notice = notice;
    Ok(session)
}

impl Extension for Github {
    fn id(&self) -> &'static str {
        ID
    }
    fn version(&self) -> &'static str {
        env!("CARGO_PKG_VERSION")
    }
    fn commands(&self) -> &'static [&'static str] {
        COMMANDS
    }
    fn concurrency(&self, _: &str) -> CommandConcurrency {
        CommandConcurrency::RejectWhileBusy
    }
    fn initialize<'a>(&'a self, ctx: &'a ExtensionContext) -> BoxFuture<'a, CoreResult> {
        Box::pin(async move {
            let _ = self.root.set(ctx.clone());
            Ok(json!({"ready":true}))
        })
    }
    fn background_activate<'a>(
        &'a self,
        ctx: &'a ExtensionContext,
        payload: &'a Value,
    ) -> BoxFuture<'a, CoreResult> {
        Box::pin(self.trays.activate(ctx, payload))
    }
    fn background_event<'a>(
        &'a self,
        ctx: &'a ExtensionContext,
        payload: &'a Value,
    ) -> BoxFuture<'a, CoreResult> {
        Box::pin(self.trays.event(ctx, payload))
    }
    fn handle<'a>(
        &'a self,
        ctx: &'a ExtensionContext,
        command: &'a str,
        op: &'a str,
        payload: &'a Value,
        state: &'a mut Value,
    ) -> BoxFuture<'a, CoreResult> {
        Box::pin(async move {
            if op == "extension:dispose-session" {
                return Ok(sdk::unchanged());
            }
            let prefs = &payload["preferences"];
            let mut session: Session = if op == "extension:run-command" {
                match initial(self, ctx, command, payload).await {
                    Ok(session) => session,
                    Err(view) => return Ok(view),
                }
            } else {
                serde_json::from_value(state.clone()).map_err(|_| "GitHub session has ended.")?
            };
            if op != "extension:run-command" {
                if matches!(session.screen, Screen::Setup) {
                    if op == "extension:invoke-action"
                        && matches!(
                            session.actions.get(text(payload, "actionId")),
                            Some(Action::Settings)
                        )
                    {
                        settings(ctx).await?;
                    }
                    return Ok(sdk::unchanged());
                }
                let api = self.transport.auth(text(prefs, "personalAccessToken"))?;
                session.notice.clear();
                match op {
                    "extension:invoke-action" => {
                        let Some(action) = session.actions.get(text(payload, "actionId")).cloned()
                        else {
                            return Ok(sdk::unchanged());
                        };
                        if let Action::FieldChanged(field) = action {
                            let value = &payload["formValues"]["value"];
                            if value.is_boolean()
                                || value.as_str().is_some_and(|s| s.len() <= 160_000)
                            {
                                if let Screen::Form(_, draft) = &mut session.screen {
                                    draft[field] = value.clone();
                                    *state = serde_json::to_value(session)
                                        .map_err(|_| "Could not save GitHub draft.")?;
                                }
                            }
                            return Ok(sdk::unchanged());
                        }
                        preserve_draft(&mut session, &payload["formValues"]);
                        if let Err(error) = perform(ctx, &api, prefs, &mut session, action).await {
                            session.notice = error;
                        }
                    }
                    "extension:load-more"
                    | "extension:refresh-session"
                    | "extension:search-text-changed" => {
                        let Screen::List(list) = &mut session.screen else {
                            return Ok(sdk::unchanged());
                        };
                        let mut next = list.clone();
                        if op == "extension:search-text-changed" {
                            if !list.uses_search() {
                                return Ok(sdk::unchanged());
                            }
                            next.query = text(payload, "searchText").chars().take(500).collect();
                        }
                        match next
                            .load(&api, prefs, &session.login, op == "extension:load-more")
                            .await
                        {
                            Ok(()) => *list = next,
                            Err(error) => session.notice = error,
                        }
                    }
                    _ => return Ok(sdk::unchanged()),
                }
            }
            let result = ui::render(&mut session, prefs);
            *state = serde_json::to_value(session)
                .map_err(|_| "Could not save the GitHub view state.")?;
            Ok(result)
        })
    }
}

#[tokio::main(worker_threads = 2)]
async fn main() {
    match std::env::args().nth(1).as_deref() {
        Some("--protocol-info") => {
            println!(
                "{}",
                json!({"id":ID,"version":env!("CARGO_PKG_VERSION"),"protocol":1,"commands":COMMANDS})
            );
            return;
        }
        Some("--build-info") => {
            println!("{}", json!({"fixture":cfg!(feature="fixture")}));
            return;
        }
        _ => {}
    }
    let result = match Transport::new() {
        Ok(transport) => {
            sdk::serve(Github {
                transport,
                trays: tray::Trays::new(),
                root: OnceLock::new(),
                menu_auth: Default::default(),
            })
            .await
        }
        Err(error) => Err(error),
    };
    if let Err(error) = result {
        eprintln!("GitHub extension: {error}");
        std::process::exit(1);
    }
}
