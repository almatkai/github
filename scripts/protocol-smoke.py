"""Exercise all 25 ordinary GitHub commands through the actual native executable."""
import http.server
import json
import os
from pathlib import Path
import subprocess
import sys
import tempfile
import threading

from fixture_peer import BASE, WEB, ZIP, LocalGitHub, Peer, field, rows

ROOT = Path(__file__).resolve().parents[1]
BINARY = Path(sys.argv[1]).resolve()
assert json.loads(subprocess.check_output([str(BINARY), "--build-info"])) == {"fixture": True}


def hits(path, method="POST"):
    return [args for verb, route, args in LocalGitHub.calls if verb == method and route == path]


def has(view, ident):
    return any(row["props"].get("id")==str(ident) for row in rows(view))


class Checks:
    def __init__(self, peer):
        self.peer=peer
        self.covered=set()
        self.checks=[]

    def launch(self, command, session=None, prefs=None):
        self.covered.add(command)
        return self.peer.open(session or command,command,prefs)

    def choose(self, command, session=None):
        view=self.launch(command,session)
        ids = next(r["props"]["actionIds"] for r in rows(view) if r["props"]["id"] == "100")
        assert any(a["id"] in ids and a["title"] == "Enter Repository Name" for a in view["actions"])
        return self.peer.action(session or command,view,"Choose Repository",row=100)

    def read(self):
        p=self.peer
        view=self.launch("search-issues","setup",{})
        assert "Personal Access Token" in json.dumps(view) and not LocalGitHub.calls
        p.action("setup",view,"Open Extension Preferences")
        view=self.launch("search-issues")
        view=p.call("search-issues","search-text-changed",{"searchText":"cursor fixture"})
        assert view["root"]["props"]["pagination"]["hasMore"]
        stale=view["actions"][0]["id"]
        view=p.select("search-issues",view,501)
        assert not view["root"]["props"]["pagination"]["hasMore"] and any(row["props"]["title"]=="Second page fixture" for row in rows(view))
        assert p.call("search-issues","invoke-action",{"actionId":stale})["mode"]=="unchanged"
        view=p.action("search-issues",view,"View Details",row=501)
        assert "Fixture reply" in view["root"]["props"]["markdown"]
        p.confirm=True
        view=p.action("search-issues",view,"Add Comment")
        p.action("search-issues",view,"Add Comment",{"body":"Synthetic comment"})
        assert hits(BASE+"/issues/11/comments")[-1]=={"body":"Synthetic comment"}
        self.checks.append("No token causes zero HTTP, REST page link is fenced, old callbacks expire, issue preview and comment work")

        prefs={"personalAccessToken":"gh_fixture_only","showAssigned":True,"showRecentlyClosed":True,
               "repositoryFilterMode":"include","repositoryList":"fixture/project"}
        assert has(self.launch("my-issues",prefs=prefs),501)
        queries=[args["q"] for args in hits("/search/issues","GET")]
        assert any("assignee:fixture" in value and "repo:fixture/project" in value for value in queries)
        assert any("is:closed" in value and "updated:" in value for value in queries)
        assert has(self.launch("my-pull-requests"),502)
        self.launch("search-pull-requests")
        view=p.call("search-pull-requests","search-text-changed",{"searchText":"synthetic"})
        p.action("search-pull-requests",view,"Merge Pull Request",row=502)
        assert hits(BASE+"/pulls/12/merge","PUT")[-1]=={"merge_method":"merge","sha":"b"*40}
        self.checks.append("Viewer issue/PR groups and repository filters, remote PR search and confirmed merge")

    def create(self):
        p=self.peer
        v=self.choose("create-issue")
        too_many = ",".join(f"item-{n}" for n in range(26))
        for key in ("labels", "assignees"):
            baseline = len(p.host_calls)
            v=p.action("create-issue",v,"Create Issue",{"title":"Limit check", key:too_many})
            assert f"at most 25 {key}" in json.dumps(v) and len(p.host_calls)==baseline
            # Clear the previous invalid draft via the next submission.
            if key == "labels":
                v=p.action("create-issue",v,"Create Issue",{"labels":"", "title":""})
        p.confirm=False
        v=p.action("create-issue",v,"Create Issue",{"title":"Cancelled","assignees":""})
        assert not hits(BASE+"/issues")
        p.confirm=True
        v=p.action("create-issue",v,"Create Issue",{"title":"FAIL WRITE","body":"Recover this draft"})
        assert hits(BASE+"/issues")==[{"title":"FAIL WRITE","body":"Recover this draft"}]
        assert field(v,"body")["value"]=="Recover this draft" and "diagnostics" not in json.dumps(v)
        v=p.action("create-issue",v,"Create Issue",{"title":"New Issue","assignees":"fixture","labels":"documentation","milestone":"3"})
        assert hits(BASE+"/issues")[-1]=={"title":"New Issue","body":"Recover this draft","assignees":["fixture"],"labels":["documentation"],"milestone":3}
        assert v["root"]["type"]=="Detail"
        self.checks.append("Cancelled writes send nothing; uncertain HTTP failure retains drafts; typed issue creation never auto-replays")

        v=self.choose("create-pull-request")
        assert field(v,"base")["value"]=="main"
        baseline=len(p.host_calls)
        v=p.action("create-pull-request",v,"Create Pull Request",{"title":"Limit","head":"feature","reviewers":too_many})
        assert "at most 25 reviewers" in json.dumps(v) and len(p.host_calls)==baseline
        p.action("create-pull-request",v,"Create Pull Request",{"title":"Synthetic PR","head":"fixture:feature/work",
                 "draft":True,"reviewers":"fixture-reviewer","autoMerge":"SQUASH","labels":"review"})
        assert hits(BASE+"/pulls")[-1]["head"]=="fixture:feature/work"
        assert hits(BASE+"/pulls/12/requested_reviewers")==[{"reviewers":["fixture-reviewer"]}]
        assert hits(BASE+"/issues/12","PATCH")[-1]["labels"]==["review"]
        assert any("EnableAutoMerge" in item["query"] for item in hits("/graphql"))
        self.checks.append("PR creation sends base/head/draft, issue metadata, review request and auto-merge")

        for session,values in [("branch",{"name":"fixture-branch"}),("linked",{"issue":"11"})]:
            v=self.choose("create-branch",session)
            v=p.action(session,v,"Create Branch",values)
            assert "Branch created" in json.dumps(v)
        assert hits(BASE+"/git/refs")==[{"ref":"refs/heads/fixture-branch","sha":"a"*40}]
        assert any("CreateLinkedBranch" in item["query"] for item in hits("/graphql"))
        v=self.launch("create-repository")
        assert field(v,"owner")["value"]=="fixture"
        p.action("create-repository",v,"Create Repository",{"owner":"fixture-org","name":"new-fixture","autoInit":True})
        assert hits("/orgs/fixture-org/repos")[-1]=={"name":"new-fixture","description":"","private":True,"auto_init":True}
        self.checks.append("Git ref branch write, issue-linked branch GraphQL mutation, organization repository creation")

        v=self.choose("create-release")
        assert has(v,15)
        v=p.action("create-release",v,"Create Release")
        p.edit("create-release",v,"tag","v1.0.0")
        p.edit("create-release",v,"draft",True)
        v=p.action("create-release",v,"Generate Release Notes")
        assert hits(BASE+"/releases/generate-notes")[-1]["tag_name"]=="v1.0.0"
        assert field(v,"body")["value"]=="Generated fixture notes"
        p.action("create-release",v,"Create Release")
        assert hits(BASE+"/releases")[-1]["draft"] is True
        self.checks.append("Release list, generated notes, retained release draft and confirmed draft creation")

    def workflows(self):
        p=self.peer
        p.confirm=True
        v=self.choose("workflow-runs")
        d=p.action("workflow-runs",v,"View Details",row=300)
        assert "Fixture Job" in json.dumps(d)
        v=p.action("workflow-runs",d,"Back")
        v=p.action("workflow-runs",v,"Re-run Workflow",row=300)
        p.action("workflow-runs",v,"Cancel Workflow Run",row=301)
        assert hits(BASE+"/actions/runs/300/rerun") and hits(BASE+"/actions/runs/301/cancel")
        v=self.choose("dispatch-workflow")
        v=p.action("dispatch-workflow",v,"Configure Dispatch",row=13)
        assert field(v,"workflow:mode")["title"]=="mode (required)"
        p.edit("dispatch-workflow",v,"ref","release")
        assert not hits(BASE+"/actions/workflows/13/dispatches")
        v=p.action("dispatch-workflow",v,"Load Inputs for Ref")
        assert hits(BASE+"/contents/.github/workflows/fixture.yml","GET")[-1]["ref"]=="release"
        # A later ref edit must still prevent dispatch against the previous schema.
        v=p.action("dispatch-workflow",v,"Dispatch Workflow",{"ref":"main","workflow:mode":"verify"})
        assert not hits(BASE+"/actions/workflows/13/dispatches")
        p.edit("dispatch-workflow",v,"ref","release")
        p.edit("dispatch-workflow",v,"workflow:mode","verify")
        p.confirm=False
        v=p.action("dispatch-workflow",v,"Dispatch Workflow")
        assert not hits(BASE+"/actions/workflows/13/dispatches")
        p.confirm=True
        p.action("dispatch-workflow",v,"Dispatch Workflow")
        assert hits(BASE+"/actions/workflows/13/dispatches")==[{"ref":"release","inputs":{"mode":"verify","dry":"true"}}]
        self.checks.append("Workflow listing/jobs, fake rerun/cancel, ref-specific YAML fields, overwrite-safe draft and confirmed loopback dispatch")

    def libraries(self):
        p=self.peer
        self.launch("search-repositories")
        v=p.call("search-repositories","search-text-changed",{"searchText":"fixture"})
        assert has(v,100)
        p.action("search-repositories",v,"Star Repository",row=100)
        assert hits("/user/starred/fixture/project","PUT")
        for command in ("my-latest-repositories","my-starred-repositories"):
            assert has(self.launch(command),100)
        v=self.launch("my-packages")
        assert has(v,201)
        v=p.action("my-packages",v,"Browse Versions",row=201)
        assert any("latest" in row["props"]["subtitle"] for row in rows(v))
        assert len({args["package_type"] for args in hits("/user/packages","GET")})==6
        self.checks.append("Repository search/latest/starred, star write, six registry queries and package versions")

        LocalGitHub.filtered_page=True
        v=self.launch("notifications","filtered",{"personalAccessToken":"gh_fixture_only","repositoryFilterMode":"include","repositoryList":"fixture/project"})
        assert v["root"]["props"]["pagination"]["hasMore"] and not has(v,42)
        load=next(a for a in v["actions"] if a["title"]=="Load More")
        assert load["id"] in v["root"]["props"]["actionIds"]
        assert has(p.action("filtered",v,"Load More"),42)
        LocalGitHub.filtered_page=False
        v=self.launch("notifications","batch")
        ids=next(r["props"]["actionIds"] for r in rows(v) if r["props"]["id"]=="42")
        assert {"Show All Notifications","Mark All as Read","Refresh"} <= {a["title"] for a in v["actions"] if a["id"] in ids}
        p.confirm=True
        LocalGitHub.mark_status=202
        reads_before=len(hits("/notifications","GET"))
        v=p.action("batch",v,"Mark All as Read",row=42)
        assert len(hits("/notifications","GET"))==reads_before+1
        assert has(v,42) and "queued" in json.dumps(v)
        LocalGitHub.mark_status=205
        v=p.action("batch",v,"Mark All as Read",row=42)
        assert not has(v,42) and "marked as read" in json.dumps(v)
        v=self.launch("notifications")
        v=p.action("notifications",v,"Mark as Read",row=42)
        assert not has(v,42)
        v=p.action("notifications",v,"Show All Notifications")
        p.action("notifications",v,"Mute Thread",row=42)
        assert hits("/notifications/threads/42","PATCH")
        assert hits("/notifications/threads/42/subscription","PUT")[-1]["ignored"] is True
        self.launch("search-discussions")
        v=p.call("search-discussions","search-text-changed",{"searchText":"synthetic"})
        assert v["root"]["props"]["pagination"]["hasMore"]
        v=p.select("search-discussions",v,"DISC1")
        assert has(v,"DISC2")
        assert has(self.launch("my-discussions"),"DISC1")
        assert any("author:fixture" in item["variables"].get("q","") for item in hits("/graphql"))
        assert has(self.launch("my-projects"),"PVT_fixture")
        self.checks.append("Notification list/read/mute, Discussion GraphQL cursors and Projects V2")

        v=self.launch("download-repository")
        assert field(v,"repository")["value"]==WEB
        v=p.action("download-repository",v,"Download ZIP",{"ref":"main","directory":str(p.work/"temp")})
        assert (p.work/"temp"/"project-main.zip").read_bytes()==ZIP.getvalue()
        p.action("download-repository",v,"Reveal ZIP")
        assert p.host_calls[-1][0]=="host.shell.reveal"
        assert hits(BASE+"/zipball/main","GET") and hits("/fixture-archive","GET")
        self.checks.append("ZIP metadata redirect is stripped of PAT, saved bytes and host reveal are verified")

    def menus(self):
        p=self.peer
        menu_keys=("unread-notifications","my-issues-menu","my-stats-menu","my-pull-requests-menu","my-packages-menu")
        for key in menu_keys:
            view=self.launch(key)
            assert view["mode"]=="no-view" and "enabled" in json.dumps(view)
            assert p.trays(key)
        retains=[args for method,args in p.host_calls if method=="host.background.retain"]
        assert {value["key"] for value in retains}==set(menu_keys)
        assert all(value["durable"] and value["data"]=={"format":1,"menu":value["key"]} for value in retains)
        stale=next(item["id"] for item in p.trays("unread-notifications")[-1]["items"] if item["title"]=="Refresh")
        p.request("background.event",{"resourceId":"unread-notifications","actionId":stale})
        p.request("background.event",{"resourceId":"unread-notifications","actionId":stale},error=True)
        read=next(item["items"][1]["id"] for item in p.trays("unread-notifications")[-1]["items"] if item.get("kind")=="submenu")
        p.reject_tray_sets=1
        p.request("background.event",{"resourceId":"unread-notifications","actionId":read},error=True)
        p.request("background.event",{"resourceId":"unread-notifications","actionId":read})
        patches=[route for method,route,_ in LocalGitHub.calls if method=="PATCH" and route.startswith("/notifications/threads/")]
        assert patches[-2:]==["/notifications/threads/42"]*2
        stats=next(item["id"] for item in p.trays("my-stats-menu")[-1]["items"] if item["title"]=="Refresh")
        LocalGitHub.stars=11
        p.request("background.event",{"resourceId":"my-stats-menu","actionId":stats})
        assert any(method=="host.notifications.show" for method,_ in p.host_calls)
        stop=next(item["id"] for item in p.trays("my-packages-menu")[-1]["items"] if item["title"]=="Stop GitHub Menu")
        p.request("background.event",{"resourceId":"my-packages-menu","actionId":stop})
        assert p.host_calls[-1]==("host.background.release",{"key":"my-packages-menu"})
        self.checks.append("Five durable tray registrations, revision fence, thread read, new-stars alert and durable release")
        key="my-issues-menu"
        for cycle in range(66):
            stop=next(item["id"] for item in p.trays(key)[-1]["items"] if item["title"]=="Stop GitHub Menu")
            p.request("background.event",{"resourceId":key,"actionId":stop})
            session=f"menu-toggle-{cycle}"
            view=p.open(session,key)
            assert view["mode"]=="no-view" and "enabled" in json.dumps(view),view
            p.request("session.dispose",{"sessionId":session})
        self.checks.append("66 stop/re-enable cycles reuse one worker and avoid the 64-task host scope limit")
        p.open("re-enable-packages","my-packages-menu")
        # Same login, different credential, rejected disconnected and replacement publications.
        previous=next(i["id"] for i in p.trays("unread-notifications")[-1]["items"] if i["title"]=="Refresh")
        previous_stats=next(i["id"] for i in p.trays("my-stats-menu")[-1]["items"] if i["title"]=="Refresh")
        start=len(p.host_calls)
        p.reject_tray_sets=6
        rotated={"personalAccessToken":"gh_fixture_only_rotated"}
        p.open("rotated","unread-notifications",rotated)
        assert p.reject_tray_sets==0
        for key,ident in [("unread-notifications",previous),("my-stats-menu",previous_stats)]:
            before=len(LocalGitHub.calls)
            p.request("background.event",{"resourceId":key,"actionId":ident},error=True)
            assert len(LocalGitHub.calls)==before
        p.open("rotated-stats","my-stats-menu",rotated)
        attempted=[params for method,params in p.host_calls[start:] if method=="host.tray.set"]
        assert any(i["id"]=="refresh-error" for i in p.trays("my-stats-menu")[-1]["items"])
        assert any(i["id"]=="refresh-error" for i in p.trays("unread-notifications")[-1]["items"])
        assert attempted and "Fixture thread" not in json.dumps(attempted) and "stats-stars" not in json.dumps(attempted)
        return retains,stale

    def limits(self):
        p=self.peer
        self.launch("search-issues","rate")
        v=p.call("rate","search-text-changed",{"searchText":"force rate limit"})
        assert "7 seconds" in json.dumps(v) and "private server diagnostics" not in json.dumps(v)
        self.checks.append("Rate-limit delay is displayed without leaking HTTP response bodies")


def main():
    invalid=subprocess.run([str(BINARY)],input=b"",capture_output=True,timeout=10,
                           env=dict(os.environ,TEZBAR_GITHUB_FIXTURE_ORIGIN="https://outside.invalid/"))
    assert invalid.returncode!=0 and not invalid.stdout and b"literal loopback" in invalid.stderr
    service=http.server.ThreadingHTTPServer(("127.0.0.1",0),LocalGitHub)
    threading.Thread(target=service.serve_forever,daemon=True).start()
    try:
        with tempfile.TemporaryDirectory(prefix="tezbar-github-fixture-") as path:
            endpoint=f"http://127.0.0.1:{service.server_port}"
            peer=Peer(BINARY,ROOT,Path(path)/"first",endpoint)
            checks=Checks(peer)
            try:
                checks.read()
                checks.create()
                checks.workflows()
                checks.libraries()
                retained,stale=checks.menus()
                checks.limits()
            finally:
                peer.close()
            restarted=Peer(BINARY,ROOT,Path(path)/"restart",endpoint)
            try:
                before=len(LocalGitHub.calls)
                for record in retained:
                    restarted.request("background.activate",{"key":record["key"],"data":record["data"]})
                    shortcut=restarted.trays(record["key"])[-1]["items"][0]
                    target={"unread-notifications":"notifications","my-stats-menu":"my-latest-repositories"}.get(
                        record["key"], record["key"].removesuffix("-menu"))
                    assert shortcut["commandName"]==target and target in restarted.view_commands
                    assert shortcut["title"].startswith("Open ")
                assert len(LocalGitHub.calls)==before
                restarted.request("background.event",{"resourceId":"unread-notifications","actionId":stale},error=True)
                reopened=restarted.open("return","unread-notifications")
                assert reopened["mode"]=="no-view"
                assert any(i.get("kind")=="submenu" for i in restarted.trays("unread-notifications")[-1]["items"])
            finally:
                restarted.close()
    finally:
        service.shutdown()
        service.server_close()
    checks.checks.append("Fixture rejects a non-loopback origin and restores menus without PAT/API traffic until reopened")
    commands=json.loads(subprocess.check_output([str(BINARY),"--protocol-info"]))["commands"]
    assert set(commands)==checks.covered and len(checks.covered)==25,set(commands)-checks.covered
    report={"passed":len(checks.checks),"checks":checks.checks,"ordinaryCommands":25,"liveAccount":"not-used","desktopAcceptance":"not-run"}
    out=ROOT/"reports"
    out.mkdir(exist_ok=True)
    (out/"protocol-smoke.json").write_text(json.dumps(report,indent=2)+"\n",encoding="utf8")
    print(json.dumps(report,indent=2))


if __name__=="__main__":
    main()
