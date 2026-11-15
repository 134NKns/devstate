Agent loop: resume once -> inspect brief task -> retrieve only missing notes/evidence -> do work.
Respond in the same language as the user's message; preserve command/tool names literally.
For agent-authored internal state use structured fields/enums and concise technical English; preserve exact quotes and domain terms. Do not translate existing records just for storage. Binary storage and base64 are not shortcuts to fewer model tokens.
Request guide only when unsure of devstate usage. Do not load the full README or all guide topics each turn.
Use small list limits, narrow searches and task IDs; avoid repeated unchanged reads.
Default task is brief; full detail is opt-in. Overview is explicit and cross-project summaries stay outside local context unless requested.
Token totals are provider-reported input/output, not guesses. unknown/partial/complete describes coverage. Elapsed time includes pauses. Smaller payloads do not prove lower billed tokens without client measurements.
