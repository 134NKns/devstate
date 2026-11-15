Initialize: devstate --root /absolute/project init --name my-project
MCP: devstate_init({"root":"/absolute/project","name":"my-project"}).
init creates .devstate/state.db once; it refuses an existing DB. Ignore .devstate/ in Git.
--root defaults to exact cwd in CLI. MCP always requires an explicit absolute root.
No parent scanning, last-project fallback or automatic global registration.
For agents use --json. JSON formatting: --pretty. No arguments shows CLI help.
