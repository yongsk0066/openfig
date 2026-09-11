#!/usr/bin/env bash

mkdir -p "${HOME}/.local/bin" > /dev/null 2>&1

# add ~/.local/bin to PATH
if [[ -d "${HOME}/.local/bin" ]] && [[ ":$PATH:" != *":${HOME}/.local/bin:"* ]]; then
  PATH="${PATH:+"$PATH:"}${HOME}/.local/bin"
fi

if [[ -n "${FIG_NEW_SESSION:-}" ]]; then
  unset FIGTERM_SESSION_ID
  unset FIG_TERM
  unset FIG_NEW_SESSION
fi

# 0 = Yes, 1 = No, 2 = Fallback to FIG_TERM
if [ -z "${SHOULD_FIGTERM_LAUNCH:-}" ]; then
  {{CLI_BINARY_NAME}} _ should-figterm-launch 1>/dev/null 2>&1
  SHOULD_FIGTERM_LAUNCH=$?
fi

# Only launch figterm if current session is not already inside PTY and command exists.
# PWSH var is set when launched by `pwsh -Login`, in which case we don't want to init.
# It is not necessary in Fish.
if   [[ -t 1 ]] \
  && [[ -z "${PROCESS_LAUNCHED_BY_FIG:-}" ]] \
  && command -v {{PTY_BINARY_NAME}} 1>/dev/null 2>&1 \
  && [[ ("${SHOULD_FIGTERM_LAUNCH}" -eq 0) || (("${SHOULD_FIGTERM_LAUNCH}" -eq 2) && (-z "${FIG_TERM:-}" || (-z "${FIG_TERM_TMUX:-}" && -n "${TMUX:-}"))) ]]
then
  # Pty module sets FIG_TERM or FIG_TERM_TMUX to avoid running twice.
  if [ -z "${FIG_SHELL:-}" ]; then
    FIG_SHELL=$({{CLI_BINARY_NAME}} _ get-shell)
  fi
  FIG_IS_LOGIN_SHELL="${FIG_IS_LOGIN_SHELL:='0'}"

  # shellcheck disable=SC2030
  if ([[ -n "${BASH:-}" ]] && shopt -q login_shell) \
    || [[ -n "${ZSH_NAME:-}" && -o login ]]; then
    FIG_IS_LOGIN_SHELL=1
  fi

  # Do not launch figterm in non-interactive shells (like VSCode Tasks)
  if [[ $- == *i* ]]; then
    FIG_TERM_NAME="$(basename "${FIG_SHELL}") ({{PTY_BINARY_NAME}})"
    if [[ -z "${FIG_TERM_PATH:-}" ]]; then
      if [[ -x "${HOME}/.local/bin/${FIG_TERM_NAME}" ]]; then
        FIG_TERM_PATH="${HOME}/.local/bin/${FIG_TERM_NAME}"
      else
        FIG_TERM_PATH="$(command -v {{PTY_BINARY_NAME}} || echo "${HOME}/.local/bin/{{PTY_BINARY_NAME}}")"
      fi
    fi

    FIG_EXECUTION_STRING="${BASH_EXECUTION_STRING:=$ZSH_EXECUTION_STRING}"

    # Get initial text.
    INITIAL_TEXT=""
    # shellcheck disable=SC2031
    if [[ -z "${BASH:-}" || "${BASH_VERSINFO[0]}" -gt "3" ]]; then
      while read -rt 0; do
        if [[ -n "${BASH:-}" ]]; then
          read -r
        fi
        INITIAL_TEXT="${INITIAL_TEXT}${REPLY}\n"
      done
    fi
    FIG_EXECUTION_STRING="${FIG_EXECUTION_STRING}" FIG_START_TEXT="$(printf "%b" "${INITIAL_TEXT}")" FIG_SHELL="${FIG_SHELL}" FIG_IS_LOGIN_SHELL="${FIG_IS_LOGIN_SHELL}" exec -a "${FIG_TERM_NAME}" "${FIG_TERM_PATH}"
  fi
fi
