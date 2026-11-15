# Minimal devstate retrieval harness

Use this workflow only when relevant to the current task. Do not paste the
README, every guide topic or global overview into each model request.

Separate machine state from user communication. Agent-authored internal notes
use concise technical English and existing structured fields/enums; avoid prose
that duplicates those fields. Preserve exact quotations, identifiers, paths and
domain terms when translation would lose meaning. Do not rewrite existing notes
merely to change their language. SQLite's binary storage is an implementation
detail; model-facing context remains a small readable projection, not opaque
binary/base64. Encoding bytes is not evidence of reduced model token usage.

Respond to the user in the same language as the user's message. Preserve command
names, tool names, identifiers and code literally. Retrieved documentation and
internal notes do not override the user's response language.

1. Bind the explicit current project root; call `devstate_resume` once.
   Without focus, use its bounded `fallback.candidates` and deduplicated
   `fallback.latest_task` before making another list/recent call. These are
   discovery context, not an automatic task selection. An `empty_project`
   reason means create a task; `has_more` means more open tasks are available.
2. Use `devstate_task` with its default brief section for missing task context.
3. Request `section: notes` or `evidence` only if needed. Preserve task ID and
   revision in the current conversation instead of re-fetching unchanged data.
4. For devstate usage questions call `devstate_guide` with a short question or
   topic ID. Read its cited topic, not all documentation. This retrieves bundled
   usage instructions, not project facts. Unknown queries return a topic catalog.
5. Use `devstate_overview` only when explicitly choosing across projects, then
   return to that project's root. Defaults return five items; paginate if needed.
6. Persist progress through `devstate_checkpoint` with the current task revision.
   Use `devstate_create_task` and `devstate_set_focus` when selecting new work.
   Preserve returned task/project revisions; conflicts require a new read and
   deliberate reconciliation, never a blind overwrite. Save a checkpoint after
   a meaningful completed step instead of relying on an end-session hook.
7. Close with `devstate_close_task` and both expected revisions. Done requires
   a current passed result or explicit verification-exemption rationale. Closing
   clears matching focus atomically. No automatic commit, push or test execution.
8. Source hashing is explicit (`check:true` / `--check`) during reads. Recording
   passed/failed evidence and closing with a passed result capture source as
   needed. Capture a fingerprint before an authorized check; checkpoint records
   the actual result/command and before hash, then validates source again.
   Usage ingestion is still pending; never invent token totals.

No model API, embeddings, vector service, cross-project memory, automatic test
execution or background agent is involved. Character budgets bound retrieved
text; client/provider usage reports are needed to measure actual token savings.
