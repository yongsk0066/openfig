# macOS input method

OpenFig uses an InputMethodKit palette helper for exact caret placement in official Ghostty. The helper queries the focused text client; it does not replace the terminal binary or the normal keyboard input source.

## Installation and activation

`just install` places the helper inside `/Applications/Fig.app/Contents/Helpers/FigInputMethod.app` and links `~/Library/Input Methods/FigInputMethod.app` there. Keep the bundle/source identifiers and connection/controller names consistent with the generated helper plist. Register the installed bundle, not a workspace build.

The installer performs these distinct steps:

1. Register the source once, then check discovery from a fresh process up to 20 times, sleeping 250ms between failed checks (about five seconds plus query/startup time).
2. Launch the helper before the terminal initializes its input context.
3. Request activation through `q debug enable-input-method`; IPC dispatches the public `TISEnableInputSource` call to the desktop GUI main loop, replacing the temporary probe/headless activation path.
4. In Settings → Integrations, **Set up input method** requests activation and returns while consent is pending. After approving in macOS, return to Fig and choose **Finish activation** if the source still needs selection. **Check again** reads the registered source without changing permissions. In the command-line installer, wait for system approval when required, checking `enabled` from fresh processes against a 120-second polling deadline.
5. Select the enabled palette, then confirm both `enabled` and `selected` from another fresh process. An already-active source skips the activation request.

A successful activation request is not proof of approval. A timeout reports unconfirmed activation and leaves the installed desktop running. The installer compares signed code-directory hashes, preserves the helper across signature-only updates and negotiates its shutdown when sealed code changes. Already-approved terminal clients reconnect automatically after helper replacement or death; user terminals are never terminated. Initial discovery/approval remains a separate installation step. Keep your usual keyboard layout selected.

## Troubleshooting

Check the symlink and installed signature first, then inspect fresh status:

```sh
readlink "$HOME/Library/Input Methods/FigInputMethod.app"
codesign --verify --deep --strict /Applications/Fig.app
/Applications/Fig.app/Contents/Helpers/FigInputMethod.app/Contents/MacOS/fig_input_method --status
```

Enabled/selected flags do not prove a live text client. The socket protocol separately reports connection readiness, retirement and foreground-client attachment. A detached foreground target is retried at 250ms without requiring another key or a visible popup. After 750ms of missing attachment, the desktop main loop acquires a selection lease for the exact enabled palette, deselects it if needed, then selects it after two seconds of run-loop progress. The normal keyboard source is untouched. Failures back off through 5, 10, 20 and at most 30 seconds; a healthy client resets the policy. Background targets stop the attachment polling.

Graceful shutdown calls public `IMKServer.paletteWillTerminate()` asynchronously, allowing up to 20 attempts at 100ms intervals for client acknowledgements; refusal or exception cancels termination. The desktop allows a five-second retirement grace period, cleared when the helper reconnects, so recovery does not race client cleanup. Normal palette termination can clear selection while approval remains enabled. A desktop-owned selection lease survives helper death and restores selection on normal owner cancellation. A two-second helper watchdog bounds stalled IMK calls; it is not a guarantee against OS-wide hangs or a process suspended by the OS.

On macOS 27.3, an installed-app test kept the same official Ghostty PID through one graceful helper restart and two SIGKILL restarts. The popup reappeared after 4.39s, 3.23s and 3.47s respectively; both SIGKILL cases needed no additional shell input, and the keyboard source was unchanged. These three observations are not a recovery-time SLA or validation across all macOS versions.

Restarting the **desktop** is a separate transport boundary. The current zsh hook retries while the editor waits for input, then resends environment, aliases, working directory and the current buffer/cursor. It does not execute the buffer or create a prompt. Existing shells that loaded an older hook still need that code refreshed; helper-only recovery preserves their desktop connection.

On the investigated machine, Keyboard Settings denied access through a workspace-targeted symlink and its discovery cache retained an obsolete test identifier. Installing at the standard path, refreshing discovery and requesting activation from a GUI main loop produced the approval prompt. These combined observations do not isolate one OS-version cause or establish a general macOS prohibition.

The installer does not edit HIToolbox preferences, clear system caches or reset security permissions. Diagnose discovery/cache identity before manual recovery. The already-approved production path was verified. Clean first-time approval through the production desktop path has not been re-tested; preserve existing grants rather than resetting them solely for a test. Mocked tests do not establish live consent behavior across macOS releases.

See [development](development.md) for app replacement/recovery and [latency diagnostics](latency.md) for display timing.
