Find: devstate tasks --status blocked --kind bug --json
Search: devstate search 'callback' --json (literal substring; no language-specific stemming).
Inspect: devstate task ID --json returns brief context and usage.
Read only what is missing: task ID --section notes|evidence|usage|full --json.
MCP: devstate_tasks, devstate_search, devstate_task; pass absolute root and optional section.
Lists default to 5. Pass next_cursor with identical filters to read more; task ID avoids another search.
Create: devstate create --title 'Fix callback'. Focus: devstate focus ID --revision PROJECT_REV.
Save: devstate checkpoint ID --revision TASK_REV --summary 'API connected' --next 'Wire UI'.
Close: devstate close ID --revision TASK_REV --project-revision PROJECT_REV --summary 'Delivered'. Pass --reason only for an explicit verification exemption. Use returned revisions for subsequent writes.
