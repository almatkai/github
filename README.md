# GitHub for Tezbar

Native Rust extension for Tezbar 0.3.0-beta.12 and later. It runs the 25 ordinary GitHub Store commands through the GPUI process host. [Port report](PORT-REPORT.md)

Enter a GitHub **Personal Access Token** in extension preferences. Give it access only to the repositories and operations you use. Read access covers browsing. The write commands require the matching GitHub repository permissions. GitHub Packages metadata requires a classic token with the appropriate package scope. [GitHub personal access tokens](https://docs.github.com/en/authentication/keeping-your-account-and-data-secure/managing-your-personal-access-tokens), [GitHub Packages REST API](https://docs.github.com/en/rest/packages/packages)

Search, browse or choose a repository, then use its native actions to open items, manage notifications or edit a draft. The create, merge and workflow forms ask for confirmation before the requested write. Choose an existing directory to save a repository ZIP.

Run any of the five menu commands to install its menu-bar item. After restart, the menu shortcut opens its corresponding GitHub list. Run the menu command again to reload preferences and resume polling. **Stop GitHub Menu** releases it.

## Local validation

Run `python scripts/verify-local.py` with Rust and Python installed. The fixture build requires a literal loopback server and a synthetic token. It performs no GitHub account or CI operations.
