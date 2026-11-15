Capture: devstate fingerprint --json or devstate_fingerprint({"root":"/absolute/project"}).
Resume/task reads do not hash source by default. Request check:true (CLI --check) for freshness: current matches scoped source; stale differs; unknown cannot establish identity; not_applicable has no check to compare.
It does not run tests. Capture before/after an authorized check; only matching stable snapshots may support that result.
Record the result in checkpoint.verification: {"status":"passed","notes":"actual command/result","before":"PRE_CHECK_HASH"}. The server captures source again and rejects changed source. Completion rechecks passed evidence. not_required needs a rationale, not a fake pass.
Scope git-source-v1 includes Git HEAD/index, tracked edits/deletions and relevant untracked source. State, output/dependency directories and common secret patterns are excluded.
Non-Git, symlinks/submodules and incomplete reads are unknown. Environment/services are not verified by a source hash.
