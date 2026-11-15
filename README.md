# devstate

**Pick up where you left off.**

Local development state for humans and coding agents. Save progress, next actions
and blockers, close your agent, then resume from the same checkpoint.

Built in Rust with a CLI, stdio MCP server and library API. Each project has its
own SQLite database. An optional registry shows project summaries without mixing
their task state. No account, model API, embeddings or hosted database required.

## Features

- Create, focus, checkpoint, resume, finish or cancel tasks.
- Prevent stale writes with task/project revisions and transactions.
- Retrieve brief context first; request notes and evidence only when needed.
- Browse registered projects, open tasks and bugs.
- Compare verification evidence with scoped source fingerprints.
- Retrieve relevant usage guidance instead of loading the entire manual.

## Install

Requires a current stable Rust toolchain and a C compiler for bundled SQLite.
Fingerprinting also requires Git; the `sqlite3` CLI is not required.

```sh
git clone https://github.com/134NKns/devstate.git
cd devstate
cargo install --path . --locked
devstate --help
```

Ensure Cargo's binary directory (normally `~/.cargo/bin`) is in `PATH`.
For development, use `cargo run -- <arguments>` or `cargo build --locked` followed
by `target/debug/devstate`. Local CLI/MCP use does not require a package release.

## Quick start

From the project you want to track:

```sh
devstate init --name my-project
devstate create --title "Document local setup"
```

Create returns an `id`, task `revision`, and `project_revision`. Substitute that
ID below. The example revisions assume a fresh DB and no concurrent updates;
use actual returned revisions when working.

```sh
# Select work without changing other tasks' statuses.
devstate focus TASK_ID --revision 1

# After making progress, record the next step.
devstate checkpoint TASK_ID --revision 1 \
  --summary "Setup notes drafted" \
  --next "Review installation instructions"

# Run again after restarting your terminal or agent.
devstate resume

# Once this documentation-only task is complete:
devstate close TASK_ID --revision 2 --project-revision 2 \
  --summary "Setup documentation completed" \
  --reason "Documentation-only task; runtime checks not required"
```

For code requiring verification, record an actual result instead of using an
exemption; see [Verification](#verification). Close clears matching focus
atomically. `--status cancelled` cancels work. Closed tasks cannot reopen in v1.

State lives at `.devstate/state.db`. Ignore `/.devstate/` in `.gitignore` or local
Git exclude rules. Devstate never automatically runs tests, commits or pushes.

## CLI

| Command | Purpose |
| --- | --- |
| `resume` | Focused checkpoint, next action, blockers and revisions |
| `tasks --status blocked --kind bug` | Filter work |
| `recent` | Recently updated tasks across statuses |
| `search "callback"` | Literal title/summary/notes search |
| `task TASK_ID` | Brief context, models, token totals and elapsed time |
| `task TASK_ID --section notes` | Notes only |
| `task TASK_ID --section evidence` | Verification evidence |
| `task TASK_ID --section usage` | Usage summary |
| `task TASK_ID --section full` | Full record |
| `focus --revision PROJECT_REV` | Clear focus without changing task status |
| `guide tokens` | Relevant usage instructions |

`--root` defaults to the **exact current directory**. It never scans parents or
selects a remembered project. From elsewhere, supply the root explicitly:

```sh
devstate --root /absolute/project resume --json
```

Human output is the default. `--json` emits compact JSON; `--pretty` emits indented
JSON. Errors go to stderr: runtime exit code 1, argument exit code 2. Help/version
use code 0. Human views inherit terminal colors and respect `NO_COLOR`/`TERM=dumb`.

Statuses: `todo`, `active`, `blocked`, `done`, `cancelled`. For blocked work,
checkpoint with `--status blocked --blockers "Reason"` and a concrete `--next`.
Checkpoints replace the latest task state; they do not create session files.

Keep returned revisions. A conflict requires a new read and reconciliation, never
a blind retry with an incremented revision. Focus/close require project revision;
checkpoint/close require task revision. Resume exposes the revisions you need.

Lists default to five items, capped at 50. Pass task-list `next_cursor` as one JSON
`--cursor` argument with identical filters. Overview uses a root-path cursor.
Pagination is live: concurrent updates can move rows between pages.

## Browse projects

```sh
devstate --root /work/project-a --registry /absolute/registry.db register
devstate --root /work/project-b --registry /absolute/registry.db register
devstate --registry /absolute/registry.db overview
devstate --registry /absolute/registry.db unregister /saved/project/path
```

Registry stores only canonical paths and identities. Overview reads each project
independently and read-only; unavailable projects do not stop others. It never
changes focus or automatically enters local resume context. Unregister removes
only a pointer, even when its directory is missing. Only register creates a
missing registry. The registry path is always explicit.

## MCP

Configure the client to launch `devstate mcp` with an absolute binary path.

**Codex:**

```sh
codex mcp add devstate -- /absolute/path/to/devstate mcp
```

**OpenCode:**

```sh
opencode mcp add devstate --global -- /absolute/path/to/devstate mcp
```

**JSON-based client configuration:**

```json
{
  "mcpServers": {
    "devstate": {
      "command": "/absolute/path/to/devstate",
      "args": ["mcp"]
    }
  }
}
```

Restart/reconnect the MCP process after rebuilding to load new tools. Registration
does not initialize project databases.

| Tools | Purpose |
| --- | --- |
| `devstate_init` | Initialize one project |
| `devstate_create_task`, `devstate_set_focus` | Create/select work |
| `devstate_checkpoint`, `devstate_close_task` | Persist/finish work with revisions |
| `devstate_resume`, `devstate_task` | Focused context and optional detail sections |
| `devstate_tasks`, `devstate_search` | List/filter/search local tasks |
| `devstate_fingerprint` | Capture scoped source identity |
| `devstate_register`, `devstate_unregister`, `devstate_overview` | Explicit project registry |
| `devstate_guide` | Retrieve usage guidance without project access |

Project tools require an absolute `root`; registry tools require an absolute
`registry`. Guide requires neither. No call falls back to the previous project's
root. Write tools require expected revisions and are annotated separately from
reads. Stored notes are data, not instructions.

Transport is synchronous newline-delimited JSON-RPC over stdio, without a network
listener. Supports initialize, ping and tools; protocol versions 2024-11-05,
2025-03-26, 2025-06-18 and 2025-11-25. Oversized frames (>1 MiB) end the connection;
EOF shuts down normally. Progress/cancellation capabilities are not advertised.
Results include structured JSON and a matching text representation for client
compatibility; client behavior determines what enters model context.

## Agent harness and context size

Follow the [small working loop](docs/HARNESS.md): resume once, retrieve missing
sections only, checkpoint meaningful steps, and close when acceptance is met.
Use global overview only when choosing between projects.

```sh
devstate guide resume --json
devstate guide tokens --json
```

Guide retrieves bundled topics with exact IDs, keyword aliases and literal terms.
It is not semantic/vector search or generated advice. Defaults: two topics and
1800 content characters total, with truncation flags or a catalog for no matches.
It does not read project notes or index source code.

Agent-authored internal notes use concise technical English and structured fields,
preserving exact quotes, identifiers and domain terms. User-facing responses
follow the user's language. Existing records are not auto-translated.

Brief/detail projections, small lists and overview deduplication reduce payloads.
Character budgets and binary/base64 encoding do not guarantee token savings.
Actual model usage depends on the tokenizer, client and workflow.

## Verification

Normal reads do not hash source. Request `resume --check` or `task TASK_ID --check`
(MCP `check:true`) to compute current/stale/unknown/not_applicable freshness.

For an authorized check, capture `fingerprint --json` beforehand, run the actual
check with stable source, then pass its result to checkpoint's `--verification`:

```json
{
  "status": "passed",
  "notes": "Actual command, result and relevant environment",
  "before": "FINGERPRINT_CAPTURED_BEFORE_THE_CHECK"
}
```

Passed/failed results require the before hash. Checkpoint captures source again
and rejects mismatches. Completion rechecks passed evidence or requires a genuinely
applicable `--reason`/`not_required_reason`. Devstate does not execute or independently
attest to the check. For `not_required`, record the rationale and omit `before`.

The fixed `git-source-v1` SHA-256 scope covers HEAD/unborn state, index entries,
file content/modes, tracked deletions and non-ignored untracked files. Staging or
committing can change it. Two scans reject observed changes, but do not provide
an atomic filesystem snapshot; environment/services are outside the fingerprint.

Excluded components: `.git`, `.devstate`, `node_modules`, `target`, `dist`, `build`,
`.next`, `.venv`, `vendor`, `secrets`, `.env`, `.env.*`, `*.pem`, `*.key`, including
matching tracked files. This is not automatic secret discovery. Non-Git roots,
symlinks, submodules, non-UTF-8 paths and incomplete reads yield unknown identity.

## Storage and limits

- One local SQLite DB per working directory, schema draft v1. Clones/worktrees do
  not share state. Root/identity mismatches fail closed.
- Initialization refuses existing files. Interrupted init can leave an incomplete
  file; inspect and back it up before recovery. No automatic migration/rebinding.
- Latest checkpoint only; no event/session history, undo or source backup.
  For a live WAL DB use SQLite Online Backup API or `VACUUM INTO`; copying only
  `state.db` is not a reliable live backup.
- Schema/reads support token totals, model names and usage coverage, but automatic
  usage collection and a usage-update interface are not implemented. Missing
  usage stays unknown. Elapsed time includes pauses/offline time.

## Development

```sh
cargo test --locked --offline
```

Omit `--offline` on a fresh checkout that needs dependency downloads. The current
37 integration tests cover CLI/MCP lifecycle, persisted resume, revision conflicts,
rollback, focus-safe completion, fingerprints, context projection and isolation.
A long-notes/evidence fixture returned 517 bytes brief vs 8246 bytes full JSON;
this is payload measurement, not billed tokens or a general savings guarantee.
This is an early v1 draft; APIs may change.
