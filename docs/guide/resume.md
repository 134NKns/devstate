Continue: devstate --root /absolute/project resume --json
MCP: devstate_resume({"root":"/absolute/project"}).
Read this once when entering a project. With explicit focus it returns the current summary, next action and blockers.
Without focus, task stays null and fallback returns up to 3 recently updated open candidates plus latest_task when not already included. Each includes its ID, revision, updated_at (UTC Unix milliseconds) and short checkpoint; no task is selected automatically.
fallback.reason is no_focus or empty_project. has_more means additional open tasks, not closed history; false does not mean an empty project. Use tasks only if the candidates do not cover your work. For an empty project, create a task. Select work explicitly with focus ID --revision PROJECT_REV.
Candidate text is capped at 120 title, 240 summary, 200 next-action and 200 blocker characters; truncated marks shortened entries. Notes and evidence stay opt-in. Fallback discovery does not hash source, including with --check.
If checkpoint text is shortened, request task ID for more context. Avoid re-reading unchanged checkpoints.
Use returned task revision for checkpoint and project revision for focus/close. Save progress with devstate_checkpoint before ending work; stale revisions require a fresh read, not a blind retry. Source freshness is opt-in via check:true (CLI --check).
