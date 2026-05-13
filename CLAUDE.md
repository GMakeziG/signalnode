# SignalNode

- Keep changes small and issue-scoped
- Prefer minimal diffs over large refactors
- Do not move/delete existing files unless necessary
- Use test-driven changes where practical
- Keep Axum handlers thin
- Prefer explicit errors and tracing::error! for internal failures
- Security fixes before feature expansion
- Commit after each logical checkpoint
