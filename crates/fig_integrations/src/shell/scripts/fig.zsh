# {{PRODUCT_NAME}} zsh hook: zsh reports its own edit buffer over a
# unix socket. No process is spawned; disconnected shells keep editing while scheduled retries
# restore context and the current buffer. Wire format: crates/fig_shell_ipc/src/hook/wire.rs.
if [[ -o interactive && -z ${_fig_sock-} ]] && autoload -Uz is-at-least add-zle-hook-widget add-zsh-hook \
  && is-at-least {{ZSH_HOOK_MIN_VERSION}} && zmodload zsh/zle zsh/zleparameter zsh/parameter zsh/net/socket zsh/system zsh/datetime zsh/sched 2>/dev/null; then

typeset -g _fig_sock='{{HOOK_SOCKET}}' _fig_fd= _fig_off= _fig_armed= _fig_isearch= _fig_last= _fig_ctx=
typeset -gi _fig_revision=0 _fig_retry=1 _fig_timer=0 _fig_status=0
typeset -gF _fig_hello_deadline=0
typeset -g _fig_connect_fd= _fig_hello= _fig_in_command= _fig_rx=
typeset -g _fig_v2= _fig_ack_id= _fig_ack_status=
typeset -gA _fig_fwd _fig_orig _fig_names
_fig_names=($'\t' tab $'\e[Z' shift+tab $'\e[A' up $'\eOA' up $'\e[B' down $'\eOB' down $'\r' enter $'\e' esc
  $'\n' control+j $'\x0b' control+k $'\x0e' control+n $'\x10' control+p $'\x12' control+r)

# \\ \n \r \t and \s for space, both ways: no field ever contains a space or a line break.
_fig_esc() { REPLY=${${${${${1//\\/\\\\}//$'\n'/\\n}//$'\r'/\\r}//$'\t'/\\t}// /\\s} }
_fig_unesc() {
  emulate -L zsh; local s=$1 out= c
  while [[ $s == *\\* ]]; do
    out+=${s%%\\*} s=${s#*\\} c=${s:0:1} s=${s:1}
    case $c in (n) out+=$'\n';; (r) out+=$'\r';; (t) out+=$'\t';; (s) out+=' ';; (*) out+=$c;; esac
  done
  REPLY=$out$s
}

# A write after the desktop went away is EPIPE, and SIGPIPE would kill the shell. syswrite fails
# without a message, so no redirection (a third of the cost of a report) is needed.
_fig_send() {
  emulate -L zsh; setopt localtraps; trap '' PIPE
  [[ -n $_fig_fd ]] && syswrite -o $_fig_fd "$1"$'\n' && return 0
  _fig_close; return 1
}
# Only our exact timer entry is removed; sched indices shift when other plugins add events.
_fig_unschedule() {
  emulate -L zsh; local i
  for (( i=${#zsh_scheduled_events}; i>0; --i )); do
    [[ ${zsh_scheduled_events[i]} == *:_fig_tick ]] && sched -$i
  done
  _fig_timer=0
}
_fig_schedule() {
  [[ -z $_fig_off && -z $_fig_in_command && $_fig_timer == 0 ]] || return 0
  _fig_timer=1; sched +${1:-$_fig_retry} _fig_tick
}
_fig_close() {
  emulate -L zsh; local fd
  for fd in $_fig_fd $_fig_connect_fd; do
    zle -F $fd 2>/dev/null; exec {fd}>&- 2>/dev/null
  done
  _fig_fd= _fig_connect_fd= _fig_hello= _fig_rx= _fig_last= _fig_ctx= _fig_v2= _fig_ack_id= _fig_ack_status= _fig_fwd=()
  _fig_unschedule; _fig_schedule
}

# WELCOME is read by ZLE readiness, never by a one-second blocking read. A silent or partial
# greeting expires at the next scheduled tick; protocol refusal alone permanently disarms.
_fig_connect() {
  emulate -L zsh
  [[ -z $_fig_fd && -z $_fig_connect_fd && -z $_fig_off ]] || return 0
  if [[ ! -S $_fig_sock ]] || ! zsocket $_fig_sock 2>/dev/null; then
    _fig_schedule; return 0
  fi
  _fig_connect_fd=$REPLY
  local tty; _fig_esc "$TTY"; tty=$REPLY; _fig_esc "$USER@$HOST"
  setopt localtraps; trap '' PIPE
  if syswrite -o $_fig_connect_fd "HELLO {{HOOK_PROTOCOL_VERSION}} $$.$RANDOM zsh $$ $tty $REPLY"$'\n'; then
    _fig_hello_deadline=$(( EPOCHREALTIME + 1 ))
    zle -F -w $_fig_connect_fd _fig_on_hello
    _fig_unschedule; _fig_schedule 1
  else
    _fig_close
  fi
  return 0
}
_fig_tick() {
  _fig_timer=0
  # Scheduled callbacks have no editor parameters; enter a widget for the live snapshot.
  if zle; then zle _fig_reconnect; fi
  return 0
}
_fig_reconnect() {
  emulate -L zsh
  [[ -z $_fig_fd && -z $_fig_off ]] || return 0
  if [[ -n $_fig_connect_fd ]]; then
    if (( EPOCHREALTIME < _fig_hello_deadline )); then _fig_schedule 1; return 0; fi
    _fig_close; _fig_unschedule
  fi
  (( _fig_retry = _fig_retry < 8 ? _fig_retry * 2 : 8 ))
  _fig_connect
  return 0
}
_fig_on_hello() {
  emulate -L zsh; local chunk line result
  [[ $1 == $_fig_connect_fd ]] || return 0
  [[ -z ${2-} ]] || { _fig_close; return 0 }
  # Stop at the newline so commands coalesced with WELCOME stay on the socket.
  while [[ $_fig_hello != *$'\n' ]]; do
    sysread -i $1 -s 1 -t 0 chunk; result=$?
    (( result == 4 )) && return 0
    (( result == 0 )) || { _fig_close; return 0 }
    _fig_hello+=$chunk
    (( ${#_fig_hello} <= 4096 )) || { _fig_off=1; _fig_close; return 0 }
  done
  line=${_fig_hello%%$'\n'*}
  [[ $line == 'WELCOME 1 '* ]] || { _fig_off=1; _fig_close; return 0 }
  _fig_fd=$_fig_connect_fd; _fig_connect_fd= _fig_hello= _fig_retry=1
  _fig_v2=; [[ $line == *' edits-v2' ]] && _fig_v2=1
  _fig_unschedule; zle -F -w $_fig_fd _fig_on_msg
  _fig_arm; _fig_context
  _fig_esc "$PWD"; _fig_send "P $_fig_status $REPLY"
  _fig_report
  return 0
}

_fig_report() {
  emulate -L zsh
  (( PENDING || KEYS_QUEUED_COUNT )) || [[ -z $_fig_fd || -n $_fig_isearch ]] && return 0
  local left=$PREBUFFER$LBUFFER REPLY line
  [[ $KEYMAP == vicmd ]] && left+=${RBUFFER:0:1}  # vi command mode sits on a character, not after it
  _fig_esc "$PREBUFFER$BUFFER"; line="${#left} $REPLY"
  [[ $line == "$_fig_last" ]] && return 0
  _fig_last=$line; setopt nomultibyte
  if [[ -n $_fig_v2 ]]; then
    (( ++_fig_revision )); _fig_send "E2 $_fig_revision ${line%% *} ${#left} ${line#* }"
  else
    _fig_send "E ${line%% *} ${#left} ${line#* }"
  fi
  return 0
}
# During ^R $BUFFER is a history line, not what was typed: report an empty line until it ends.
_fig_isearch_on() { [[ -n $_fig_isearch ]] || { _fig_isearch=1 _fig_last=; _fig_send 'E 0 0 ' }; return 0 }
_fig_isearch_off() { _fig_isearch=; return 0 }

# One bounded frame reader owns all post-handshake input. Readiness means some bytes, not a
# complete line: leave partial frames buffered and return to ZLE instead of blocking typing.
# Return 0 with REPLY, 2 while incomplete, or 1 after retiring a closed/oversized connection.
_fig_read_line() {
  emulate -L zsh; setopt nomultibyte
  local chunk result; local -F end=$(( EPOCHREALTIME + ${1:-0} )) remaining
  while [[ -n $_fig_fd ]]; do
    if [[ $_fig_rx == *$'\n'* ]]; then
      REPLY=${_fig_rx%%$'\n'*}; _fig_rx=${_fig_rx#*$'\n'}
      (( ${#REPLY} <= 1048576 )) && return 0
      _fig_close; return 1
    fi
    (( ${#_fig_rx} <= 1048576 )) || { _fig_close; return 1 }
    (( remaining = end - EPOCHREALTIME )); (( remaining < 0 )) && remaining=0
    sysread -i $_fig_fd -s 4096 -t $remaining chunk; result=$?
    (( result == 4 )) && return 2
    (( result == 0 )) || { _fig_close; return 1 }
    _fig_rx+=$chunk
    # A busy peer cannot extend a key's deadline by sending unrelated or partial messages.
    [[ $_fig_rx == *$'\n'* ]] || (( EPOCHREALTIME < end )) || return 2
  done
  return 1
}

# A forwarded key has one total verdict deadline, followed by at most 80ms for KEY 1's
# insertion. No emulate here: the original widget must run under the user's options.
_fig_key() {
  zle -f nolast 2>/dev/null
  local name=${_fig_names[$KEYS]} line end left REPLY
  if [[ -n ${_fig_fwd[$name]-} && $LASTWIDGET != *menu-select* ]] && _fig_send "K $name"; then
    end=$(( EPOCHREALTIME + 1 ))
    while [[ -n $_fig_fd ]] && (( (left = end - EPOCHREALTIME) > 0 )) && _fig_read_line $left; do
      line=$REPLY
      case $line in ("KEY 2 $name") return 0;; ("KEY 0 $name") break;; ("KEY 1 $name") ;; (*) _fig_line "$line"; continue;; esac
      end=$(( EPOCHREALTIME + 0.08 ))
      while [[ -n $_fig_fd ]] && (( (left = end - EPOCHREALTIME) > 0 )) && _fig_read_line $left; do
        line=$REPLY; _fig_line "$line" accept || return 0
        [[ $line == INSERT* ]] || continue
        while [[ -n $_fig_fd ]] && _fig_read_line 0; do _fig_line "$REPLY" accept || return 0; done
        return 0
      done
      return 0
    done
  fi
  zle ${_fig_orig[${WIDGET#_fig_key_}$KEYS]:-undefined-key} -- "$@"
}

# One line from the desktop, inside zle. With $2, inside a key widget: an insertion may run the line
# (status 1 once it has).
_fig_line() {
  emulate -L zsh; local -a f=("${(@s: :)1}"); local REPLY n
  case $f[1] in
    (INSERT2)
      # Compare at the owner, including unreported edits/cursor movements. Never rebase stale work.
      if [[ $f[2] == $_fig_ack_id ]]; then _fig_send "ACK $f[2] $_fig_ack_status"; return 0; fi
      _fig_unesc "$f[5]"
      local left=$PREBUFFER$LBUFFER expected=$REPLY edit_status=0
      [[ $KEYMAP == vicmd ]] && left+=${RBUFFER:0:1}
      if (( EPOCHREALTIME * 1000 > ${f[10]:-0} )) || [[ $f[3] != $_fig_revision || $f[4] != ${#left} || $expected != $PREBUFFER$BUFFER ]]; then
        edit_status=1
      elif [[ -n $_fig_isearch || ${f[6]} != <-> ]] || (( f[6] > $#LBUFFER )); then
        edit_status=2
      else
        n=$f[6]; (( n )) && LBUFFER[-n,-1]=
        _fig_unesc "$f[7]"; LBUFFER+=$REPLY; (( CURSOR -= ${f[8]:-0} )); _fig_report; zle -R
      fi
      _fig_ack_id=$f[2] _fig_ack_status=$edit_status; _fig_send "ACK $f[2] $edit_status"
      [[ $edit_status == 0 && ${f[9]-} == 1 && -n ${2-} ]] && { zle accept-line; return 1 } ;;
    (INSERT) n=$f[2]; (( n > $#LBUFFER )) && n=$#LBUFFER; (( n )) && LBUFFER[-n,-1]=
      _fig_unesc "$f[3]"; LBUFFER+=$REPLY; (( CURSOR -= ${f[4]:-0} )); _fig_report; zle -R
      [[ ${f[5]-} == 1 && -n ${2-} ]] && { zle accept-line; return 1 } ;;
    (INTERCEPT) _fig_fwd=(); (( f[2] )) && for n in $f[3,-1]; do _fig_fwd[$n]=1; done ;;
    (BYE) _fig_off=1; _fig_close ;;
  esac
  return 0
}
_fig_on_msg() {
  emulate -L zsh; local REPLY
  [[ $1 == $_fig_fd ]] || return 0
  [[ -z ${2-} ]] || { _fig_close; return 0 }
  while [[ -n $_fig_fd ]] && _fig_read_line 0; do _fig_line "$REPLY"; done
  return 0
}

# Every prompt: put back a hook a plugin replaced with `zle -N zle-line-pre-redraw …` (N9). A free
# pre-redraw slot is taken directly: add-zle-hook-widget's dispatcher costs more per key than the report
# (and it keeps working when a plugin adds its own later). Once: bind the keys, remembering each key's
# widget (read from a file, not a subshell); string bindings and unbound keys but Esc stay as they are.
_fig_arm() {
  emulate -L zsh; local h fn km seq line tmp=$_fig_sock:h/keys.$$
  for h fn in line-init _fig_report line-pre-redraw _fig_report isearch-update _fig_isearch_on isearch-exit _fig_isearch_off; do
    zstyle -a zle-$h widgets line
    [[ $widgets[zle-$h] == user:$fn || ( $widgets[zle-$h] == user:azhw:zle-$h && -n ${(M)line:#<->:(user:|)$fn} ) ]] && continue
    if [[ $h == line-pre-redraw && -z $widgets[zle-$h] ]]; then zle -N zle-$h $fn; continue; fi
    add-zle-hook-widget -d $h $fn; add-zle-hook-widget $h $fn
  done
  [[ -d $_fig_sock:h && -z $_fig_armed ]] && _fig_armed=1 || return 0
  { for km in emacs viins vicmd; do for seq in ${(k)_fig_names}; do bindkey -M $km $seq; done; done } >| $tmp 2>/dev/null
  for km in emacs viins vicmd; do for seq in ${(k)_fig_names}; do
    read -r line || break; line=${line##* }
    [[ $line == \"* || $line == _fig_key_* || ( $line == undefined-key && $seq != $'\e' ) ]] && continue
    _fig_orig[$km$seq]=$line; zle -N _fig_key_$km _fig_key; bindkey -M $km $seq _fig_key_$km
  done; done < $tmp
  zmodload -F zsh/files b:zf_rm 2>/dev/null && zf_rm -f $tmp
}

_fig_context() {
  emulate -L zsh; local REPLY line k; local -a env al
  for k in ${(k)parameters[(R)*export*]}; do env+=("$k=${(P)k}"); done
  for k in ${(k)aliases}; do al+=("$k=$aliases[$k]"); done
  line="${(pj:\0:)env}"$'\n'"${(pj:\n:)al}"
  if [[ -n $_fig_fd && $line != "$_fig_ctx" ]]; then
    _fig_ctx=$line; _fig_esc "${(pj:\0:)env}"; line=$REPLY; _fig_esc "${(pj:\n:)al}"; _fig_send "C $line $REPLY"
  fi
}
_fig_precmd() {
  local st=$?; emulate -L zsh; local REPLY line
  _fig_status=$st _fig_in_command= _fig_isearch= _fig_last=
  _fig_arm
  if [[ -z $_fig_fd ]]; then _fig_unschedule; _fig_connect; return 0; fi
  # Commands arriving during execution cannot insert into a later prompt.
  while [[ -n $_fig_fd ]] && _fig_read_line 0; do line=$REPLY; [[ $line == INSERT* ]] || _fig_line "$line"; done
  _fig_context
  _fig_esc "$PWD"; _fig_send "P $st $REPLY"; return 0
}
_fig_preexec() {
  emulate -L zsh; (( ++_fig_revision )); _fig_isearch= _fig_last= _fig_in_command=1
  _fig_unschedule
  [[ -z $_fig_connect_fd ]] || _fig_close
  local REPLY; _fig_esc "$1"; _fig_send "X $REPLY"; return 0
}

zle -N _fig_on_hello
zle -N _fig_reconnect
zle -N _fig_on_msg
add-zsh-hook precmd _fig_precmd
add-zsh-hook preexec _fig_preexec
fi
