# Local latency diagnostics

Disabled by default. Launch the signed app with `FIG_LATENCY_TRACE=1` and
`FIG_LOG_LEVEL=error,fig_latency=info,fig_desktop::webview::runtime=info`.
Quit the existing app first; `open` otherwise reuses its environment.
Do not enable general trace logging to measure user input: it can include commands.

```sh
open -g /Applications/Fig.app --env FIG_LATENCY_TRACE=1 \
  --env FIG_LOG_LEVEL=error,fig_latency=info,fig_desktop::webview::runtime=info \
  --args --no-dashboard
python3 scripts/latency-report.py CAPTURE.log --output REPORT_DIRECTORY
```

Capture only the new portion of the native log. The diagnostic channel contains
stage names, numeric timing/counts and session/revision identifiers, not command
text or environment values. WebView batches are capped at 512 samples/128 KiB and
validated by the native receiver; in-memory snapshots are bounded. Timing logs
use the application's existing logger, which rotates oversized files at startup.
There is no continuous file-size cap. Use short diagnostic sessions; restart without the flag to stop.

Use `just latency-report CAPTURE.log REPORT_DIRECTORY` or the Python command above.
The previous order-based `latency-trace` tool has been retired.

Correlate by session and input revision. `input-to-current-suggestions-dom` requires
current parsed input, visible UI state and a nonempty list. A first DOM commit may
be a stale or hidden list and is not this endpoint. RAF is a rendering opportunity,
not actual pixel presentation. Native hook reception is not physical keydown.
Native Instant durations and JS durations use separate clocks. Cross-process unix
correlation has approximately 1ms resolution in WKWebView; never claim sub-ms
bridge latency or add independent quantiles. Missing visible endpoints do not imply
lost inputs. Canceled/superseded requests must not be counted as completed display.

Main-loop diagnostics categorize handlers without formatting their payloads.
`loop-*` spans below 0.5ms are omitted to reduce observation overhead. Queue lock
and enqueue durations distinguish producer contention from time awaiting dispatch.
`geometry-screen` and `geometry-window-bottom` split native geometry work. Overlap
between a slow input and a handler is evidence of occupation, not proof that an
unrelated earlier outlier had the same cause. Unattributed intervals can include
AppKit/WebKit work outside our callback and OS scheduling; use a separate native
stack sampling run if needed. Do not compare sampled and unsampled runs as a strict A/B.

Backend comparison (same 1,000-entry fixture, alternating old/new, release build):

```sh
cargo nextest run --locked --cargo-profile release -p fig_desktop_api \
  --run-ignored only --no-capture -E 'test(directory_backend_benchmark)'
```

This excludes transport/UI. Directory responses preserve each name as a separate
string, append `/` to folders when requested, and follow directory symlinks. Legacy
callers receive plain names by default. There is no persistent directory cache;
existing generator triggers govern refresh, so a new read sees filesystem changes.
