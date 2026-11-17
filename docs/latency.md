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
not actual pixel presentation. The callback can be canceled and rescheduled by another commit: it is the next RAF of a surviving commit, not necessarily the earliest frame for that input. Check native `window-visible`/`window-hidden` state at the endpoint too. Native hook reception is not physical keydown.
Native Instant durations and JS durations use separate clocks. Cross-process unix
correlation has approximately 1ms resolution in WKWebView; never claim sub-ms
bridge latency or add independent quantiles. Missing visible endpoints do not imply
lost inputs. Canceled/superseded requests must not be counted as completed display.

For Computer Use measurements, time the input request and correlate its unique hook by session/revision. Request-start to RAF includes automation delivery and scheduling. The automation call returning does not establish an OS key-dispatch timestamp. Do not reintroduce a global keyboard monitor for diagnostics: a temporary monitor coincided with duplicate input; disabling it and restarting together restored single input, without isolating individual causality. Record foreground application, edit buffer, workload and matched endpoint count; exclude setup/wrong-focus samples. If baseline visible endpoints are absent, report that gap instead of a total-display speedup. State sample counts and retain hidden, superseded and missing-endpoint counts separately. Ten visible samples do not establish tail-latency guarantees. Keep per-sample receipts outside the repository.

Main-loop diagnostics categorize handlers without formatting their payloads.
`loop-*` spans below 0.5ms are omitted to reduce observation overhead. Queue lock
and enqueue durations distinguish producer contention from time awaiting dispatch.
`geometry-screen` and `geometry-window-bottom` split native geometry work; the latter reads cached bounds. `window-frame-worker` measures query work inside the helper; `window-frame-worker-ipc` includes process startup when cold, pipe delivery and response decoding. The same work/IPC pair exists for other isolated native lanes. Moving that query off main does not make the query itself faster: one local capture still measured roughly 2–11ms for the worker query, not a platform-wide bound. Overlap
between a slow input and a handler is evidence of occupation, not proof that an
unrelated earlier outlier had the same cause. Unattributed intervals can include
AppKit/WebKit work outside our callback and OS scheduling; use a separate native
stack sampling run if needed. Do not compare sampled and unsampled runs as a strict A/B.

Backend comparison (same 1,000-entry fixture, alternating old/new, release build):

```sh
cargo nextest run --locked --cargo-profile release -p fig_shell_ipc \
  --run-ignored only --no-capture -E 'test(directory_backend_benchmark)'
```

This compares the enumeration implementation with the older `ls` fallback and excludes the current process boundary and UI. To compare the direct backend with disposable-process round trips (50 samples after 10 warmups, alternating order and equal outputs):

```sh
cargo run --locked --release --target aarch64-apple-darwin -p fig_desktop \
  --example io_isolation_benchmark -- target/aarch64-apple-darwin/release/fig_io_worker
```

This includes helper startup, IPC and reaping on every isolated call, but excludes the WebView and terminal. It uses a 1,000-entry directory and `/bin/pwd`; local results do not predict remote-volume tails or physical key-to-display latency. Directory responses preserve each name as a separate
string, append `/` to folders when requested, and follow directory symlinks. Legacy
callers receive plain names by default. There is no persistent directory cache;
existing generator triggers govern refresh, so a new read sees filesystem changes.

Compare icon query cost with the same native rendering function, alternating direct and isolated calls and checking decoded pixel equality:

```sh
cargo run --locked --release --target aarch64-apple-darwin -p fig_desktop \
  --example native_isolation_benchmark -- target/aarch64-apple-darwin/release/fig_native_worker
```

Cold helper startup and warm round trips are separate costs. This benchmark bypasses the icon cache and does not measure terminal-to-display latency. Isolation removes native icon work from the desktop main thread, not the work itself.


Compare stationary-window query cadence with read-only requests to an existing terminal:

```sh
cargo run --locked -p fig_desktop --example frame_watch_benchmark -- \
  /Applications/Fig.app/Contents/MacOS/fig_native_worker TERMINAL_PID
```

It alternates fixed/adaptive schedules in two reversed pairs of five-second windows, reporting query counts, missing frames and round-trip durations. It does not move the window or measure drag accuracy, CPU/energy use or key-to-display latency. Deterministic cadence tests separately verify the 20 Hz drag path and bounded settling period. `just latency` measures PTY write-to-hook/echo; compare the same CLI build profile and workload, and report sample counts.
