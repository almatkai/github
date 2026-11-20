"""Loopback-only API and JSONL host for GitHub tests."""
import http.server
import io
import json
import os
import queue
import re
import subprocess
import sys
import threading
from urllib.parse import parse_qs, urlsplit
import uuid
import zipfile

BASE = "/repos/fixture/project"
WEB = "https://github.com/fixture/project"
REPO = {"id": 100, "node_id": "REPO_FIXTURE", "full_name": "fixture/project",
        "html_url": WEB, "default_branch": "main", "description": "Fixture repository"}
ISSUE = {"id": 501, "node_id": "ISSUE_FIXTURE", "number": 11, "title": "Fixture issue",
         "state": "open", "body": "Issue body", "html_url": WEB + "/issues/11",
         "repository_url": "https://api.github.com" + BASE}
PULL = {**ISSUE, "id": 502, "node_id": "PR_FIXTURE", "number": 12, "title": "Fixture PR",
        "html_url": WEB + "/pull/12", "pull_request": {}, "head":{"sha":"b"*40}}
THREAD = {"id": "42", "unread": True, "reason": "review_requested", "repository": REPO,
          "subject": {"title": "Fixture thread", "type": "PullRequest",
                      "url": "https://api.github.com" + BASE + "/pulls/12"}}
WORKFLOW = {"id": 13, "name": "Synthetic workflow", "state": "active",
            "path": ".github/workflows/fixture.yml", "html_url": WEB + "/actions/workflows/fixture.yml"}
RUN = {"id": 300, "name": "Run completed", "status": "completed", "conclusion": "failure",
       "head_branch": "main", "event": "workflow_dispatch", "run_number": 3, "html_url": WEB + "/actions/runs/300"}
RELEASE = {"id": 15, "name": "Existing Release", "tag_name": "v0.1.0", "html_url": WEB + "/releases/tag/v0.1.0"}
PACKAGE = {"id": 201, "name": "fixture-image", "package_type": "container",
           "visibility": "private", "html_url": WEB + "/pkgs/container/fixture-image"}
YAML = b"""name: Fixture
on:
  workflow_dispatch:
    inputs:
      mode:
        type: choice
        required: true
        options: [preview, verify]
      dry:
        type: boolean
        default: true
"""
ZIP = io.BytesIO()
with zipfile.ZipFile(ZIP, "w") as archive:
    archive.writestr("fixture/README.txt", "Synthetic archive")


class LocalGitHub(http.server.BaseHTTPRequestHandler):
    calls = []
    stars = 10
    mark_status = 205
    filtered_page = False

    def log_message(self, *args):
        pass

    def do_GET(self):
        self.dispatch()

    do_POST = do_PUT = do_PATCH = do_DELETE = do_GET

    def reply(self, status, data=None, content_type="application/json", **headers):
        if not isinstance(data, bytes):
            data = b"" if data is None else json.dumps(data).encode()
        self.send_response(status)
        self.send_header("Content-Type", content_type)
        self.send_header("Content-Length", str(len(data)))
        for name, value in headers.items():
            self.send_header(name.replace("_", "-"), value)
        self.end_headers()
        self.wfile.write(data)

    def dispatch(self):
        req = urlsplit(self.path)
        route, method = req.path, self.command
        length = int(self.headers.get("Content-Length", "0"))
        args = json.loads(self.rfile.read(length)) if length else {k:v[-1] for k,v in parse_qs(req.query).items()}
        self.calls.append((method, route, args))
        if route == "/fixture-archive":
            assert self.headers.get("Authorization") is None
            return self.reply(200, ZIP.getvalue(), "application/zip")
        auth = self.headers.get("Authorization")
        assert auth in {"Bearer gh_fixture_only", "Bearer gh_fixture_only_rotated"}
        if auth.endswith("_rotated") and route != "/user":
            return self.reply(403)
        if route == "/notifications":
            if method == "PUT":
                return self.reply(self.mark_status)
            if self.filtered_page and args.get("page") == "1":
                hidden = {**THREAD, "repository": {**REPO, "full_name": "hidden/repo"}}
                return self.reply(200, [hidden], Link='<https://outside.invalid/page2>; rel="next"')
            return self.reply(200, [THREAD, {**THREAD, "id":"43"}], X_Poll_Interval="7200")
        assert self.headers.get("X-GitHub-Api-Version") == "2022-11-28"
        if route == "/graphql":
            body = self.graph(args)
            return self.reply(200, {"data": body})
        if route == "/search/issues":
            query = args["q"]
            if "force rate limit" in query:
                return self.reply(429, {"message": "gh_fixture_only private server diagnostics"}, Retry_After="7")
            row = PULL if "is:pr" in query else ISSUE
            if "is:closed" in query:
                row = {**row, "id": row["id"] + 10, "state": "closed"}
            if args.get("page") == "2":
                return self.reply(200, {"items": [{**row, "id": row["id"] + 20, "title": "Second page fixture"}]})
            headers = {"Link": '<https://outside.invalid/never-follow?page=2>; rel="next"'} if "cursor fixture" in query else {}
            return self.reply(200, {"items": [row]}, **headers)
        if route == BASE + "/contents/.github/workflows/fixture.yml":
            assert self.headers["Accept"] == "application/vnd.github.raw+json"
            assert args["ref"] in ("main", "release")
            return self.reply(200, YAML, "text/plain")
        if route.startswith(BASE + "/zipball"):
            return self.reply(302, Location="/fixture-archive")
        if method == "GET":
            data = {
                "/user": {"login": "fixture"}, "/user/orgs": [{"login": "fixture-org"}],
                "/user/repos": [REPO], "/user/starred": [REPO], "/search/repositories": {"items": [REPO]},
                "/user/packages": [PACKAGE] if args.get("package_type") == "container" else [],
                "/user/packages/container/fixture-image/versions": [{"id":204, "name":"sha256:fixture",
                    "package_html_url": PACKAGE["html_url"], "metadata":{"container":{"tags":["latest"]}}}],
                "/notifications": [THREAD], BASE: REPO, BASE+"/issues/11": ISSUE, BASE+"/pulls/12": PULL,
                BASE+"/issues/11/comments": [{"body": "Fixture reply", "user":{"login":"fixture"}}],
                BASE+"/git/ref/heads/main": {"object":{"sha":"a"*40}},
                BASE+"/releases": [RELEASE], BASE+"/actions/workflows": {"workflows":[WORKFLOW]},
                BASE+"/actions/runs": {"workflow_runs":[RUN,{**RUN,"id":301,"name":"Run active","status":"in_progress","conclusion":None}]},
                BASE+"/actions/runs/300/jobs": {"jobs":[{"name":"Fixture Job","conclusion":"failure"}]},
            }
            assert route in data, (method, route)
            return self.reply(200,data[route])
        if args.get("title") == "FAIL WRITE":
            return self.reply(500, {"message": "gh_fixture_only private server diagnostics"})
        writes = {
            ("POST", "/user/repos"):(201,REPO), ("POST","/orgs/fixture-org/repos"):(201,REPO),
            ("POST", BASE+"/issues"):(201,ISSUE), ("POST",BASE+"/pulls"):(201,PULL),
            ("PATCH",BASE+"/issues/11"):(200,{**ISSUE,**args}), ("PATCH",BASE+"/issues/12"):(200,{**PULL,**args}),
            ("PATCH",BASE+"/pulls/12"):(200,{**PULL,**args}), ("PUT",BASE+"/pulls/12/merge"):(200,{"merged":True}),
            ("POST",BASE+"/pulls/12/requested_reviewers"):(201,None),
            ("POST",BASE+"/issues/11/comments"):(201,{"id":99,"html_url":WEB+"/issues/11#issuecomment-99"}),
            ("POST",BASE+"/git/refs"):(201,{"ref":args.get("ref")}),
            ("POST",BASE+"/releases"):(201,{**RELEASE,**args}),
            ("POST",BASE+"/releases/generate-notes"):(200,{"body":"Generated fixture notes"}),
            ("POST",BASE+"/actions/runs/300/rerun"):(201,None),
            ("POST",BASE+"/actions/runs/301/cancel"):(202,None),
            ("POST",BASE+"/actions/workflows/13/dispatches"):(204,None),
            ("PUT","/user/starred/fixture/project"):(204,None), ("DELETE","/user/starred/fixture/project"):(204,None),
            ("PATCH","/notifications/threads/42"):(205,None), ("PATCH","/notifications/threads/43"):(205,None), ("DELETE","/notifications/threads/42"):(204,None),
            ("PUT","/notifications/threads/42/subscription"):(200,None), ("PUT","/notifications"):(205,None),
        }
        assert (method,route) in writes, (method,route)
        self.reply(*writes[method,route])

    def graph(self,args):
        operation=args["query"].split()[1].split("(")[0].split("{")[0]
        values=args["variables"]
        if operation=="SearchDiscussions":
            after=values.get("after")
            row={"id":"DISC2" if after else "DISC1","number":4,"title":"Fixture discussion",
                 "url":WEB+"/discussions/4","body":"Discussion","repository":{"nameWithOwner":"fixture/project"},
                 "comments":{"totalCount":3}}
            return {"search":{"nodes":[row],"pageInfo":{"hasNextPage":not after,"endCursor":"DISC_CURSOR" if not after else None}}}
        if operation=="MyProjects":
            return {"viewer":{"projectsV2":{"nodes":[{"id":"PVT_fixture","number":1,"title":"Fixture Project",
                    "url":"https://github.com/users/fixture/projects/1","closed":False}],
                    "pageInfo":{"hasNextPage":False,"endCursor":None}}}}
        if operation=="CreateLinkedBranch":
            assert values["input"]["issueId"]=="ISSUE_FIXTURE"
            return {"createLinkedBranch":{"linkedBranch":{"ref":{"name":"linked-branch"}}}}
        if operation=="EnableAutoMerge":
            assert values=={"id":"PR_FIXTURE","method":"SQUASH"}
            return {"enablePullRequestAutoMerge":{"pullRequest":{"id":"PR_FIXTURE"}}}
        if operation=="ViewerStats":
            return {"viewer":{"login":"fixture","followers":{"totalCount":7},"following":{"totalCount":1},
                    "pullRequests":{"totalCount":4},"issues":{"totalCount":2},
                    "owned":{"totalCount":1,"nodes":[{"stargazerCount":type(self).stars}]}},
                    "rateLimit":{"remaining":1000}}
        raise AssertionError(operation)


def rows(view):
    for node in view["root"].get("children", []):
        if node["type"] == "List.Section":
            yield from node["children"]
        elif node["type"] == "List.Item":
            yield node


def field(view, ident):
    return next(node["props"] for node in view["root"]["children"] if node["props"].get("id") == ident)


class Peer:
    def __init__(self, binary, root, work, endpoint):
        work.mkdir()
        self.work=work
        self.host_calls=[]
        self.reject_tray_sets=0
        self.view_commands={c["name"] for c in json.loads((root/"extension.json").read_text(encoding="utf8"))["commands"] if c["mode"]=="view" and c["runtime"]=="rust" and {"Windows","macOS"} <= set(c.get("platforms", ["Windows","macOS"]))}
        self.confirm=False
        self.frames=queue.Queue()
        self.process=subprocess.Popen([str(binary)],stdin=subprocess.PIPE,stdout=subprocess.PIPE,stderr=subprocess.PIPE,
                       env=dict(os.environ,TEZBAR_GITHUB_FIXTURE_ORIGIN=endpoint))

        def pump():
            for line in self.process.stdout:self.frames.put(json.loads(line))

        threading.Thread(target=pump,daemon=True).start()
        hello=self.frames.get(timeout=15)
        assert hello["id"]=="com.tezbar.native.github" and hello["protocol"]==1
        for child in ("data","temp"):(work/child).mkdir()
        caps=json.loads((root/"extension.json").read_text(encoding="utf8"))["capabilities"]
        self.request("initialize",{"protocol":1,"extensionId":hello["id"],"packageDir":str(root),
                     "dataDir":str(work/"data"),"tempDir":str(work/"temp"),"grantedCapabilities":caps,
                     "migrationPaths":{},"reason":"test","platform":"Windows" if sys.platform=="win32" else "macOS"})

    def send(self,frame):
        self.process.stdin.write((json.dumps(frame)+"\n").encode())
        self.process.stdin.flush()

    def request(self,method,parameters,error=False):
        ident="h:"+str(uuid.uuid4())
        self.send({"type":"request","id":ident,"method":method,"params":parameters})
        while True:
            frame=self.frames.get(timeout=25)
            if frame["type"]=="response" and frame["id"]==ident:
                assert "gh_fixture_only" not in json.dumps(frame), "Credential was exposed"
                assert ("error" in frame)==error, frame
                return frame["error" if error else "result"]
            if frame["type"]=="request":
                routing,params=frame["method"],frame["params"]
                assert "gh_fixture_only" not in json.dumps(frame), "Credential sent to host"
                self.host_calls.append((routing,params))
                if routing == "host.tray.set":
                    pending = list(params["items"])
                    seen = set()
                    while pending:
                        item = pending.pop()
                        item_id = item["id"]
                        assert re.fullmatch(r"[a-zA-Z0-9][a-zA-Z0-9_.-]{0,199}", item_id)
                        assert item_id not in seen
                        seen.add(item_id)
                        if "commandName" in item:
                            assert item["commandName"] in self.view_commands
                        pending.extend(item.get("items", []))
                    if self.reject_tray_sets:
                        self.reject_tray_sets -= 1
                        self.send({"type":"response","id":frame["id"],"error":{"code":"host-error","message":"Fixture rejected tray"}})
                        continue
                results={"host.dialog.confirm":{"response":int(self.confirm)},"host.clipboard.read":WEB,
                         "host.background.retain":{"retained":True,"key":params.get("key")},
                         "host.background.release":{"retained":False,"key":params.get("key")}}
                self.send({"type":"response","id":frame["id"],"result":results.get(routing,True)})

    def open(self,session,command,prefs=None):
        return self.request("session.open",{"sessionId":session,"commandName":command,"argumentValues":{},
                            "preferences":{"personalAccessToken":"gh_fixture_only"} if prefs is None else prefs})

    def call(self,session,verb,payload=None):
        return self.request("session.call",{"sessionId":session,"operation":"extension:"+verb,"payload":payload or {}})

    def action(self,session,view,title,values=None,row=None):
        actions=view["actions"]
        if row is not None:
            node=next(node for node in rows(view) if node["props"].get("id")==str(row))
            actions=[value for value in actions if value["id"] in node["props"]["actionIds"]]
        action=next(value for value in actions if value["title"]==title)
        return self.call(session,"invoke-action",{"actionId":action["id"],"formValues":values})

    def select(self, session, view, row_id):
        # beta.12 requests another page when selection is within three rows of the end.
        ids = [row["props"]["id"] for row in rows(view)]
        if ids.index(str(row_id)) + 3 >= len(ids) and view["root"]["props"].get("pagination", {}).get("hasMore"):
            return self.call(session, "load-more")
        return view

    def edit(self, session, view, key, value):
        ident = field(view, key)["actionId"]
        assert self.call(session, "invoke-action", {"actionId":ident, "formValues":{"value":value}})["mode"] == "unchanged"

    def trays(self,key):
        return [params for method,params in self.host_calls if method=="host.tray.set" and params["resourceId"]==key]

    def close(self):
        self.request("shutdown",{})
        self.process.stdin.close()
        assert self.process.wait(timeout=10)==0,self.process.stderr.read().decode()
