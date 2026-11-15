-- devstate schema v1: one SQLite database per project.
-- Initialization DDL; run inside the caller's transaction on a new database.
-- Requires SQLite >= 3.42 (STRICT tables and subsecond timestamps).
-- Connection setup: foreign_keys=ON, busy_timeout=5000, synchronous=FULL.
-- Configure journal_mode=WAL during init; keep write transactions short.
-- FULL is intentional: an acknowledged checkpoint should survive power loss
-- subject to the filesystem/hardware honoring SQLite's durability requests.
-- The Rust connection layer configures PRAGMAs before opening that transaction.

-- IDs are application-generated UUID strings. Times are UTC Unix milliseconds.
-- spec_path is repository-relative; validate containment in the application.
-- Text caps bound storage, NOT tokens; the response layer controls its budget.
CREATE TABLE tasks (
    id                  TEXT PRIMARY KEY CHECK (length(trim(id)) BETWEEN 1 AND 64),
    title               TEXT NOT NULL CHECK (length(trim(title)) BETWEEN 1 AND 200),
    kind                TEXT NOT NULL DEFAULT 'task' CHECK (kind IN ('task', 'bug')),
    notes               TEXT NOT NULL DEFAULT '' CHECK (length(notes) <= 8000),
    spec_path           TEXT CHECK (spec_path IS NULL OR length(trim(spec_path)) BETWEEN 1 AND 4096),
    status              TEXT NOT NULL DEFAULT 'todo'
                        CHECK (status IN ('todo', 'active', 'blocked', 'done', 'cancelled')),
    phase               TEXT NOT NULL DEFAULT 'define'
                        CHECK (phase IN ('define', 'investigate', 'plan', 'implement', 'review', 'verify', 'deliver')),

    -- Latest checkpoint only; never load historical sessions to resume.
    summary             TEXT NOT NULL DEFAULT '' CHECK (length(summary) <= 1200),
    next_action         TEXT NOT NULL DEFAULT '' CHECK (length(next_action) <= 600),
    blockers            TEXT NOT NULL DEFAULT '' CHECK (length(blockers) <= 600),
    git_branch          TEXT CHECK (git_branch IS NULL OR length(git_branch) BETWEEN 1 AND 1024),
    git_head            TEXT CHECK (git_head IS NULL OR (
                            length(git_head) IN (40, 64)
                            AND git_head NOT GLOB '*[^0-9a-f]*')),

    -- Nullable means source identity is unknown, never implicitly clean.
    -- A fingerprint includes a versioned scope policy and file contents;
    -- git_head or a dirty boolean alone cannot identify uncommitted source.
    source_scope        TEXT CHECK (source_scope IS NULL OR length(trim(source_scope)) BETWEEN 1 AND 1000),
    source_fingerprint  TEXT CHECK (source_fingerprint IS NULL OR (
                            length(source_fingerprint) = 64
                            AND source_fingerprint NOT GLOB '*[^0-9a-f]*')),
    verification_status TEXT NOT NULL DEFAULT 'not_run'
                        CHECK (verification_status IN ('not_run', 'passed', 'failed', 'not_required')),
    verification_notes  TEXT NOT NULL DEFAULT '' CHECK (length(verification_notes) <= 4000),
    verified_fingerprint TEXT CHECK (verified_fingerprint IS NULL OR (
                            length(verified_fingerprint) = 64
                            AND verified_fingerprint NOT GLOB '*[^0-9a-f]*')),
    verified_at         INTEGER,

    -- Task-level totals only. NULL means unknown, never an assumed zero.
    input_tokens        INTEGER CHECK (input_tokens IS NULL OR input_tokens >= 0),
    output_tokens       INTEGER CHECK (output_tokens IS NULL OR output_tokens >= 0),
    usage_status        TEXT NOT NULL DEFAULT 'unknown'
                        CHECK (usage_status IN ('unknown', 'partial', 'complete')),
    -- JSON array of unique provider/model strings, validated by the application.
    -- [] means no model information reported, not proof that none was used.
    models_used         TEXT NOT NULL DEFAULT '[]'
                        CHECK (length(models_used) <= 4096)
                        CHECK (CASE WHEN json_valid(models_used)
                            THEN json_type(models_used) = 'array' ELSE 0 END),

    revision            INTEGER NOT NULL DEFAULT 1 CHECK (revision >= 1),
    created_at          INTEGER NOT NULL DEFAULT (CAST(unixepoch('subsec') * 1000 AS INTEGER)),
    updated_at          INTEGER NOT NULL DEFAULT (CAST(unixepoch('subsec') * 1000 AS INTEGER)),
    -- First actual start, not task creation. Preserve across pauses/restarts.
    -- Elapsed wall time = closed_at - started_at; includes pauses/offline time.
    started_at          INTEGER,
    closed_at           INTEGER,
    CHECK (updated_at >= created_at),
    CHECK (started_at IS NULL OR started_at BETWEEN created_at AND updated_at),
    CHECK (closed_at IS NULL OR closed_at BETWEEN created_at AND updated_at),
    CHECK (started_at IS NULL OR closed_at IS NULL OR closed_at >= started_at),
    CHECK (status NOT IN ('active', 'blocked', 'done') OR started_at IS NOT NULL),
    CHECK (verified_at IS NULL OR verified_at BETWEEN created_at AND updated_at),
    CHECK (
        (usage_status = 'unknown' AND input_tokens IS NULL AND output_tokens IS NULL)
        OR (usage_status = 'partial' AND (input_tokens IS NOT NULL OR output_tokens IS NOT NULL))
        OR (usage_status = 'complete' AND input_tokens IS NOT NULL AND output_tokens IS NOT NULL)
    ),
    CHECK ((source_scope IS NULL) = (source_fingerprint IS NULL)),
    CHECK (
        (status IN ('done', 'cancelled') AND closed_at IS NOT NULL)
        OR (status NOT IN ('done', 'cancelled') AND closed_at IS NULL)
    ),
    CHECK (status NOT IN ('active', 'blocked') OR length(trim(next_action)) > 0),
    CHECK (status != 'blocked' OR length(trim(blockers)) > 0),
    CHECK (verification_status = 'not_run' OR length(trim(verification_notes)) > 0),
    CHECK (
        (verification_status IN ('not_run', 'not_required')
            AND verified_fingerprint IS NULL AND verified_at IS NULL)
        OR (verification_status IN ('passed', 'failed') AND verified_at IS NOT NULL)
    ),
    -- A passed result with an unknown source is recordable, but cannot prove
    -- completion. not_required needs an explicit rationale in notes above.
    CHECK (status != 'done' OR verification_status = 'not_required' OR (
        verification_status = 'passed'
        AND source_fingerprint IS NOT NULL
        AND verified_fingerprint IS NOT NULL
        AND source_fingerprint = verified_fingerprint
    ))
) STRICT;

-- A DB has exactly one project after init inserts this singleton row.
-- project_id is a stable UUID, independent of directory names and remotes.
-- Never infer project identity from a mutable global "last project".
CREATE TABLE project (
    singleton           INTEGER PRIMARY KEY CHECK (singleton = 1),
    project_id          TEXT NOT NULL UNIQUE CHECK (length(trim(project_id)) BETWEEN 1 AND 64),
    name                TEXT NOT NULL CHECK (length(trim(name)) BETWEEN 1 AND 120),
    root_path           TEXT NOT NULL CHECK (length(trim(root_path)) > 0),
    -- Focus selects what resume opens; several tasks may be active at once.
    focused_task_id     TEXT REFERENCES tasks(id) ON DELETE RESTRICT,
    revision            INTEGER NOT NULL DEFAULT 1 CHECK (revision >= 1),
    created_at          INTEGER NOT NULL DEFAULT (CAST(unixepoch('subsec') * 1000 AS INTEGER)),
    updated_at          INTEGER NOT NULL DEFAULT (CAST(unixepoch('subsec') * 1000 AS INTEGER)),
    CHECK (updated_at >= created_at)
) STRICT;

CREATE INDEX tasks_by_status_updated ON tasks(status, updated_at DESC, id DESC);
CREATE INDEX tasks_by_updated ON tasks(updated_at DESC, id DESC);
CREATE INDEX tasks_by_kind_updated ON tasks(kind, updated_at DESC, id DESC);

-- Application contract:
-- 1. Bind a process to one explicit canonical project root. Check identity
--    before reads/writes; a moved/copied DB requires explicit rebinding.
--    Store at <working-directory-root>/.devstate/state.db (gitignored), not
--    in the shared Git directory. Each clone/worktree has its own local state.
--    Never ATTACH other project databases or fall back to global memory.
-- 2. Mutations use WHERE id = ? AND revision = ?, increment revision and
--    set updated_at. Zero changed rows means conflict: re-read, never overwrite.
--    Apply the same rule to project using singleton = 1. Use UTC milliseconds
--    and max(current_time_ms, previous_updated_at) for updates. Ties sort by ID;
--    wall-clock timestamps do not establish a global causal order.
-- 3. Use BEGIN IMMEDIATE for multi-step mutations. Switching focus checks the
--    project revision and target eligibility without changing other tasks.
--    Focus may point to todo/active/blocked, never done/cancelled. Closing a
--    focused task clears its pointer atomically and increments both revisions.
--    A stale project revision rolls the entire operation back.
-- 4. Fingerprint v1: SHA-256 over a deterministic length-prefixed encoding of
--    the algorithm version, explicit scope policy, HEAD (or unborn marker),
--    and sorted scoped paths, types/modes, and content digests. Non-Git roots
--    and unsupported symlinks/submodules produce unknown source identity.
--    Include tracked edits/deletions, index state and relevant untracked files;
--    do not follow symlinks outside the root. Exclude .git, .devstate, secrets,
--    build outputs and dependencies via an explicit documented scope policy.
--    Scope must cover relevant test/config/lock files; record environment,
--    commands and exclusions in verification_notes or an evidence reference.
--    Unknown/unreadable inputs make the fingerprint NULL, not a partial hash.
-- 5. Capture fingerprints before AND after verification. Only attach a
--    verified_fingerprint when they match and capture was stable. Persist
--    completion time, actual result and evidence; never infer passed from done.
--    Fingerprints identify scoped source, not backups or proof of unchanged
--    external services/environment. Arbitrary concurrent file writes cannot
--    be made atomic by a DB transaction: require a stable tree while checking.
-- 6. Recompute source identity on explicit freshness reads and when closing
--    with passed evidence, not on every ordinary resume/task read.
--    Report freshness separately: current / stale / unknown / not_applicable.
--    Retain old evidence rather than relabeling a historical pass as not_run.
--    Reevaluate not_required rationale if scope changes. No automatic commit
--    or mandatory clean Git tree; unchanged uncommitted source is supported.
-- 7. resume follows focused_task_id only; NULL means no focus. Never guess
--    from the latest blocked task. Return a bounded projection, not SELECT *;
--    spec_path and verification_notes are fetched on demand. Enforce response
--    budgets in the tool layer, mark omitted fields, and provide detail IDs.
-- 8. List with separate filtered/unfiltered queries matching the indexes;
--    use (updated_at, id) keyset cursors. Concurrent updates can move rows
--    across cursors: pagination is live, not a guaranteed snapshot.
-- 9. Back up using SQLite Online Backup API or VACUUM INTO, not a live copy
--    of state.db alone. Latest checkpoint has no undo history; Git does not
--    recover overwritten DB state. Source files need their own backup.
-- 10. ponytail: no session/event archive, embeddings, or search index in v1.
--     Specs and delivery evidence remain in repository documents/Git.
-- 11. Usage is optional and sourced from actual tool/provider reports, never
--     model estimates. Input includes cached input; output includes reasoning
--     tokens where reported as output. Do not add either subset a second time.
--     Unsupported/ambiguous reports stay unknown or partial. Aggregate across
--     every model used for this task; validate/deduplicate provider/model names
--     in models_used. Completeness refers to token coverage, not model coverage.
--     Update absolute cumulative totals with the task revision, not blind
--     increments: retrying a write must not count the same report twice.
--     Producers must deduplicate reports before aggregation; this summary-only
--     schema cannot deduplicate individual requests. Gaps make usage partial.
-- 12. Set started_at on first activation, retain it through pauses and restarts.
--     Compute elapsed time at read time, using closed_at for terminal tasks
--     or current time for open tasks; label it elapsed time including pauses,
--     not active effort or model latency. Never-started tasks have unknown time.
--     Reopening terminal tasks is outside v1; starting new work creates a task.

PRAGMA user_version = 1;
