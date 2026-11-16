export interface EnvironmentVariable {
  key: string;
  value?: string;
}

export interface ShellContext {
  /** Full snapshots replace environment and aliases, even when empty. */
  contextSnapshot?: boolean;
  inputRevision?: bigint;
  /** The current PID of the shell process */
  pid?: number;
  /** /dev/ttys## of terminal session */
  ttys?: string;
  /** the name of the process */
  processName?: string;
  /** the directory where the user ran the command */
  currentWorkingDirectory?: string;
  /** the value of $TERM_SESSION_ID */
  sessionId?: string;
  /** the version of the shell integration */
  integrationVersion?: number;
  /** the terminal the shell runs in */
  terminal?: string;
  /** the hostname of the computer the shell runs on */
  hostname?: string;
  /** path to the shell being used in the terminal */
  shellPath?: string;
  /** the environment variables of the shell, note that only exported variables are included */
  environmentVariables?: EnvironmentVariable[];
  /** the raw output of alias */
  alias?: string;
}
