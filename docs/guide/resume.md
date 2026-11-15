Continue: devstate --root /absolute/project resume --json
MCP: devstate_resume({"root":"/absolute/project"}).
Read this once when entering a project. It returns only explicit focus, current summary, next action and blockers.
No focus means no selected task; inspect tasks rather than guessing from latest updates.
If checkpoint text is shortened, request task ID for more context. Avoid re-reading unchanged checkpoints.
Use returned task revision for checkpoint and project revision for focus/close. Save progress with devstate_checkpoint before ending work; stale revisions require a fresh read, not a blind retry. Source freshness is opt-in via check:true (CLI --check).
