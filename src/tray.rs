use crate::{
    api::{text, Api},
    model::{self, Kind, List, Row},
};
use reqwest::Method;
use serde_json::{json, Value};
use std::{
    collections::BTreeMap,
    sync::{
        atomic::{AtomicBool, AtomicU64, Ordering},
        Arc,
    },
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};
use tezbar_extension_sdk::{self as sdk, CoreResult, ExtensionContext};
use tokio::sync::{Mutex, Notify};

pub const MENUS: [&str; 5] = [
    "unread-notifications",
    "my-issues-menu",
    "my-stats-menu",
    "my-pull-requests-menu",
    "my-packages-menu",
];
const ICON: &str = "resources/assets/icon.png";
static NEXT_START: AtomicU64 = AtomicU64::new(1);

#[derive(Clone)]
pub struct Trays {
    lanes: Arc<[Lane; 5]>,
}

#[derive(Default)]
struct Lane {
    menu: Mutex<Option<Menu>>,
    wake: Notify,
    running: AtomicBool,
}

#[derive(Clone)]
enum MenuAction {
    Configure,
    Disable,
    Refresh,
    Open(String),
    Read(String),
}

#[derive(Clone)]
struct Menu {
    generation: String,
    revision: u64,
    callbacks: BTreeMap<String, MenuAction>,
    refreshed: Instant,
    api: Option<Api>,
    login: String,
    prefs: Value,
    rows: Vec<Row>,
    stats: Value,
    error: String,
}
impl Menu {
    fn new() -> Self {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_or(0, |v| v.as_nanos());
        let index = NEXT_START.fetch_add(1, Ordering::Relaxed);
        Self {
            generation: format!("{nanos:x}-{index}"),
            revision: 0,
            callbacks: BTreeMap::new(),
            refreshed: Instant::now(),
            api: None,
            login: String::new(),
            prefs: json!({}),
            rows: vec![],
            stats: Value::Null,
            error: String::new(),
        }
    }
    fn callback_id(&self, revision: u64, slot: usize) -> String {
        format!("a.{}.{revision:x}.{slot:x}", self.generation)
    }
    fn count(&self) -> usize {
        self.prefs["maxitems"]
            .as_u64()
            .or_else(|| text(&self.prefs, "maxitems").parse().ok())
            .unwrap_or(12)
            .clamp(1, 35) as usize
    }
}
impl Trays {
    pub fn new() -> Self {
        Self {
            lanes: Arc::new(std::array::from_fn(|_| Lane::default())),
        }
    }
    fn slot(key: &str) -> Result<usize, String> {
        MENUS
            .iter()
            .position(|menu| *menu == key)
            .ok_or_else(|| "Unknown GitHub menu.".into())
    }

    pub async fn invalidate_changed(&self, ctx: &ExtensionContext, candidate: Option<&Api>) {
        for (index, key) in MENUS.iter().enumerate() {
            let mut guard = self.lanes[index].menu.lock().await;
            if let Some(menu) = guard.as_mut() {
                if menu
                    .api
                    .as_ref()
                    .is_some_and(|old| !candidate.is_some_and(|new| old.same_credentials(new)))
                {
                    Self::disconnect(ctx, key, menu).await;
                    self.lanes[index].wake.notify_one();
                }
            }
        }
    }

    async fn disconnect(ctx: &ExtensionContext, key: &str, menu: &mut Menu) {
        // Fence callbacks and erase private data before either host call can fail.
        *menu = Menu::new();
        if publish(ctx, key, menu).await.is_err() {
            let _ = ctx
                .host("host.tray.remove", json!({"resourceId":key}))
                .await;
        }
    }

    pub async fn run(
        &self,
        ctx: &ExtensionContext,
        root: &ExtensionContext,
        key: &str,
        api: Api,
        login: String,
        prefs: &Value,
    ) -> CoreResult {
        let index = Self::slot(key)?;
        self.ensure_worker(root, index)?;
        let mut guard = self.lanes[index].menu.lock().await;
        if let Some(menu) = guard.as_mut() {
            if menu.api.is_some() && menu.login != login {
                Self::disconnect(ctx, key, menu).await;
            }
        }
        let old = guard.as_ref();
        let mut next = old.cloned().unwrap_or_else(Menu::new);
        let first = old.is_none();
        next.api.get_or_insert(api);
        next.login = login;
        // Avoid copying the token even into the menu preference map.
        next.prefs = prefs
            .as_object()
            .map(|v| {
                Value::Object(
                    v.iter()
                        .filter(|(key, _)| *key != "personalAccessToken")
                        .map(|(key, value)| (key.clone(), value.clone()))
                        .collect(),
                )
            })
            .unwrap_or_else(|| json!({}));
        match refresh(key, &mut next).await {
            Ok(()) => next.error.clear(),
            Err(error) => next.error = error,
        }
        next.refreshed = Instant::now();
        if let Some(menu) = guard.as_mut() {
            menu.refreshed = next.refreshed;
        }
        if first {
            retain(ctx, key).await?;
        }
        self.lanes[index].wake.notify_one();
        if let Err(error) = publish(ctx, key, &mut next).await {
            if first {
                let _ = ctx
                    .host("host.background.release", json!({"key":key}))
                    .await;
            }
            return Err(error);
        }
        *guard = Some(next);
        Ok(sdk::no_view("GitHub menu enabled"))
    }

    pub async fn activate(&self, ctx: &ExtensionContext, payload: &Value) -> CoreResult {
        let key = text(payload, "key");
        let index = Self::slot(key)?;
        if payload["data"]["format"] != 1 || payload["data"]["menu"] != key {
            return Err("Invalid GitHub menu registration.".into());
        }
        let mut guard = self.lanes[index].menu.lock().await;
        if guard.is_some() {
            return Ok(json!({"active":true}));
        }
        let mut menu = Menu::new();
        retain(ctx, key).await?;
        publish(ctx, key, &mut menu).await?;
        *guard = Some(menu);
        Ok(json!({"active":true}))
    }

    fn ensure_worker(&self, root: &ExtensionContext, index: usize) -> Result<(), String> {
        let lane = &self.lanes[index];
        if lane.running.swap(true, Ordering::AcqRel) {
            return Ok(());
        }
        let trays = self.clone();
        let context = root.clone();
        if let Err(error) = root.spawn(async move { trays.poll(context, index).await }) {
            lane.running.store(false, Ordering::Release);
            return Err(error);
        }
        Ok(())
    }

    async fn poll(&self, ctx: ExtensionContext, index: usize) {
        let key = MENUS[index];
        let lane = &self.lanes[index];
        loop {
            let active = {
                let guard = lane.menu.lock().await;
                guard
                    .as_ref()
                    .filter(|menu| menu.api.is_some())
                    .map(|menu| {
                        (
                            menu.generation.clone(),
                            period(key, menu).saturating_sub(menu.refreshed.elapsed()),
                        )
                    })
            };
            let Some((generation, duration)) = active else {
                lane.wake.notified().await;
                continue;
            };
            tokio::select! {
                _=lane.wake.notified()=>continue,
                _=tokio::time::sleep(duration)=>{}
            }
            let mut current = lane.menu.lock().await;
            if let Some(menu) = current.as_mut().filter(|menu| {
                menu.generation == generation
                    && menu.api.is_some()
                    && menu.refreshed.elapsed() >= period(key, menu)
            }) {
                let _ = refresh_and_publish(&ctx, key, menu).await;
            }
        }
    }

    pub async fn event(&self, ctx: &ExtensionContext, payload: &Value) -> CoreResult {
        let key = text(payload, "resourceId");
        let index = Self::slot(key)?;
        let mut guard = self.lanes[index].menu.lock().await;
        let menu = guard.as_mut().ok_or("This GitHub menu has ended.")?;
        let action = menu
            .callbacks
            .get(text(payload, "actionId"))
            .cloned()
            .ok_or("This GitHub menu changed. Reopen it before choosing an action.")?;
        match action {
            MenuAction::Configure => {
                ctx.host(
                    "host.settings.open",
                    json!({"section":"settings-extensions"}),
                )
                .await?;
            }
            MenuAction::Disable => {
                ctx.host("host.tray.remove", json!({"resourceId":key}))
                    .await?;
                if let Err(error) = ctx
                    .host("host.background.release", json!({"key":key}))
                    .await
                {
                    let _ = publish(ctx, key, menu).await;
                    return Err(error);
                }
                *guard = None;
                self.lanes[index].wake.notify_one();
            }
            MenuAction::Refresh => {
                self.lanes[index].wake.notify_one();
                refresh_and_publish(ctx, key, menu).await?;
            }
            MenuAction::Open(url) => {
                let target = model::web(&url).ok_or("GitHub did not return a link.")?;
                sdk::open(ctx, &target).await?;
            }
            MenuAction::Read(thread) => {
                let api = menu
                    .api
                    .as_ref()
                    .ok_or("Run the menu command to reconnect.")?;
                api.mutate(Method::PATCH, &["notifications", "threads", &thread], None)
                    .await?;
                let mut next = menu.clone();
                next.rows.retain(|row| row.id != thread);
                publish(ctx, key, &mut next).await?;
                *menu = next;
            }
        }
        Ok(json!({"ok":true}))
    }
}
fn period(key: &str, menu: &Menu) -> Duration {
    let seconds = if key == "my-stats-menu" {
        match text(&menu.prefs, "refreshFrequency") {
            "15m" => 900,
            "1h" => 3600,
            "2h" => 7200,
            _ => 1800,
        }
    } else if key == "unread-notifications" {
        return notification_period(menu.api.as_ref().map_or(0, Api::poll_interval));
    } else {
        600
    };
    Duration::from_secs(seconds)
}
pub(crate) fn notification_period(minimum: u64) -> Duration {
    Duration::from_secs(300.max(minimum))
}
async fn retain(ctx: &ExtensionContext, key: &str) -> Result<(), String> {
    let reply = ctx
        .host(
            "host.background.retain",
            json!({"key":key,"durable":true,"data":{"format":1,"menu":key}}),
        )
        .await?;
    if reply["retained"] != true {
        return Err("Tezbar could not retain this GitHub menu.".into());
    }
    Ok(())
}
async fn refresh_and_publish(
    ctx: &ExtensionContext,
    key: &str,
    menu: &mut Menu,
) -> Result<(), String> {
    let mut next = menu.clone();
    match refresh(key, &mut next).await {
        Ok(()) => next.error.clear(),
        Err(error) => next.error = error,
    }
    next.refreshed = Instant::now();
    menu.refreshed = next.refreshed;
    publish(ctx, key, &mut next).await?;
    if key == "my-stats-menu" && next.prefs["notifyOnNewStars"] != false && next.error.is_empty() {
        if let (Some(old), Some(new)) = (menu.stats["stars"].as_u64(), next.stats["stars"].as_u64())
        {
            if new > old {
                let increment = new - old;
                let _=ctx.host("host.notifications.show",json!({"id":"github-new-stars","title":"New GitHub stars","body":format!("Your repositories gained {increment} stars.")})).await;
            }
        }
    }
    *menu = next;
    Ok(())
}
async fn refresh(key: &str, menu: &mut Menu) -> Result<(), String> {
    let api = menu
        .api
        .as_ref()
        .ok_or("Run the menu command to reconnect.")?;
    if key == "my-stats-menu" {
        let data=api.graph("query ViewerStats{viewer{login url followers{totalCount} following{totalCount} pullRequests(states:[OPEN]){totalCount} issues(states:[OPEN]){totalCount} owned:repositories(first:100,ownerAffiliations:OWNER,orderBy:{field:STARGAZERS,direction:DESC}){totalCount nodes{stargazerCount forkCount nameWithOwner url}}} rateLimit{remaining limit}}",json!({})).await?;
        let viewer = &data["viewer"];
        let stars = viewer["owned"]["nodes"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|row| row["stargazerCount"].as_u64())
            .fold(0u64, u64::saturating_add);
        menu.stats = json!({"owner":viewer["login"],"url":viewer["url"],"followers":viewer["followers"]["totalCount"],
            "following":viewer["following"]["totalCount"],"stars":stars,"prsOpen":viewer["pullRequests"]["totalCount"],
            "issuesOpen":viewer["issues"]["totalCount"],"repositories":viewer["owned"]["totalCount"],"rateLimit":data["rateLimit"]["remaining"]});
        menu.rows = vec![];
        return Ok(());
    }
    let kind = match key {
        "unread-notifications" => Kind::Notifications { unread: true },
        "my-issues-menu" => Kind::Issues {
            pull: false,
            mine: true,
        },
        "my-pull-requests-menu" => Kind::Issues {
            pull: true,
            mine: true,
        },
        "my-packages-menu" => Kind::Packages,
        _ => return Err("Unknown GitHub menu.".into()),
    };
    let mut list = List::new(kind);
    list.load(api, &menu.prefs, &menu.login, false).await?;
    // Menus intentionally bound refresh work and item counts. Browse the
    // corresponding command for the remaining pages.
    if list.has_more() && list.rows.len() < menu.count() {
        list.load(api, &menu.prefs, &menu.login, true).await?;
    }
    menu.rows = list.rows;
    Ok(())
}
fn view_command(key: &str) -> &str {
    match key {
        "unread-notifications" => "notifications",
        "my-issues-menu" => "my-issues",
        "my-pull-requests-menu" => "my-pull-requests",
        "my-packages-menu" => "my-packages",
        _ => "my-latest-repositories",
    }
}
fn name(key: &str) -> &str {
    match key {
        "unread-notifications" => "GitHub Notifications",
        "my-issues-menu" => "GitHub Issues",
        "my-pull-requests-menu" => "GitHub Pull Requests",
        "my-packages-menu" => "GitHub Packages",
        _ => "GitHub Stats",
    }
}
async fn publish(ctx: &ExtensionContext, key: &str, menu: &mut Menu) -> Result<(), String> {
    let revision = menu.revision.wrapping_add(1);
    let mut callbacks = BTreeMap::new();
    let mut action = |operation: MenuAction, title: &str| {
        let id = menu.callback_id(revision, callbacks.len());
        callbacks.insert(id.clone(), operation);
        json!({"id":id,"title":title})
    };
    let mut items = vec![];
    let connected = menu.api.is_some();
    if connected
        && key == "unread-notifications"
        && menu.error.is_empty()
        && menu.prefs["alwaysShow"] == false
        && menu.rows.is_empty()
    {
        ctx.host("host.tray.remove", json!({"resourceId":key}))
            .await?;
        menu.callbacks.clear();
        menu.revision = revision;
        return Ok(());
    }
    let mut title = name(key).to_owned();
    let launch = json!({"id":"launch-view","title": match key {
        "unread-notifications" => "Open Notifications",
        "my-issues-menu" => "Open My Issues",
        "my-pull-requests-menu" => "Open My Pull Requests",
        "my-packages-menu" => "Open My Packages",
        _ => "Open My Latest Repositories",
    },"commandName":view_command(key)});
    if !connected {
        items.push(launch);
    } else {
        if key == "my-stats-menu" {
            let metric = menu.prefs["titleMetric"].as_str().unwrap_or("followers");
            if let Some(value) = menu.stats[metric].as_u64() {
                title = format!("GitHub {value}");
            }
            for (key, label) in [
                ("followers", "Followers"),
                ("following", "Following"),
                ("stars", "Stars Received"),
                ("prsOpen", "Open Pull Requests"),
                ("issuesOpen", "Open Issues"),
                ("repositories", "Owned Repositories"),
                ("rateLimit", "Remaining API Calls"),
            ] {
                if let Some(value) = menu.stats[key].as_u64() {
                    items.push(json!({"id":format!("stats-{key}"),"kind":"section","title":format!("{label} ? {value}")}));
                }
            }
        } else {
            let count = menu.rows.len();
            if key == "unread-notifications" {
                if menu.prefs["showUnreadCount"] != false {
                    title = format!("GitHub {count}");
                }
            } else if menu.prefs["showtext"] != false {
                if key == "my-packages-menu" && !text(&menu.prefs, "titlePackage").is_empty() {
                    title = menu
                        .rows
                        .iter()
                        .find(|row| {
                            row.title
                                .eq_ignore_ascii_case(text(&menu.prefs, "titlePackage"))
                        })
                        .map(|row| row.title.clone())
                        .unwrap_or(title);
                } else {
                    title = format!("{title} {count}");
                }
                if count > 0 && menu.prefs["useUnreadIndicator"] == true {
                    title = format!("? {title}");
                }
            } else {
                title = "GitHub".into();
            }
            for (index, row) in menu.rows.iter().take(menu.count()).enumerate() {
                let label = row.title.chars().take(80).collect::<String>();
                if key == "unread-notifications" {
                    let mut thread_actions = vec![action(
                        MenuAction::Open(row.url.clone()),
                        "Open Notification",
                    )];
                    if row
                        .id
                        .parse::<u64>()
                        .is_ok_and(|id| id.to_string() == row.id)
                    {
                        thread_actions
                            .push(action(MenuAction::Read(row.id.clone()), "Mark as Read"));
                    }
                    items.push(json!({"id":format!("thread-{index}"),"kind":"submenu","title":label,"items":thread_actions}));
                } else if !row.url.is_empty() {
                    items.push(action(MenuAction::Open(row.url.clone()), &label));
                }
            }
        }
        if !menu.error.is_empty() {
            items.push(json!({"id":"refresh-error","kind":"section","title":"GitHub could not refresh this menu"}));
        }
        items.push(launch);
        items.push(action(MenuAction::Refresh, "Refresh"));
    }
    items.extend([
        action(MenuAction::Configure, "Open Extension Preferences"),
        action(MenuAction::Disable, "Stop GitHub Menu"),
    ]);
    ctx.host("host.tray.set",json!({"resourceId":key,"title":title.chars().take(75).collect::<String>(),"icon":ICON,
        "tooltip":if connected{name(key)}else{"Run this GitHub menu command to reconnect"},"items":items})).await?;
    menu.callbacks = callbacks;
    menu.revision = revision;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn callbacks_are_owned_by_a_generation_and_revision() {
        let first = Menu::new();
        let key = first.callback_id(3, 0);
        assert_ne!(first.callback_id(4, 0), key);
        assert_ne!(first.callback_id(3, 1), key);
        assert_ne!(Menu::new().callback_id(3, 0), key);
        let longest = first.callback_id(u64::MAX, usize::MAX);
        assert!(
            longest.len() <= 200
                && longest
                    .bytes()
                    .all(|c| c.is_ascii_alphanumeric() || b"-_.".contains(&c))
        );
    }
}
