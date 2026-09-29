# Task: Remediation Worker — Final Polish & Test Green

1. `SpotArbitrageBot.tsx`:
   - Line 267: Pass `isPaused={isPaused}` to `<BotPhaseBadge phase={bot.phase} isPaused={isPaused} />`.
   - Line 571: Update dock tabs container to `className="p-3.5 h-96 overflow-y-auto"` to match `h-96` standard.
2. Backend Test:
   - Run `cargo test -p arbitrageur-desktop -- test_emergency_flatten_retries_partial_close`.
   - If it fails, inspect `frontend/src-tauri/src/execution.rs` and `commands.rs`: Ensure repeat emergency flatten calls properly recognize existing partial / in-flight close children so duplicate close child signals are not spawned.
3. Verification:
   - Run `cd frontend && npm test`
   - Run `cargo test -p arbitrageur-desktop`
   - Ensure both pass with zero failures.
