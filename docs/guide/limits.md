Current v1 supports init, task create/checkpoint/focus/close, focused/recent queries, search, notes/evidence/usage reads, source fingerprints and registered-project overview.
Write tools: devstate_create_task, devstate_checkpoint, devstate_set_focus, devstate_close_task. Revisions prevent lost updates. Closing a focused task clears focus atomically. Done requires current passed evidence or an explicit not_required_reason.
Automatic usage ingestion is NOT implemented. Do not invent usage totals or claim an unsuccessful checkpoint was saved.
Latest checkpoint is not a session history or source backup. Git does not recover overwritten DB state; use proper SQLite backup for a live WAL DB.
This usage guide is curated retrieval, not an LLM answer service, code search or project memory. It never queries another project's task data.
