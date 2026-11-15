Register: devstate --root /absolute/project --registry /absolute/registry.db register
Overview: devstate --registry /absolute/registry.db overview --json
MCP: devstate_register({"root":"/absolute/project","registry":"/absolute/registry.db"}), devstate_overview({"registry":"/absolute/registry.db"}).
Registry stores only project paths/identities. Overview reads summaries independently; unavailable projects do not stop others.
After choosing a project, use its root for resume. Do not inject global overview into every local task.
