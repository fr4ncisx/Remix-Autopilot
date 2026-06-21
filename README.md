# Remix Autopilot

![Actual Version](https://img.shields.io/badge/version-0.1.0--beta-F54927?style=flat-square) ![Rust latest](https://img.shields.io/badge/Rust-latest-orange?style=flat-square&logo=rust) ![Ratatui TUI](https://img.shields.io/badge/TUI-Ratatui-5c7cfa?style=flat-square) ![Provider aware AI](https://img.shields.io/badge/LLM-provider--aware-22c55e?style=flat-square) ![Git automation](https://img.shields.io/badge/Git-automation-CC5666?style=flat-square&logo=git) ![Tests cargo](https://img.shields.io/badge/tests-cargo-16a34a?style=flat-square)

> [!WARNING]
> **Use at your own risk.** This project is currently in **Beta**. There can still be bugs in the AI assistance and in the code design itself. Exhaustive end-to-end (E2E) testing has not been completed yet to guarantee 100% precise operation under all environments, workflows, and Git configurations. Always review the AI-generated commit plans and pull requests before execution.

Remix Autopilot is a Rust TUI for developers who want reviewable Git
automation with a configurable AI provider. You launch it inside a working
directory, the onboarding wizard checks the required Git and provider setup,
and the selected provider helps with commit planning, change explanation,
reviews, and pull request drafting.

This README covers what the project does, what you must install, how to
install it globally as `autopilot`, how to use the main workflows, where
configuration is stored, and how to troubleshoot the common failure cases.

## What Remix Autopilot does

Remix Autopilot is built for one job: turn an active repository into a safer,
clearer local workflow for commits and GitHub operations.

- Analyze one large diff and split it into multiple focused Conventional
  Commits.
- Preview the whole commit plan before writing anything to Git history.
- Stage only the files or safe hunks needed for each generated commit group.
- Explain changes and review diffs with the selected AI provider.
- Draft pull requests through GitHub CLI after you confirm the content.
- Switch branches from inside the TUI after fetching and pruning `origin`.
- Persist user settings globally while always operating on the current
  repository directory.

## Requirements

You need a small toolchain around the CLI. Some parts are mandatory, and some
are only needed for specific workflows.

| Component         | Required             | Why it is needed                                                 |
| ----------------- | -------------------- | ---------------------------------------------------------------- |
| Rust + Cargo      | Yes                  | Build and install the `autopilot` binary.                        |
| Git               | Yes                  | Inspect diffs, stage changes, commit, switch branches, and push. |
| AI provider       | Yes for AI workflows | Generate commit plans, explanations, reviews, and PR drafts.     |
| Ollama            | Optional             | Run local models when you choose the Ollama provider.            |
| Provider API key  | Provider-dependent   | Use OpenAI, Gemini, Anthropic, or another API-backed provider.   |
| GitHub CLI (`gh`) | Optional             | Create GitHub repositories and pull requests.                    |
| Internet access   | Optional             | Needed for API providers, `git push`, `gh`, and remote setup.    |

Remix Autopilot uses your current working directory as the target repository.
It does not ask you to pass a repository path, and it does not copy itself into
each project.

## Install the prerequisites

Install the required tools before you install the CLI itself. If one of these
tools is missing, some commands won't work or the app will block until you fix
the environment.

1. Install Rust and Cargo.
   
   ```bash
   rustup default stable
   ```

2. Install Git and make sure `git` is on your `PATH`.

3. Choose at least one AI provider.

   For local AI, install Ollama from <https://ollama.com/download>, start it,
   and pull a model.
   
   ```bash
   ollama serve
   ollama pull qwen3.5:latest
   ```

   For API-backed AI, keep the provider API key ready. You can enter it in the
   onboarding wizard or later in settings.

4. Optional: install GitHub CLI if you want `/setup` to create GitHub
   repositories or `/pr` to open pull requests.

5. Optional: authenticate GitHub CLI if you installed it.
   
   ```bash
   gh auth login
   ```

## Install Remix Autopilot globally

The intended user flow is a global install. You install the binary once, then
open any repository and run `autopilot`.

1. Clone or open the source tree for this project.

2. Install the binary with Cargo.
   
   ```bash
   cargo install --path .
   ```

3. If you already installed an older local build, reinstall it in place.
   
   ```bash
   cargo install --path . --force
   ```

4. Open a new terminal and verify that the command resolves.
   
   ```bash
   autopilot
   ```

Cargo installs the executable as `autopilot`, even though the Cargo package is
named `remix-autopilot`. To uninstall it later, use:

```bash
cargo uninstall remix-autopilot
```

If `autopilot` is not recognized after installation, your Cargo bin directory
is probably not on `PATH`.

- On Windows, the usual path is `%USERPROFILE%\.cargo\bin`.
- On macOS and Linux, the usual path is `$HOME/.cargo/bin`.

Restart the terminal after updating `PATH`.

## Quick start

The fastest way to validate the setup is to move into the project directory,
launch the TUI, and complete the required onboarding steps.

1. Open a terminal in the repository you want to work on.

2. Run the CLI.
   
   ```bash
   autopilot
   ```

3. Complete the onboarding wizard if it appears.

4. Press `/` to open slash command suggestions.

5. Run `/model` if you want to pick a specific provider model.

6. Run `/commit`, `/review`, or `/diff` to start with a concrete workflow.

When setup is pending, the onboarding wizard starts with language selection so
you can configure the app in a language you understand. It skips steps that
are already configured. It cannot be closed with `Esc` while required setup is
missing, because the app cannot run AI workflows safely without that setup.
Use `Ctrl+C` if you need to quit. If a setup picker or text input is open,
`Esc` returns to the current wizard step instead of bypassing onboarding.

If you are developing Remix Autopilot itself, you can still launch it from the
source tree with `cargo run`. That is a developer workflow, not the main
end-user install path.

## How repository targeting works

Remix Autopilot always operates on the directory where you launched the
process. That means you can use one global installation for many projects
without passing extra arguments.

- If you run `autopilot` inside `C:\work\api`, the app works on that Git
  repository.
- If you run it inside a directory that is not a Git repository, the onboarding
  wizard guides you through the required Git setup, or the app can
  auto-initialize the repo when that setting is enabled.
- Settings remain global per user, but Git actions always apply to the current
  directory.

This split is intentional: configuration is user-scoped, and repository actions
are working-directory-scoped.

## What the UI shows

The app opens in a full-screen alternate terminal view. The main pane shows the
conversation and command results, the input stays fixed near the bottom, and
the footer stays visible while you work.

The status bar is a single-line app state summary. It shows only relevant
runtime information such as repository state, remote status, active branch,
provider health, language, execution mode, and context usage. Keyboard help is
rendered in a separate footer so shortcuts do not wrap into the app status.

When required setup is missing, the app opens the onboarding wizard instead of
the normal settings modal. The wizard starts with language selection, explains
each required step with short **what's missing**, **why it matters**, and
**next action** sections, skips completed steps, and keeps focus until the
missing requirement is fixed or you quit. On narrower terminals, the wizard
switches to a stacked layout so the main action remains visible.

## Keyboard controls

You use the TUI entirely from the keyboard. The controls are small, but they
cover the full workflow.

| Key              | Action                                                                    |
| ---------------- | ------------------------------------------------------------------------- |
| `Enter`          | Send input or confirm the selected modal action.                          |
| `/`              | Open slash command suggestions after onboarding is complete.              |
| `Tab`            | Autocomplete the selected slash suggestion or move between modal actions. |
| `Shift+Tab`      | Switch execution mode between Autopilot and Scout.                        |
| `Up` / `Down`    | Move through suggestions, settings rows, or modal rows with wraparound.   |
| `Left` / `Right` | Change values inside modals and settings.                                 |
| `PgUp` / `PgDn`  | Scroll long commit plan content.                                          |
| `F2`             | Open onboarding if required, otherwise open settings.                     |
| `Esc`            | Clear the prompt, close suggestions, or close non-required modals.        |
| `Ctrl+C`         | Quit the application.                                                     |

Inside the commit plan modal, the content area scrolls independently from the
action buttons. That matters when the generated plan contains many commits.
The required onboarding wizard deliberately ignores `Esc` so you cannot bypass
setup that the app needs to function. In setup child screens, such as provider
pickers or text inputs, `Esc` returns to the wizard step.

## Slash commands

Slash commands are the deterministic entry points for the app. Type `/` in the
input row to open suggestions, use `Up` and `Down` to select a command, and
press `Tab` to autocomplete it.

| Command         | What it does                                                                                         |
| --------------- | ---------------------------------------------------------------------------------------------------- |
| `/commit`       | Analyze the current diff, preview grouped Conventional Commits, and execute them after confirmation. |
| `/switch`       | Fetch `origin --prune`, list remote and local branches, and switch branches from a modal.            |
| `/diff`         | Show a summary of the current diff.                                                                  |
| `/model`        | List or pick the active provider model.                                                              |
| `/lang`         | Change the UI language and the preferred AI response language.                                       |
| `/staged`       | Toggle staged-only mode for diff-based commands.                                                     |
| `/push`         | Push the current branch with Git.                                                                    |
| `/pr`           | Draft and create a pull request through GitHub CLI.                                                  |
| `/pull-request` | Alias for `/pr`.                                                                                     |
| `/explain`      | Explain the current changes in plain language.                                                       |
| `/review`       | Review the current changes for bugs, risks, and missing tests.                                       |
| `/setup`        | Initialize Git, add `origin`, or create a GitHub repository.                                         |
| `/theme`        | Change the color theme.                                                                              |
| `/config`       | Open interactive settings, or onboarding if required.                                                |
| `/reset`        | Reset app configuration, API keys, and `origin` without deleting `.git` or local files.               |
| `/resolve`      | Open the next pending setup, provider, or dependency issue in an actionable modal.                    |
| `/help`         | Show in-app help.                                                                                    |
| `/exit`         | Quit the application.                                                                                |

The AI-backed commands depend on the selected provider being configured and
available. The GitHub-backed commands depend on `gh`. The local Git commands
still work without GitHub CLI.

## Execution modes

The bottom bar shows the current execution mode. You can change it with
`Shift+Tab` while the app is idle.

- **Autopilot** is the direct workflow. It executes AI-assisted actions after
  the normal confirmation steps.
- **Scout** adds an analysis-and-decision loop for AI-backed workflows, so you
  can inspect or redirect what the assistant is proposing before taking the
  next action.

When the selected provider is not configured or unavailable, AI-backed modes
are unavailable until you finish setup or restore that provider.

## Commit workflow

`/commit` is the core feature. It turns one repository diff into a previewable
commit plan, then executes only after you confirm it.

1. Run `/commit`.
2. Let the selected AI provider analyze the diff and return a structured
   commit plan.
3. Review the modal that lists each proposed commit group, affected files, and
   rationale.
4. Confirm the plan to execute the commits in order.
5. Optionally confirm a later push prompt, depending on your settings.

The commit execution path is conservative:

- It previews the plan before writing commits.
- It stages only the files or safe hunks needed for each generated group.
- It does not use `git add -A` as a blanket staging step for the whole repo.
- It stops on the first failing commit and surfaces the error in the TUI.

If the current branch is exactly `main` or `master`, Remix Autopilot opens a
protected-branch warning modal before it talks to the selected provider. That
gives you one more chance to avoid committing directly to a protected branch.

## Branch switching workflow

`/switch` is the in-app branch picker. It is designed for real repositories,
not just a flat branch name list.

When you run `/switch`, the CLI:

1. Executes `git fetch origin --prune`.
2. Loads both remote `origin/*` branches and local branches.
3. Removes `origin/HEAD`.
4. Sorts each section by the newest commit date.
5. Shows `origin` branches first and local branches second.
6. Highlights `main` and `master` as protected branches.

The checkout behavior is also guarded:

- Selecting the current branch is a no-op with visible feedback.
- Selecting a local branch checks out that local branch.
- Selecting a remote-only branch creates and checks out a local tracking
  branch.
- Selecting a remote branch that already has a same-name local branch checks
  out the local branch instead of detaching `HEAD`.

## Pull request workflow

`/pr` and `/pull-request` handle pull request creation through GitHub CLI.
This workflow uses local Git state, but it is not offline because it creates
the pull request through GitHub CLI.

The PR flow does this:

1. Fetch remote branch data.
2. Let you choose a base branch.
3. Build a title and body draft with the selected AI provider.
4. Show the draft for confirmation.
5. Run `gh pr create` only after you confirm.

You must have all of the following for `/pr` to work:

- an existing Git repository,
- an `origin` remote,
- GitHub CLI installed,
- GitHub CLI authenticated with `gh auth login`,
- internet access.

## Repository setup workflow

The onboarding wizard handles required first-run blockers automatically.
`/setup` remains available for repository bootstrapping after the app is
usable.

Depending on the current state, `/setup` can:

- initialize Git in the current directory,
- add an `origin` remote,
- create a GitHub repository through `gh repo create`,
- ask whether the new GitHub repository must be private or public.

If `auto_setup_repo` is enabled, some local workflows such as `/commit` can
initialize a new Git repository automatically when the directory is not yet a
repo. Remote GitHub setup still requires the explicit `/setup` path.

## Settings and configuration

The settings are global for your user account. They are not stored per
repository, and they persist across sessions.

The config file is stored under your OS config directory at:

```text
<config_dir>/remix-autopilot/config.json
```

On common systems, that usually resolves to one of these paths:

- Windows: `%AppData%\remix-autopilot\config.json`
- Linux: `~/.config/remix-autopilot/config.json`
- macOS: `~/Library/Application Support/remix-autopilot/config.json`

Press `F2` to open the settings UI after onboarding is complete. If required
setup is still missing, `F2` returns to onboarding instead. The available
settings are:

| Setting                    | Default     | Description                                                                           |
| -------------------------- | ----------- | ------------------------------------------------------------------------------------- |
| `provider`                 | `unset`     | Selected AI provider, such as Ollama, OpenAI, Gemini, or Anthropic.                   |
| `model`                    | `null`      | Selected provider model. If unset, you pick one in the UI.                            |
| `base_url`                 | `null`      | Optional custom endpoint for compatible providers.                                    |
| `api_key`                  | secret      | Provider API key, stored in the OS secret store instead of `config.json`.             |
| `language`                 | `English`   | UI language and preferred AI response language.                                       |
| `staged_only`              | `false`     | Restrict diff-based commands to staged changes only.                                  |
| `auto_setup_repo`          | `true`      | Auto-initialize Git repositories when needed for local flows.                         |
| `prompt_push_after_commit` | `true`      | Ask for confirmation before pushing after commit execution.                           |
| `theme`                    | `CodexDark` | TUI color palette.                                                                    |
| `history_limit`            | `Medium`    | Stored conversation length. Values are `Small` (20), `Medium` (40), and `Large` (80). |

The app currently supports these themes:

- `CodexDark`
- `Nord`
- `Sunset`
- `Dracula`
- `HighContrast`
- `Light`

### Reset configuration

Use `/reset` when you want to return Remix Autopilot to the first-run setup
flow. The command opens a confirmation modal before it changes anything.

The safe reset removes:

- The selected AI provider, model, base URL, and saved global preferences.
- API keys stored in the OS secret store for API-backed providers.
- The `origin` remote from the current Git repository, if one exists.

The safe reset doesn't remove `.git`, commits, branches, local files, or remote
history. After the reset completes, onboarding opens again so you can configure
the app from the beginning.

This is an example of the default config shape:

```json
{
  "provider": "unset",
  "model": null,
  "base_url": null,
  "language": "English",
  "staged_only": false,
  "auto_setup_repo": true,
  "prompt_push_after_commit": true,
  "theme": "CodexDark",
  "history_limit": "Medium"
}
```

## Offline and dependency behavior

The app makes a hard distinction between local features and network-dependent
features. That matters when the machine is offline or when a dependency is
missing.

- If the selected provider is unavailable, AI-backed commands are blocked
  until you restore that provider or choose another one.
- If the machine is offline, local Git and local Ollama flows still work when
  Ollama is selected, running, and has the requested model.
- If the machine is offline, API-backed providers are unavailable.
- If the machine is offline, `/push`, `/pr`, and remote GitHub setup are
  unavailable.
- If GitHub CLI is not installed or authenticated, GitHub-specific flows fail
  with a direct error instead of silently falling back.

The **Retry** action in onboarding only retries the relevant dependency check.
It does not bypass the requirement and it does not unlock AI workflows while
the selected provider is still unavailable.

## Troubleshooting

Most failures come from a missing dependency, a missing Git remote, or a repo
state mismatch. Start with the concrete message the TUI gives you.

### `autopilot` is not recognized

This means Cargo installed the binary, but the Cargo bin directory is not on
your `PATH`, or you are still using an older terminal session.

1. Verify that Cargo installed the binary.
   
   ```bash
   cargo install --list
   ```

2. Confirm that your Cargo bin directory is on `PATH`.

3. Restart the terminal.

### The onboarding wizard is open

The wizard opens when a required piece of setup is missing. Complete the
current step, choose a valid provider, or use **Retry** after fixing the
environment. `Esc` does not close this wizard; use `Ctrl+C` if you need to
quit the app. If you choose a provider that needs a model or API key, the
wizard also gives you a **Change provider** action so you don't get stuck.

### The AI provider is not selected or configured

Open onboarding with `/setup` when setup is required, or open settings with
`F2` after onboarding is complete. Pick a provider, choose a model, and add an
API key if the provider needs one.

### Ollama is missing

This only applies when you choose the Ollama provider. If Ollama is not in
`PATH`, onboarding shows the download URL. Install Ollama from
<https://ollama.com/download>, start it, then use **Retry**.

### Ollama is installed but not responding

If Ollama is installed but not listening on `localhost:11434`, onboarding stays
open and tells you to start the desktop app or run:

```bash
ollama serve
```

After Ollama is actually responding, use **Retry**.

### This directory is not a Git repository

Complete the onboarding Git step, run `/setup`, or leave `auto_setup_repo`
enabled and use a local workflow such as `/commit` to initialize Git
automatically when appropriate.

### This repository has no `origin` remote

Complete the onboarding remote step or use `/setup` to add an existing remote
or create a new GitHub repository. Commands such as `/push` and `/pr` depend on
`origin`.

### `/pr` fails even though Git works

`/pr` depends on GitHub CLI, not only Git. Verify both installation and auth:

```bash
gh auth status
```

If that fails, run `gh auth login`.

### The commit plan looks too large or low quality

Try one of these reductions before rerunning `/commit`:

- Stage a smaller subset of files and enable `/staged`.
- Switch to a model that handles your repo style better.
- Commit unrelated changes separately before asking for a new plan.

## Development and local verification

If you are working on the CLI itself, keep the normal Rust maintenance loop
close at hand.

```bash
cargo run
cargo test
cargo clippy --all-targets --all-features -- -D warnings
```

For installation testing during development, reinstall the global binary with
`cargo install --path . --force`.

## Architecture

The codebase is intentionally layered so UI work, domain logic, and external
tool integration stay separate.

- `domain`: core types, diff shaping, commit-plan rules, settings models.
- `application`: use cases that coordinate Git, AI providers, GitHub CLI, and
  config.
- `infrastructure`: adapters for Git, provider clients, GitHub CLI, config
  persistence, and environment checks.
- `ui`: TUI state machine, rendering, input handling, modals, and status UI.

## Next steps

Once the install is stable, the next useful checks are practical:

1. Run `autopilot` inside a real repository.
2. Complete onboarding and choose an AI provider.
3. Test `/diff` and `/review` on a small change.
4. Test `/commit` on a branch that is not `main` or `master`.
5. Test `/switch` and `/pr` only after `origin` and `gh auth login` are ready.
