#!/usr/bin/env bash
# Stage 0 latency, desktop half (docs/16-latency.md): per-keystroke hop timings from a desktop log
# written at FIG_LOG_LEVEL=trace, summarised as n / median / p90 / max per hop.
#
#   scripts/latency-trace.sh [--session ID] [--since 2026-09-17T11:58:00] [--rows] LOG
#
# The log has no id that follows one keystroke through the desktop, so the lines are joined by
# order: every `edit_buffer` hook opens a window, and each later hop is the first matching line
# after the previous hop and before the next hook. That is exact while the keys are further apart
# than the whole desktop round trip (the figterm2 driver paces them 250 ms apart), and silently
# wrong when typing outruns the webview — a PositionWindow for key N then lands in key N+1's window.
# docs/16-latency.md lists the log lines that would make the join exact.
#
# Hops, in the order the desktop goes through them (lines as of 42067ab3):
#   recv      fig_remote_ipc/src/remote.rs:118     "Received remote message" with EditBuffer(...)
#   emit      fig_desktop/src/remote_ipc/mod.rs:199 "Sending edit buffer change notification to webview"
#   loop      fig_desktop/src/webview/mod.rs:321    "Main loop event" UserEvent ... autocomplete ... Emit { event_name: Notification
#   changed   fig_desktop/src/webview/mod.rs:321    "Main loop event" UserEvent(PlatformBoundEvent(EditBufferChanged))
#   show      fig_desktop/src/webview/mod.rs:321    "Main loop event" UserEvent ... autocomplete ... window_event: Show }
#   position  fig_desktop/src/request/window.rs:30  "Position Window Request" (the webview answering)
#   batch     fig_desktop/src/webview/mod.rs:321    "Main loop event" UserEvent ... autocomplete ... Batch([UpdateWindowGeometry
#   geometry  fig_desktop/src/webview/window.rs:284 "updated window geometry" window_id=autocomplete
#   visible   (no log line: `set_visible` in webview/window.rs:439 is silent) -> always n=0
#
# `--rows` also prints one tab-separated row per keystroke (milliseconds after `recv`, `-` when a
# hop was not found) before the summary.

set -euo pipefail

usage() {
  sed -n '2,6p' "$0" | sed 's/^# \{0,1\}//' >&2
  exit 2
}

session=""
since=""
rows=0
log=""
while [ $# -gt 0 ]; do
  case "$1" in
    --session)
      [ $# -ge 2 ] || usage
      session="$2"
      shift 2
      ;;
    --since)
      [ $# -ge 2 ] || usage
      since="$2"
      shift 2
      ;;
    --rows)
      rows=1
      shift
      ;;
    -h | --help) usage ;;
    -*) usage ;;
    *)
      [ -z "$log" ] || usage
      log="$1"
      shift
      ;;
  esac
done
[ -n "$log" ] || usage
[ -r "$log" ] || {
  echo "cannot read $log" >&2
  exit 1
}

work="$(mktemp -d "${TMPDIR:-/tmp}/latency-trace.XXXXXX")"
trap 'rm -rf "$work"' EXIT

# `tracing_subscriber::fmt` colours its output even into the file. Lines from different threads
# reach the file a few microseconds out of timestamp order (the timestamp is taken before the
# line is queued to the writer thread), so sort on the timestamp; the ISO format sorts as text.
# Continuation lines of multi-line messages lose their place, but they carry no timestamp and
# are skipped anyway.
esc="$(printf '\033')"

sed "s/${esc}\[[0-9;]*m//g" "$log" | LC_ALL=C sort -s -k1,1 | LC_ALL=C awk -v session="$session" -v since="$since" \
  -v stats="$work/stats" -v rowfile="$work/rows" -v counts="$work/counts" '
# Days since 1970-01-01 for a proleptic Gregorian date (Howard Hinnant, days_from_civil).
function days(y, m, d,    era, yoe, doy, doe) {
  y -= (m <= 2)
  era = int((y >= 0 ? y : y - 399) / 400)
  yoe = y - era * 400
  doy = int((153 * (m + (m > 2 ? -3 : 9)) + 2) / 5) + d - 1
  doe = yoe * 365 + int(yoe / 4) - int(yoe / 100) + doy
  return era * 146097 + doe - 719468
}

# Seconds since the first timestamped line; small numbers keep microseconds exact in a double.
function stamp(line,    day, sod) {
  day = days(substr(line, 1, 4) + 0, substr(line, 6, 2) + 0, substr(line, 9, 2) + 0)
  sod = substr(line, 12, 2) * 3600 + substr(line, 15, 2) * 60 + substr(line, 18)
  if (day0 == "") day0 = day
  return (day - day0) * 86400 + sod
}

# The figterm session id inside the hook context (`session_id: Some("...")`), or "".
function session_of(line,    s) {
  if (!match(line, /session_id: Some\("[^"]*"\)/)) return ""
  s = substr(line, RSTART + 18, RLENGTH - 20)
  return s
}

function flush(    i, name, d) {
  if (!open) return
  keys++
  row = keys
  for (i = 1; i <= nhops; i++) {
    name = hop[i]
    if (name in t) {
      d = (t[name] - t["recv"]) * 1000
      if (name != "recv") printf "%02d %s\t%.3f\n", i, name, d > stats
    }
  }
  # Hop-to-hop deltas for the pieces that have their own owner.
  pair("emit", "loop", "10 emit->loop")
  pair("loop", "position", "11 loop->position")
  pair("position", "batch", "12 position->batch")
  pair("batch", "geometry", "13 batch->geometry")
  line = keys "\t\"" text "\""
  for (i = 2; i <= nhops; i++) {
    name = hop[i]
    line = line "\t" ((name in t) ? sprintf("%.3f", (t[name] - t["recv"]) * 1000) : "-")
  }
  print line > rowfile
  delete t
  open = 0
}

function pair(from, to, label) {
  if ((from in t) && (to in t)) printf "%s\t%.3f\n", label, (t[to] - t[from]) * 1000 > stats
}

BEGIN {
  nhops = split("recv emit loop changed show position batch geometry visible", hop, " ")
  open = 0
  keys = 0
  day0 = ""
}

!/^[0-9][0-9][0-9][0-9]-[0-9][0-9]-[0-9][0-9]T[0-9][0-9]:[0-9][0-9]:[0-9][0-9]\.[0-9]+Z/ { next }

{
  if (since != "" && substr($0, 1, length(since)) < since) next
  ts = stamp($0)
}

/Received remote message/ && /request: Some\(EditBuffer\(EditBufferHook/ {
  s = session_of($0)
  sessions[s] = 1
  if (session != "" && s != session) next
  flush()
  open = 1
  t["recv"] = ts
  # The hook text is the last `text: "..."` on the line (the context comes first).
  text = ""
  rest = $0
  while (match(rest, /text: "([^"\\]|\\.)*", cursor: /)) {
    text = substr(rest, RSTART + 7, RLENGTH - 7 - 11)
    rest = substr(rest, RSTART + RLENGTH)
  }
  next
}

!open { next }

/Sending edit buffer change notification to webview/ {
  if (!("emit" in t)) t["emit"] = ts
  next
}

/Main loop event/ && /UserEvent\(WindowEvent \{ window_id: WindowId\("autocomplete"\), window_event: Emit \{ event_name: Notification/ {
  if (("emit" in t) && !("loop" in t)) t["loop"] = ts
  next
}

/Main loop event/ && /UserEvent\(PlatformBoundEvent\(EditBufferChanged\)\)/ {
  if (!("changed" in t)) t["changed"] = ts
  next
}

/Main loop event/ && /UserEvent\(WindowEvent \{ window_id: WindowId\("autocomplete"\), window_event: Show \}\)/ {
  if (!("show" in t)) t["show"] = ts
  next
}

/Position Window Request/ && /window_id=autocomplete/ {
  if (!("position" in t)) t["position"] = ts
  next
}

/Main loop event/ && /UserEvent\(WindowEvent \{ window_id: WindowId\("autocomplete"\), window_event: Batch\(\[UpdateWindowGeometry/ {
  if (("position" in t) && !("batch" in t)) t["batch"] = ts
  next
}

/updated window geometry/ && /window_id=autocomplete/ {
  if (("batch" in t) && !("geometry" in t)) t["geometry"] = ts
  next
}

END {
  flush()
  n = 0
  for (s in sessions) n++
  printf "%d %d\n", keys, n > counts
  close(stats)
  close(rowfile)
}
'

touch "$work/stats" "$work/rows"
read -r keys sessions <"$work/counts"

if [ "$rows" -eq 1 ]; then
  printf 'key\ttext\temit\tloop\tchanged\tshow\tposition\tbatch\tgeometry\tvisible  (ms after recv)\n'
  cat "$work/rows"
  echo
fi

{
  echo "keystrokes (edit_buffer hooks): $keys; figterm sessions in log: $sessions${session:+ (filtered to $session)}"
  if [ "$sessions" -gt 1 ] && [ -z "$session" ]; then
    echo "warning: more than one session; the order-based join mixes them. Use --session." >&2
  fi
  if [ "$keys" -eq 0 ]; then
    echo "no edit_buffer hooks found; was the desktop running with FIG_LOG_LEVEL=trace?" >&2
    exit 1
  fi

  printf '\n%-22s %5s %10s %10s %10s\n' "hop (ms)" n median p90 max
  # Nearest-rank percentiles per hop; the numeric prefix keeps the pipeline order.
  LC_ALL=C sort -t "$(printf '\t')" -k1,1 -k2,2g "$work/stats" | awk -F '\t' '
    function out(    rank50, rank90) {
      if (cnt == 0) return
      rank50 = int((50 * cnt + 99) / 100); if (rank50 < 1) rank50 = 1
      rank90 = int((90 * cnt + 99) / 100); if (rank90 < 1) rank90 = 1
      label = index(name, "->") ? substr(name, 4) : "recv->" substr(name, 4)
      printf "%-22s %5d %10.3f %10.3f %10.3f\n", label, cnt, v[rank50], v[rank90], v[cnt]
    }
    $1 != name { out(); name = $1; cnt = 0; delete v }
    { v[++cnt] = $2 }
    END { out() }
  '
  echo "recv->visible              0          -          -          -   (no log line; see docs/16-latency.md)"
}
