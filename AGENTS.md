# Agent execution constraints

## Bounded command output

Never stream potentially high-volume commands directly to the terminal or tool output. This
includes fuzzers, mutation testing, benchmarks, full quality gates, recursive diagnostics, and
verbose builds.

- Redirect complete stdout and stderr to a temporary log file. Do not use `tee`.
- Configure quiet mode plus explicit runtime, memory, concurrency, and per-test limits whenever
  the tool supports them.
- On success, print only a compact summary. On failure, print at most the final 100 log lines.
- A tool response token limit is not an output-safety mechanism; bound the producing process and
  redirect its output before it reaches the terminal.
- After an interruption or timeout, verify that no child process remains before continuing.
- Stop the command immediately if output volume, memory, swap, or disk usage grows unexpectedly.

For libFuzzer runs, use `-verbosity=0`, `-rss_limit_mb=1024`, a bounded
`-max_total_time`, and a bounded per-input `-timeout`, with all output redirected to a temporary
log. Copy the tracked seed corpus to a task-specific temporary directory and fuzz that copy so
newly minimized inputs never dirty the repository or make later status output unbounded.
