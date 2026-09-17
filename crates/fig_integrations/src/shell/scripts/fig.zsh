# {{PRODUCT_NAME}} zsh hook (docs/15-architecture-study.md §3.3): zsh reports its own edit buffer over a
# unix socket instead of a pty wrapper reading it off the screen. No process is spawned, every hook
# returns 0, and a socket failure disarms the hook, leaving a plain shell. Wire format and the reasons
# behind it: crates/fig_shell_ipc/src/hook/wire.rs.
if [[ -o interactive && -z ${_fig_sock-} ]] && autoload -Uz is-at-least add-zle-hook-widget add-zsh-hook \
  && is-at-least {{ZSH_HOOK_MIN_VERSION}} && zmodload zsh/zle zsh/zleparameter zsh/parameter zsh/net/socket zsh/system zsh/datetime 2>/dev/null; then

typeset -g _fig_sock='{{HOOK_SOCKET}}' _fig_fd= _fig_off= _fig_armed= _fig_isearch= _fig_last= _fig_ctx=
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
_fig_close() {
  emulate -L zsh; [[ -n $_fig_fd ]] || return 0
  zle -F $_fig_fd 2>/dev/null; exec {_fig_fd}>&- 2>/dev/null
  _fig_fd= _fig_last= _fig_ctx= _fig_fwd=()
}

# No socket: try again at the next prompt. Anything but WELCOME 1 within a second: never again.
_fig_connect() {
  emulate -L zsh
  [[ -z $_fig_fd && -z $_fig_off && -S $_fig_sock ]] && zsocket $_fig_sock 2>/dev/null || return 1
  local fd=$REPLY line tty; _fig_esc "$TTY"; tty=$REPLY; _fig_esc "$USER@$HOST"; setopt localtraps; trap '' PIPE
  if syswrite -o $fd "HELLO {{HOOK_PROTOCOL_VERSION}} $$.$RANDOM zsh $$ $tty $REPLY"$'\n' \
    && read -t 1 -r -u $fd line && [[ $line == 'WELCOME 1 '* ]]; then
    _fig_fd=$fd; zle -F -w $fd _fig_on_msg; return 0
  fi
  _fig_off=1; exec {fd}>&- 2>/dev/null; return 1
}

_fig_report() {
  emulate -L zsh
  (( PENDING || KEYS_QUEUED_COUNT )) || [[ -z $_fig_fd || -n $_fig_isearch ]] && return 0
  local left=$PREBUFFER$LBUFFER REPLY line
  [[ $KEYMAP == vicmd ]] && left+=${RBUFFER:0:1}  # vi command mode sits on a character, not after it
  _fig_esc "$PREBUFFER$BUFFER"; line="${#left} $REPLY"
  [[ $line == "$_fig_last" ]] && return 0
  _fig_last=$line; setopt nomultibyte
  _fig_send "E ${line%% *} ${#left} ${line#* }"; return 0
}
# During ^R $BUFFER is a history line, not what was typed: report an empty line until it ends.
_fig_isearch_on() { [[ -n $_fig_isearch ]] || { _fig_isearch=1 _fig_last=; _fig_send 'E 0 0 ' }; return 0 }
_fig_isearch_off() { _fig_isearch=; return 0 }

# A forwarded key waits for the desktop's verdict: KEY 0 zsh's after all, KEY 2 taken, KEY 1 taken and an
# insertion may follow. zsh runs no widget from a `zle -F` handler until the next key arrives, so a line
# an insertion ends with can only be run from here: wait up to 80 ms for it. No `emulate`: the original
# widget runs under the user's options.
_fig_key() {
  zle -f nolast 2>/dev/null
  local name=${_fig_names[$KEYS]} line end left
  if [[ -n ${_fig_fwd[$name]-} && $LASTWIDGET != *menu-select* ]] && _fig_send "K $name"; then
    while [[ -n $_fig_fd ]] && read -t 1 -r -u $_fig_fd line; do
      case $line in ("KEY 2 $name") return 0;; ("KEY 0 $name") break;; ("KEY 1 $name") ;; (*) _fig_line "$line"; continue;; esac
      end=$(( EPOCHREALTIME + 0.08 ))
      while [[ -n $_fig_fd ]] && (( (left = end - EPOCHREALTIME) > 0 )) && read -t $left -r -u $_fig_fd line; do
        _fig_line "$line" accept || return 0
        [[ $line == INSERT* ]] || continue
        while [[ -n $_fig_fd ]] && read -t 0 -r -u $_fig_fd line; do _fig_line "$line" accept || return 0; done
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
    (INSERT) n=$f[2]; (( n > $#LBUFFER )) && n=$#LBUFFER; (( n )) && LBUFFER[-n,-1]=
      _fig_unesc "$f[3]"; LBUFFER+=$REPLY; (( CURSOR -= ${f[4]:-0} )); _fig_report; zle -R
      [[ ${f[5]-} == 1 && -n ${2-} ]] && { zle accept-line; return 1 } ;;
    (INTERCEPT) _fig_fwd=(); (( f[2] )) && for n in $f[3,-1]; do _fig_fwd[$n]=1; done ;;
    (BYE) _fig_off=1; _fig_close ;;
  esac
  return 0
}
_fig_on_msg() {
  emulate -L zsh; local line
  [[ -z ${2-} ]] && read -r -u $1 line || { _fig_close; return 0 }
  _fig_line "$line"
  while [[ -n $_fig_fd ]] && read -t 0 -r -u $1 line; do _fig_line "$line"; done
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
  [[ -z $_fig_armed ]] && _fig_armed=1 || return 0
  { for km in emacs viins vicmd; do for seq in ${(k)_fig_names}; do bindkey -M $km $seq; done; done } >| $tmp 2>/dev/null
  for km in emacs viins vicmd; do for seq in ${(k)_fig_names}; do
    read -r line || break; line=${line##* }
    [[ $line == \"* || $line == _fig_key_* || ( $line == undefined-key && $seq != $'\e' ) ]] && continue
    _fig_orig[$km$seq]=$line; zle -N _fig_key_$km _fig_key; bindkey -M $km $seq _fig_key_$km
  done; done < $tmp
  zmodload -F zsh/files b:zf_rm 2>/dev/null && zf_rm -f $tmp
}

_fig_precmd() {
  local st=$?; emulate -L zsh; local REPLY line k; local -a env al
  _fig_isearch= _fig_last=
  [[ -n $_fig_fd ]] || _fig_connect || return 0
  _fig_arm
  # Lines that came while a command ran: keep interception, drop insertions (no zle to apply them).
  while [[ -n $_fig_fd ]] && read -t 0 -r -u $_fig_fd line; do [[ $line == INSERT* ]] || _fig_line "$line"; done
  for k in ${(k)parameters[(R)*export*]}; do env+=("$k=${(P)k}"); done
  for k in ${(k)aliases}; do al+=("$k=$aliases[$k]"); done
  line="${(pj:\0:)env}"$'\n'"${(pj:\n:)al}"
  if [[ -n $_fig_fd && $line != "$_fig_ctx" ]]; then
    _fig_ctx=$line; _fig_esc "${(pj:\0:)env}"; line=$REPLY; _fig_esc "${(pj:\n:)al}"; _fig_send "C $line $REPLY"
  fi
  _fig_esc "$PWD"; _fig_send "P $st $REPLY"; return 0
}
_fig_preexec() { emulate -L zsh; _fig_isearch= _fig_last=; local REPLY; _fig_esc "$1"; _fig_send "X $REPLY"; return 0 }

zle -N _fig_on_msg
add-zsh-hook precmd _fig_precmd
add-zsh-hook preexec _fig_preexec
fi
