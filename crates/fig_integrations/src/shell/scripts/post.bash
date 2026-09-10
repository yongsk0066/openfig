if [[ -n "${BASH:-}" ]]; then

# add ~/.local/bin to PATH
if [[ -d "${HOME}/.local/bin" ]] && [[ ":$PATH:" != *":${HOME}/.local/bin:"* ]]; then
  PATH="${PATH:+"$PATH:"}${HOME}/.local/bin"
fi

if [[ -z "${TTY:-}" ]]; then
  TTY=$(tty)
fi
export TTY

export SHELL_PID="$$"

Q_LAST_PS1="$PS1"
Q_LAST_PS2="$PS2"
Q_LAST_PS3="$PS3"

if [[ -z "${Q_SHELL:-}" ]]; then
  Q_SHELL=$({{CLI_BINARY_NAME}} _ get-shell)
fi

# Construct Operating System Command.
function __fig_preexec() {
  # One write instead of two.
  # shellcheck disable=SC2059
  printf '\033]697;OSCLock=%s\007\033]697;PreExec\007' "${QTERM_SESSION_ID}"

  # Reset user prompts before executing a command, but only if it hasn't
  # changed since we last set it.
  if [[ -n "${Q_USER_PS1+x}" && "${PS1}" = "${Q_LAST_PS1}" ]]; then
    Q_LAST_PS1="${Q_USER_PS1}"
    PS1="${Q_USER_PS1}"
  fi
  if [[ -n "${Q_USER_PS2+x}" && "${PS2}" = "${Q_LAST_PS2}" ]]; then
    Q_LAST_PS2="${Q_USER_PS2}"
    PS2="${Q_USER_PS2}"
  fi
  if [[ -n "${Q_USER_PS3+x}" && "${PS3}" = "${Q_LAST_PS3}" ]]; then
    Q_LAST_PS3="${Q_USER_PS3}"
    PS3="${Q_USER_PS3}"
  fi

  _fig_done_preexec="yes"
}

function __fig_preexec_preserve_status() {
  __fig_ret_value="$?"
  __fig_preexec "$@"
  __bp_set_ret_value "${__fig_ret_value}" "${__bp_last_argument_prev_command:?}"
}

function __fig_pre_prompt () {
  __fig_ret_value="$?"

  # One `printf` for the whole prompt report; this used to be nine separate `fig_osc` calls, i.e.
  # nine writes into the pty on every prompt. `WSLDistro` never fires on macOS and nothing parses
  # it, so the key is gone (its guard also read `${WSL_DISTRO_NAME}` unguarded, which is an error
  # under `set -u`).
  # shellcheck disable=SC2059
  printf '\033]697;OSCUnlock=%s\007\033]697;Dir=%s\007\033]697;Shell=bash\007\033]697;ShellPath=%s\007\033]697;PID=%d\007\033]697;ExitCode=%s\007\033]697;TTY=%s\007\033]697;Log=%s\007\033]697;User=%s\007' \
    "${QTERM_SESSION_ID}" \
    "${PWD}" \
    "${Q_SHELL:-$SHELL}" \
    "$$" \
    "${__fig_ret_value}" \
    "${TTY}" \
    "${Q_LOG_LEVEL}" \
    "${USER:-root}"

  if command -v {{CLI_BINARY_NAME}} >/dev/null 2>&1; then
    (command {{CLI_BINARY_NAME}} _ pre-cmd --alias "$(\alias)" > /dev/null 2>&1 &) >/dev/null 2>&1
  fi

  # Work around bug in CentOS 7.2 where preexec doesn't run if you press ^C
  # while entering a command.
  [[ -z "${_fig_done_preexec:-}" ]] && __fig_preexec ""
  _fig_done_preexec=""

  # Reset $?
  __bp_set_ret_value "${__fig_ret_value}" "${__bp_last_argument_prev_command}"
}

function __fig_post_prompt () {
  __fig_ret_value="$?"

  __fig_reset_hooks

  # If Q_USER_PSx is undefined or PSx changed by user, update Q_USER_PSx.
  if [[ -z "${Q_USER_PS1+x}" || "${PS1}" != "${Q_LAST_PS1}" ]]; then
    Q_USER_PS1="${PS1}"
  fi
  if [[ -z "${Q_USER_PS2+x}" || "${PS2}" != "${Q_LAST_PS2}" ]]; then
    Q_USER_PS2="${PS2}"
  fi
  if [[ -z "${Q_USER_PS3+x}" || "${PS3}" != "${Q_LAST_PS3}" ]]; then
    Q_USER_PS3="${PS3}"
  fi

  # `$'…'` rather than `$(fig_osc …)`: this ran on every prompt and each command substitution is
  # a fork. Same bytes, three fewer processes per prompt. (post.zsh has always done it this way.)
  START_PROMPT=$'\\[\033]697;StartPrompt\007\\]'
  END_PROMPT=$'\\[\033]697;EndPrompt\007\\]'
  NEW_CMD=$'\\[\033]697;NewCmd='"${QTERM_SESSION_ID}"$'\007\\]'

  # Reset $? first in case it's used in $Q_USER_PSx.
  __bp_set_ret_value "${__fig_ret_value}" "${__bp_last_argument_prev_command}"
  PS1="${START_PROMPT}${Q_USER_PS1}${END_PROMPT}${NEW_CMD}"
  PS2="${START_PROMPT}${Q_USER_PS2}${END_PROMPT}"
  PS3="${START_PROMPT}${Q_USER_PS3}${END_PROMPT}${NEW_CMD}"

  Q_LAST_PS1="${PS1}"
  Q_LAST_PS2="${PS2}"
  Q_LAST_PS3="${PS3}"
}

__fig_reset_hooks() {
  # Rely on PROMPT_COMMAND instead of precmd_functions because precmd_functions
  # are all run before PROMPT_COMMAND.
  # Set PROMPT_COMMAND to "[
  #   __fig_pre_prompt,
  #   ...precmd_functions,
  #   ORIGINAL_PROMPT_COMMAND,
  #   __fig_post_prompt,
  #   __bp_interactive_mode
  # ]"
  # bash >= 5.1 lets `PROMPT_COMMAND` be an array, and reading it as a scalar sees element 0 only.
  # Everything from element 1 on — which is exactly where bash-preexec puts `__bp_interactive_mode`
  # — was therefore invisible here: not stripped, not re-added, and left running behind the string
  # this function assembles into element 0. `[*]` with `IFS=$'\n'` is the idiom bash-preexec uses
  # itself (`__bp_in_prompt_command`), and newline is the separator used below.
  local existing_prompt_command IFS=$'\n'
  existing_prompt_command="${PROMPT_COMMAND[*]}"
  existing_prompt_command="${existing_prompt_command//__fig_post_prompt[;$'\n']}"
  existing_prompt_command="${existing_prompt_command//__fig_post_prompt}"
  existing_prompt_command="${existing_prompt_command//__bp_interactive_mode[;$'\n']}"
  existing_prompt_command="${existing_prompt_command//__bp_interactive_mode}"
  __bp_sanitize_string existing_prompt_command "$existing_prompt_command"

  # `PROMPT_COMMAND=""` on an array variable only assigns element 0, so the elements collected
  # above would run a second time. Drop the variable first.
  unset -v PROMPT_COMMAND
  # shellcheck disable=SC2178
  PROMPT_COMMAND=""
  if [[ -n "${existing_prompt_command:-}" ]]; then
        # shellcheck disable=SC2179
      PROMPT_COMMAND+=${existing_prompt_command}$'\n'
  fi;
  # shellcheck disable=SC2179
  PROMPT_COMMAND+=$'__fig_post_prompt\n'
  # shellcheck disable=SC2179
  PROMPT_COMMAND+='__bp_interactive_mode'

  if [[ ${precmd_functions[0]} != __fig_pre_prompt ]]; then
    for index in "${!precmd_functions[@]}"; do
      if [[ ${precmd_functions[$index]} == __fig_pre_prompt ]]; then
        unset -v 'precmd_functions[$index]'
      fi
    done
    precmd_functions=(__fig_pre_prompt "${precmd_functions[@]}")
  fi

  if [[ ${preexec_functions[0]} != __fig_preexec_preserve_status ]]; then
    for index in "${!preexec_functions[@]}"; do
      if [[ ${preexec_functions[$index]} == __fig_preexec_preserve_status ]]; then
        unset -v 'preexec_functions[$index]'
      fi
    done
    preexec_functions=(__fig_preexec_preserve_status "${preexec_functions[@]}")
  fi
}

# Ensure that bash-preexec is installed
# even if the user overrides COMMAND_PROMPT
# https://github.com/withfig/fig/issues/888
#
# We also need to ensure Warp is not running
# since they expect any plugins to not include
# it again
if [[ "${TERM_PROGRAM}" != "WarpTerminal" ]]; then
  __bp_install_after_session_init
fi
__fig_reset_hooks

fi

(command {{CLI_BINARY_NAME}} _ pre-cmd --alias "$(\alias)" > /dev/null 2>&1 &) >/dev/null 2>&1
