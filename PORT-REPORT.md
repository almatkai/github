# GitHub Rust port

The Store inventory ranks GitHub 14th with 222,064 downloads at capture. The extension was authored by **thomaslombart** and is owned by **raycast**. Its Store identity is `89648e03-cceb-4205-9f40-75fcb039a4c6`. The source tree is frozen at `dccfc8877dd0e658295cb4b6d29b4a0a1b79baf8`. [Store listing](https://www.raycast.com/raycast/github), [immutable source](https://github.com/raycast/extensions/tree/dccfc8877dd0e658295cb4b6d29b4a0a1b79baf8/extensions/github)

The port is version 0.1.0 of `com.tezbar.native.github`, replacing `raycast.github`. It uses schema 3, Rust process execution, protocol 1, and Tezbar **0.3.0-beta.12** or later. The immutable SDK commit is `1a11afc14693cc6463e0b95ae7d61991b4a60427`. The source freeze contains 202 byte-verified original Git blobs, the complete upstream tree inventory including modes, original assets, package metadata and the original licenses. The manifest preserves the original 25 command IDs, modes, preferences and both source platforms. The port's repository field is the pinned upstream directory. [SDK commit](https://github.com/almatkai/tezbar-extension-sdk-rust/tree/1a11afc14693cc6463e0b95ae7d61991b4a60427)

## Ordinary commands

| Source command | Native workflow |
|---|---|
| `my-pull-requests` | Viewer-authored, assigned, mentioned, review-requested, reviewed and recently closed query groups. Groups honor the original visibility, draft, team-review and repository preferences. |
| `search-pull-requests` | Paginated qualified PR search, updated details, comments, form-based title edit, close/reopen, and confirmed merge pinned to the head SHA displayed in the dialog. |
| `create-pull-request` | Repository picker or explicit owner/name, head and base refs, draft and maintainer-edit options, reviewers, assignees, labels, milestone and optional auto-merge. A created PR is linked even if a follow-up metadata write fails. |
| `my-issues` | Grouped viewer-owned, assigned, mentioned and recently closed issue queries with corresponding repository and visibility preferences. |
| `search-issues` | Paginated qualified issue search, live issue and comment preview, comment, title edit and confirm close/reopen. |
| `create-issue` | Repository selection, editable title and Markdown description, assignees, labels, milestone and optional browser open after a confirmed create. |
| `create-branch` | Create a Git ref from a selected source branch commit, or link a GitHub issue through `createLinkedBranch`. The user can copy or open the resulting URL. |
| `create-release` | Browse releases, select target and tag, generate notes into the draft, and confirm draft/prerelease or release creation. |
| `create-repository` | List the authenticated owner and available organizations, account-scoped repository creation, privacy, description and optional README. |
| `search-repositories` | Remote pagination, fork/archive and owner-label preferences, open, GitHub web editor, clone URL copying and star/unstar. |
| `my-latest-repositories` | Recently pushed accessible repositories and repository actions. |
| `my-starred-repositories` | Paginated viewer stars with immediate local removal after a successful unstar. |
| `my-packages` | Paginated container, npm, Maven, Docker, NuGet and RubyGems registry queries, visibility and hosted version/tag browsing. |
| `workflow-runs` | Repository picker and run pagination, job previews and explicitly confirmed rerun or cancel. |
| `dispatch-workflow` | Browse repository workflows, retrieve the chosen ref's YAML, validate required inputs and choices, and explicitly confirm dispatch. Only the loopback implementation was exercised. |
| `notifications` | Unread/all views, target URL mapping, thread read, done and mute, and a confirmed account-wide mark-read action. |
| `unread-notifications` | Durable tray registration, unread count, visibility preferences, open/read callbacks and an interactive full-list shortcut. |
| `search-discussions` | GraphQL discussion search with opaque cursors, discussion preview and deep link. |
| `my-discussions` | Viewer-authored GraphQL discussion search, opaque cursors and discussion actions. |
| `my-projects` | Viewer Projects V2 GraphQL connection, open/closed status, description and browser links. |
| `my-issues-menu` | Durable issue tray with configured source groups, bounded item count, title indicator and a GitHub Issues command shortcut. |
| `my-stats-menu` | Durable stats tray, viewer metrics and repository star totals, selectable title metric and polling interval, positive star notifications and access to the main views. |
| `my-pull-requests-menu` | Durable PR tray with configured source groups, bounded item count, title indicator and a GitHub PR command shortcut. |
| `my-packages-menu` | Durable package tray with registry queries, bounded item count and configured package/count tile. |
| `download-repository` | Clipboard URL prefill, repository/ref validation, authorized zipball request, credential-free codeload redirect and atomic ZIP save without overwriting an existing file. |

## API and menu behavior

REST pins GitHub API version `2022-11-28`, supported through March 10, 2028. The binary sends its sensitive bearer header to the fixed `https://api.github.com` origin. It constructs REST page requests from trusted route segments even when a next link supplies another origin, and uses returned GraphQL cursors for discussions and projects. Non-idempotent calls have automatic retries disabled. An uncertain response keeps the draft and directs the user to check GitHub. Download redirects are restricted to GitHub codeload, without forwarding the bearer. [REST versions](https://docs.github.com/en/rest/about-the-rest-api/api-versions), [REST pagination](https://docs.github.com/en/rest/using-the-rest-api/using-pagination-in-the-rest-api), [Workflows](https://docs.github.com/en/rest/actions/workflows), [Repository ZIP](https://docs.github.com/en/rest/repos/contents)

Each menu retains a durable identity before acknowledging a new registration. Menus use generation- and revision-bound callbacks. Each menu has one reusable polling worker that sleeps without a timer while the menu is disabled. A stop action releases the durable owner and removes the corresponding tray item. Durable records hold only the menu identity and format version. After process restart, the SDK host supplies no extension preferences, so the restored menu offers a list shortcut without contacting GitHub. Running the menu command again provides the current token in memory and resumes polling.

## Preview limits

- GitHub.com is the supported origin. Managed OAuth, non-GitHub.com Enterprise Server origins, credential migration and automatic authenticated refresh immediately after process restart are outside this preview. A user-provided Personal Access Token is required for authenticated routes. The host currently lacks scoped preference or secret rehydration during background activation.
- All 20 upstream AI tools remain unregistered. Their individual IDs are listed in the `tools` inventory in [UPSTREAM.json](UPSTREAM.json), and their original source and the AI skill/read-resource source are preserved. The current ordinary-command host has no inbound equivalent for Raycast AI tools, mentions or resources.
- Native reviewer and assignee selection uses usernames, with labels and milestone entered directly. Project and issue-type assignment, custom templates, team review creation, advanced PR action menus and inline diff review are absent. Viewer PR and issue searches implement the primary groups and repository filtering, with bounded results.
- The repository browser copies an HTTPS or SSH clone URL and opens GitHub or github.dev. Local cloning, repository history/pins, and launching particular VS Code or other Git clients are absent. Release asset upload, workflow pins, workflow status/branch filters, workflow logs and retries of individual jobs are outside this preview.
- Packages list the authenticated account's six package types and versions. Package administration, organization package aggregation, and registry download-count metrics are outside this preview. The stats menu uses the bundled icon and summarizes up to 100 owned repositories for the star count. It does not recreate the original detailed stats and star-history views.
- Notifications use core reason/title/read fields and common subject links. The menu reads bounded pages each time. Native desktop notification display, context menus, UI acceptance and live GitHub permissions remain to be validated separately. The fixture suite made no real repository, notification, release or workflow changes.
- ZIP download accepts a user-entered existing directory and 128 MiB maximum response. The archive is saved without extraction. Path variants beyond repository root/branch URL or an explicit ref are outside this preview.

## Local evidence

`python scripts/verify-local.py` runs source, format, unit, JSONL fixture, package-refusal, optimized package and production ZIP startup gates. `reports/protocol-smoke.json` records the fixture groups. Source freeze and per-file/platform inventories are produced under `dist/`. Windows and macOS have separate native artifacts.
