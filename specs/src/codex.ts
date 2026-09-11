import { filepaths, folders } from "@fig/spec-generators";

/** `JSON.parse` that yields `undefined` instead of throwing, so a generator can fall back to `[]`. */
const parseJson = (raw: string): unknown => {
  try {
    return JSON.parse(raw) as unknown;
  } catch (_err) {
    return undefined;
  }
};

const isRecord = (value: unknown): value is Record<string, unknown> =>
  typeof value === "object" && value !== null && !Array.isArray(value);

const string = (value: unknown): string | undefined =>
  typeof value === "string" && value.length > 0 ? value : undefined;

/** Drops blanks and duplicates, which every generator below can produce. */
const uniqueSuggestions = (suggestions: Fig.Suggestion[]): Fig.Suggestion[] => {
  const seen = new Set<string>();
  const result: Fig.Suggestion[] = [];
  for (const suggestion of suggestions) {
    const name = typeof suggestion.name === "string" ? suggestion.name : undefined;
    if (!name || seen.has(name)) {
      continue;
    }
    seen.add(name);
    result.push(suggestion);
  }
  return result;
};

/**
 * Saved sessions for `resume`, `fork`, `archive`, `unarchive` and `delete`.
 *
 * Codex keeps one line per session in `$CODEX_HOME/session_index.jsonl`
 * (`{"id","thread_name","updated_at"}`), newest last — cheaper and more informative than
 * walking the `sessions/<year>/<month>/<day>/rollout-*.jsonl` tree, which only carries ids
 * in its file names.
 */
const sessionGenerator: Fig.Generator = {
  script: [
    "bash",
    "-c",
    `tail -n 60 "${"$"}{CODEX_HOME:-$HOME/.codex}/session_index.jsonl" 2>/dev/null`,
  ],
  postProcess: (out) =>
    uniqueSuggestions(
      out
        .split("\n")
        .toReversed()
        .flatMap((line): Fig.Suggestion[] => {
          const entry = parseJson(line);
          if (!isRecord(entry)) {
            return [];
          }
          const id = string(entry.id);
          if (!id) {
            return [];
          }
          return [
            {
              name: id,
              description: string(entry.thread_name) ?? "Saved session",
              icon: "fig://icon?type=commit",
            },
          ];
        }),
    ),
  cache: { ttl: 5000 },
};

/**
 * Reads the `[<section>.<name>]` table headers out of `$CODEX_HOME/config.toml`.
 *
 * Only the first component after the section name is kept, so `[mcp_servers.foo.env]`
 * still yields `foo`, and quoted names such as `[plugins."x@y"]` keep their inner text.
 */
const tomlSectionGenerator = (
  section: string,
  description: string,
  icon: string,
): Fig.Generator => ({
  script: [
    "bash",
    "-c",
    `grep -E '^[[:space:]]*\\[${section}\\.' "${"$"}{CODEX_HOME:-$HOME/.codex}/config.toml" 2>/dev/null`,
  ],
  postProcess: (out) =>
    uniqueSuggestions(
      out.split("\n").flatMap((line): Fig.Suggestion[] => {
        const match = line.trim().match(/^\[[^.\]]+\.(.+)]$/);
        if (!match) {
          return [];
        }
        const rest = match[1];
        const name = rest.startsWith('"')
          ? (rest.match(/^"([^"]*)"/)?.[1] ?? "")
          : rest.split(".")[0];
        return name ? [{ name, description, icon }] : [];
      }),
    ),
  cache: { ttl: 10_000 },
});

const mcpServerGenerator = tomlSectionGenerator(
  "mcp_servers",
  "Configured MCP server",
  "fig://icon?type=box",
);

const profileGenerator = tomlSectionGenerator(
  "profiles",
  "Config profile",
  "fig://icon?type=gear",
);

const marketplaceGenerator = tomlSectionGenerator(
  "marketplaces",
  "Configured plugin marketplace",
  "fig://icon?type=box",
);

const pluginGenerator = tomlSectionGenerator(
  "plugins",
  "Configured plugin",
  "fig://icon?type=box",
);

/**
 * Model slugs from `$CODEX_HOME/models_cache.json`, the catalog Codex already refreshed for
 * itself — so nothing here touches the network. Falls back to the slugs the CLI docs name.
 */
const modelGenerator: Fig.Generator = {
  script: ["bash", "-c", `cat "${"$"}{CODEX_HOME:-$HOME/.codex}/models_cache.json" 2>/dev/null`],
  postProcess: (out) => {
    const parsed = parseJson(out);
    const models = isRecord(parsed) && Array.isArray(parsed.models) ? parsed.models : [];
    return uniqueSuggestions(
      models.flatMap((model): Fig.Suggestion[] => {
        if (!isRecord(model)) {
          return [];
        }
        const slug = string(model.slug);
        if (!slug) {
          return [];
        }
        return [
          {
            name: slug,
            description: string(model.description) ?? string(model.display_name) ?? "Model",
            icon: "fig://icon?type=asterisk",
          },
        ];
      }),
    );
  },
  cache: { ttl: 60_000 },
};

const helpOption: Fig.Option = {
  name: ["-h", "--help"],
  description: "Print help (see a summary with '-h')",
};

const versionOption: Fig.Option = {
  name: ["-V", "--version"],
  description: "Print version",
};

const configOption: Fig.Option = {
  name: ["-c", "--config"],
  description: "Override a configuration value that would come from ~/.codex/config.toml",
  isRepeatable: true,
  args: { name: "key=value" },
};

const enableOption: Fig.Option = {
  name: "--enable",
  description: "Enable a feature. Equivalent to -c features.<name>=true",
  isRepeatable: true,
  args: { name: "FEATURE" },
};

const disableOption: Fig.Option = {
  name: "--disable",
  description: "Disable a feature. Equivalent to -c features.<name>=false",
  isRepeatable: true,
  args: { name: "FEATURE" },
};

/** `-c`, `--enable` and `--disable`, which clap puts on every single subcommand. */
const commonOptions: Fig.Option[] = [configOption, enableOption, disableOption];

const strictConfigOption: Fig.Option = {
  name: "--strict-config",
  description: "Error out when config.toml has fields this version of Codex does not recognize",
};

const imageOption: Fig.Option = {
  name: ["-i", "--image"],
  description: "Optional image(s) to attach to the initial prompt",
  args: { name: "FILE", isVariadic: true, generators: filepaths({ extensions: ["png", "jpg", "jpeg", "gif", "webp"] }) },
};

const modelOption: Fig.Option = {
  name: ["-m", "--model"],
  description: "Model the agent should use",
  args: { name: "MODEL", generators: modelGenerator },
};

const ossOption: Fig.Option = {
  name: "--oss",
  description: "Use open-source provider",
};

const localProviderOption: Fig.Option = {
  name: "--local-provider",
  description: "Which local provider to use with --oss",
  args: { name: "OSS_PROVIDER", suggestions: ["lmstudio", "ollama"] },
};

const profileOption: Fig.Option = {
  name: ["-p", "--profile"],
  description: "Layer $CODEX_HOME/<name>.config.toml on top of the base user config",
  args: { name: "CONFIG_PROFILE", generators: profileGenerator },
};

const sandboxOption: Fig.Option = {
  name: ["-s", "--sandbox"],
  description: "Sandbox policy for model-generated shell commands",
  args: {
    name: "SANDBOX_MODE",
    suggestions: [
      { name: "read-only", description: "No writes, no network" },
      { name: "workspace-write", description: "Write inside the workspace" },
      { name: "danger-full-access", description: "No sandbox at all" },
    ],
  },
};

const bypassOption: Fig.Option = {
  name: "--dangerously-bypass-approvals-and-sandbox",
  description: "Skip all confirmation prompts and run commands without sandboxing",
  isDangerous: true,
  exclusiveOn: ["--yolo"],
};

/** Hidden in `--help`, but the binary still takes it as the short spelling of the bypass flag. */
const yoloOption: Fig.Option = {
  name: "--yolo",
  description: "Alias for --dangerously-bypass-approvals-and-sandbox",
  isDangerous: true,
  exclusiveOn: ["--dangerously-bypass-approvals-and-sandbox"],
};

const bypassHookTrustOption: Fig.Option = {
  name: "--dangerously-bypass-hook-trust",
  description: "Run enabled hooks without requiring persisted hook trust for this invocation",
  isDangerous: true,
};

const cdOption: Fig.Option = {
  name: ["-C", "--cd"],
  description: "Tell the agent to use the specified directory as its working root",
  args: { name: "DIR", generators: folders },
};

const addDirOption: Fig.Option = {
  name: "--add-dir",
  description: "Additional directories that should be writable alongside the primary workspace",
  isRepeatable: true,
  args: { name: "DIR", generators: folders },
};

const askForApprovalOption: Fig.Option = {
  name: ["-a", "--ask-for-approval"],
  description: "When the model requires human approval before executing a command",
  args: {
    name: "APPROVAL_POLICY",
    suggestions: [
      { name: "untrusted", description: 'Only run "trusted" commands without asking' },
      { name: "on-request", description: "The model decides when to ask" },
      { name: "never", description: "Never ask; failures go straight back to the model" },
    ],
  },
};

const searchOption: Fig.Option = {
  name: "--search",
  description: "Enable live web search",
};

const noAltScreenOption: Fig.Option = {
  name: "--no-alt-screen",
  description: "Disable alternate screen mode, preserving terminal scrollback",
};

const remoteOption: Fig.Option = {
  name: "--remote",
  description: "Connect the TUI to a remote app server endpoint",
  args: { name: "ADDR" },
};

const remoteAuthTokenEnvOption: Fig.Option = {
  name: "--remote-auth-token-env",
  description: "Env var holding the bearer token for a remote app server websocket",
  args: { name: "ENV_VAR" },
};

const skipGitRepoCheckOption: Fig.Option = {
  name: "--skip-git-repo-check",
  description: "Allow running Codex outside a Git repository",
};

const ephemeralOption: Fig.Option = {
  name: "--ephemeral",
  description: "Run without persisting session files to disk",
};

const ignoreUserConfigOption: Fig.Option = {
  name: "--ignore-user-config",
  description: "Do not load $CODEX_HOME/config.toml; auth still uses CODEX_HOME",
};

const ignoreRulesOption: Fig.Option = {
  name: "--ignore-rules",
  description: "Do not load user or project execpolicy .rules files",
};

const outputSchemaOption: Fig.Option = {
  name: "--output-schema",
  description: "Path to a JSON Schema file describing the model's final response shape",
  args: { name: "FILE", generators: filepaths({ extensions: ["json"] }) },
};

const jsonlOption: Fig.Option = {
  name: "--json",
  description: "Print events to stdout as JSONL",
};

const outputLastMessageOption: Fig.Option = {
  name: ["-o", "--output-last-message"],
  description: "File where the last message from the agent should be written",
  args: { name: "FILE", template: "filepaths" },
};

/** The three ways `review` can choose a diff; only one of them makes sense at a time. */
const reviewTargetOptions: Fig.Option[] = [
  {
    name: "--uncommitted",
    description: "Review staged, unstaged, and untracked changes",
    exclusiveOn: ["--base", "--commit"],
  },
  {
    name: "--base",
    description: "Review changes against the given base branch",
    exclusiveOn: ["--uncommitted", "--commit"],
    args: { name: "BRANCH" },
  },
  {
    name: "--commit",
    description: "Review the changes introduced by a commit",
    exclusiveOn: ["--uncommitted", "--base"],
    args: { name: "SHA" },
  },
  {
    name: "--title",
    description: "Optional commit title to display in the review summary",
    args: { name: "TITLE" },
  },
];

/** The interactive-TUI options shared by the root command, `resume` and `fork`. */
const interactiveOptions: Fig.Option[] = [
  ...commonOptions,
  remoteOption,
  remoteAuthTokenEnvOption,
  strictConfigOption,
  imageOption,
  modelOption,
  ossOption,
  localProviderOption,
  profileOption,
  sandboxOption,
  bypassOption,
  yoloOption,
  bypassHookTrustOption,
  cdOption,
  addDirOption,
  askForApprovalOption,
  searchOption,
  noAltScreenOption,
];

/** The session-management options `archive`, `unarchive` and `delete` all repeat. */
const sessionCommandOptions: Fig.Option[] = [
  ...commonOptions,
  remoteOption,
  remoteAuthTokenEnvOption,
  imageOption,
  modelOption,
  ossOption,
  localProviderOption,
  profileOption,
  sandboxOption,
  bypassOption,
  yoloOption,
  bypassHookTrustOption,
  cdOption,
  addDirOption,
  strictConfigOption,
];

const sessionArg: Fig.Arg = {
  name: "SESSION",
  description: "Session id (UUID) or session name; UUIDs take precedence if it parses",
  generators: sessionGenerator,
};

const helpSubcommand: Fig.Subcommand = {
  name: "help",
  description: "Print this message or the help of the given subcommand(s)",
  args: { name: "command", isOptional: true },
};

const completionSpec: Fig.Spec = {
  name: "codex",
  description: "Codex CLI - OpenAI's command-line coding agent",
  subcommands: [
    {
      name: ["exec", "e"],
      description: "Run Codex non-interactively",
      subcommands: [
        {
          name: "resume",
          description: "Resume a previous session by id or pick the most recent with --last",
          options: [
            ...commonOptions,
            {
              name: "--last",
              description: "Resume the most recent recorded session without specifying an id",
            },
            { name: "--all", description: "Show all sessions (disables cwd filtering)" },
            { ...imageOption, args: { name: "FILE", template: "filepaths" } },
            strictConfigOption,
            modelOption,
            bypassOption,
            yoloOption,
            bypassHookTrustOption,
            skipGitRepoCheckOption,
            ephemeralOption,
            ignoreUserConfigOption,
            ignoreRulesOption,
            outputSchemaOption,
            jsonlOption,
            outputLastMessageOption,
            helpOption,
          ],
          args: [
            {
              name: "SESSION_ID",
              description: "Conversation/session id (UUID) or thread name",
              isOptional: true,
              generators: sessionGenerator,
            },
            {
              name: "PROMPT",
              description: "Prompt to send after resuming the session",
              isOptional: true,
            },
          ],
        },
        {
          name: "review",
          description: "Run a code review against the current repository",
          options: [
            ...commonOptions,
            ...reviewTargetOptions,
            strictConfigOption,
            modelOption,
            bypassOption,
            yoloOption,
            bypassHookTrustOption,
            skipGitRepoCheckOption,
            ephemeralOption,
            ignoreUserConfigOption,
            ignoreRulesOption,
            outputSchemaOption,
            jsonlOption,
            outputLastMessageOption,
            helpOption,
          ],
          args: {
            name: "PROMPT",
            description: "Custom review instructions. If `-` is used, read from stdin",
            isOptional: true,
          },
        },
        helpSubcommand,
      ],
      options: [
        ...commonOptions,
        strictConfigOption,
        imageOption,
        modelOption,
        ossOption,
        localProviderOption,
        profileOption,
        sandboxOption,
        bypassOption,
        yoloOption,
        bypassHookTrustOption,
        cdOption,
        addDirOption,
        skipGitRepoCheckOption,
        ephemeralOption,
        ignoreUserConfigOption,
        ignoreRulesOption,
        outputSchemaOption,
        {
          name: "--color",
          description: "Color settings for use in the output",
          args: { name: "COLOR", default: "auto", suggestions: ["always", "never", "auto"] },
        },
        jsonlOption,
        outputLastMessageOption,
        helpOption,
        versionOption,
      ],
      args: {
        name: "PROMPT",
        description: "Initial instructions for the agent; `-` reads from stdin",
        isOptional: true,
      },
    },
    {
      name: "review",
      description: "Run a code review non-interactively",
      options: [...commonOptions, strictConfigOption, ...reviewTargetOptions, helpOption],
      args: {
        name: "PROMPT",
        description: "Custom review instructions. If `-` is used, read from stdin",
        isOptional: true,
      },
    },
    {
      name: "login",
      description: "Manage login",
      subcommands: [
        {
          name: "status",
          description: "Show login status",
          options: [...commonOptions, helpOption],
        },
        helpSubcommand,
      ],
      options: [
        ...commonOptions,
        {
          name: "--with-api-key",
          description: "Read the API key from stdin",
          exclusiveOn: ["--with-access-token"],
        },
        {
          name: "--with-access-token",
          description: "Read the access token from stdin",
          exclusiveOn: ["--with-api-key"],
        },
        { name: "--device-auth", description: "Sign in with the device authorization flow" },
        helpOption,
      ],
    },
    {
      name: "logout",
      description: "Remove stored authentication credentials",
      isDangerous: true,
      options: [...commonOptions, helpOption],
    },
    {
      name: "mcp",
      description: "Manage external MCP servers for Codex",
      subcommands: [
        {
          name: "list",
          description: "List the configured MCP servers",
          options: [
            ...commonOptions,
            { name: "--json", description: "Output the configured servers as JSON" },
            helpOption,
          ],
        },
        {
          name: "get",
          description: "Show one MCP server's configuration",
          options: [
            ...commonOptions,
            { name: "--json", description: "Output the server configuration as JSON" },
            helpOption,
          ],
          args: {
            name: "NAME",
            description: "Name of the MCP server to display",
            generators: mcpServerGenerator,
          },
        },
        {
          name: "add",
          description: "Add an MCP server configuration",
          options: [
            ...commonOptions,
            {
              name: "--env",
              description: "Environment variables for the server. Only valid with stdio servers",
              isRepeatable: true,
              args: { name: "KEY=VALUE" },
            },
            {
              name: "--url",
              description: "URL for a streamable HTTP MCP server",
              args: { name: "URL" },
            },
            {
              name: "--bearer-token-env-var",
              description: "Env var to read for a bearer token. Only valid with HTTP servers",
              args: { name: "ENV_VAR" },
            },
            {
              name: "--oauth-client-id",
              description: "Optional OAuth client identifier to use for this MCP server",
              args: { name: "CLIENT_ID" },
            },
            {
              name: "--oauth-resource",
              description: "Optional OAuth resource parameter to include during MCP login",
              args: { name: "RESOURCE" },
            },
            helpOption,
          ],
          args: [
            { name: "NAME", description: "Name for the MCP server configuration" },
            {
              name: "COMMAND",
              description: "Command to launch the server; use --url for an HTTP server",
              isOptional: true,
              isVariadic: true,
              isCommand: true,
            },
          ],
        },
        {
          name: "remove",
          description: "Remove an MCP server configuration",
          isDangerous: true,
          options: [...commonOptions, helpOption],
          args: {
            name: "NAME",
            description: "Name of the MCP server configuration to remove",
            generators: mcpServerGenerator,
          },
        },
        {
          name: "login",
          description: "Authenticate with an MCP server over OAuth",
          options: [
            ...commonOptions,
            {
              name: "--scopes",
              description: "Comma-separated list of OAuth scopes to request",
              args: { name: "SCOPE,SCOPE" },
            },
            helpOption,
          ],
          args: {
            name: "NAME",
            description: "Name of the MCP server to authenticate with oauth",
            generators: mcpServerGenerator,
          },
        },
        {
          name: "logout",
          description: "Clear stored credentials for an MCP server",
          isDangerous: true,
          options: [...commonOptions, helpOption],
          args: {
            name: "NAME",
            description: "Name of the MCP server to deauthenticate",
            generators: mcpServerGenerator,
          },
        },
        helpSubcommand,
      ],
      options: [...commonOptions, helpOption],
    },
    {
      name: "plugin",
      description: "Manage Codex plugins",
      subcommands: [
        {
          name: "add",
          description: "Install a plugin from a configured marketplace snapshot",
          options: [
            configOption,
            {
              name: ["-m", "--marketplace"],
              description: "Marketplace to use when PLUGIN does not include @MARKETPLACE",
              args: { name: "MARKETPLACE", generators: marketplaceGenerator },
            },
            enableOption,
            { name: "--json", description: "Output install result as JSON" },
            disableOption,
            helpOption,
          ],
          args: {
            name: "PLUGIN[@MARKETPLACE]",
            description: "Plugin selector to install",
          },
        },
        {
          name: "list",
          description: "List plugins available from configured marketplace snapshots",
          options: [
            configOption,
            {
              name: ["-m", "--marketplace"],
              description: "Only list plugins from this configured marketplace name",
              args: { name: "MARKETPLACE", generators: marketplaceGenerator },
            },
            enableOption,
            { name: "--json", description: "Output plugin list as JSON" },
            {
              name: "--available",
              description: "Include uninstalled marketplace plugins in the JSON output",
              dependsOn: ["--json"],
            },
            disableOption,
            helpOption,
          ],
        },
        {
          name: "marketplace",
          description: "Add, list, upgrade, or remove configured plugin marketplaces",
          subcommands: [
            {
              name: "add",
              description: "Add a local or Git marketplace to the configured sources",
              options: [...commonOptions, helpOption],
              args: { name: "SOURCE" },
            },
            {
              name: "list",
              description: "List plugin marketplaces Codex is considering and their roots",
              options: [...commonOptions, helpOption],
            },
            {
              name: "upgrade",
              description: "Refresh configured Git marketplace snapshots",
              options: [...commonOptions, helpOption],
            },
            {
              name: "remove",
              description: "Remove a configured marketplace source by name",
              isDangerous: true,
              options: [...commonOptions, helpOption],
              args: { name: "NAME", generators: marketplaceGenerator },
            },
            helpSubcommand,
          ],
          options: [...commonOptions, helpOption],
        },
        {
          name: "remove",
          description: "Remove an installed plugin from local config and cache",
          isDangerous: true,
          options: [
            configOption,
            {
              name: ["-m", "--marketplace"],
              description: "Marketplace to use when PLUGIN does not include @MARKETPLACE",
              args: { name: "MARKETPLACE", generators: marketplaceGenerator },
            },
            enableOption,
            { name: "--json", description: "Output remove result as JSON" },
            disableOption,
            helpOption,
          ],
          args: {
            name: "PLUGIN[@MARKETPLACE]",
            description: "Plugin selector to remove",
            generators: pluginGenerator,
          },
        },
        helpSubcommand,
      ],
      options: [...commonOptions, helpOption],
    },
    {
      name: "mcp-server",
      description: "Start Codex as an MCP server (stdio)",
      options: [...commonOptions, strictConfigOption, helpOption],
    },
    {
      name: "app-server",
      description: "[experimental] Run the app server or related tooling",
      subcommands: [
        {
          name: "daemon",
          description: "Manage the local app-server daemon",
          subcommands: [
            {
              name: "bootstrap",
              description: "Install durable local app-server management for SSH-driven use",
              options: [...commonOptions, helpOption],
            },
            {
              name: "start",
              description: "Start the local app server daemon if it is not already running",
              options: [...commonOptions, helpOption],
            },
            {
              name: "restart",
              description: "Restart the local app server daemon",
              options: [...commonOptions, helpOption],
            },
            {
              name: "enable-remote-control",
              description: "Enable remote control for future starts and a running managed daemon",
              options: [...commonOptions, helpOption],
            },
            {
              name: "disable-remote-control",
              description: "Disable remote control for future starts and a running managed daemon",
              options: [...commonOptions, helpOption],
            },
            {
              name: "stop",
              description: "Stop the local app server daemon",
              options: [...commonOptions, helpOption],
            },
            {
              name: "version",
              description: "Print local CLI and running app-server versions as JSON",
              options: [...commonOptions, helpOption],
            },
            helpSubcommand,
          ],
          options: [...commonOptions, helpOption],
        },
        {
          name: "proxy",
          description: "Proxy stdio bytes to the running app-server control socket",
          options: [
            configOption,
            {
              name: "--sock",
              description: "Path to the app-server Unix domain socket to connect to",
              args: { name: "SOCKET_PATH", template: "filepaths" },
            },
            enableOption,
            disableOption,
            helpOption,
          ],
        },
        {
          name: "generate-ts",
          description: "[experimental] Generate TypeScript bindings for the app server protocol",
          options: [
            configOption,
            {
              name: ["-o", "--out"],
              description: "Output directory where .ts files will be written",
              args: { name: "DIR", generators: folders },
            },
            enableOption,
            {
              name: ["-p", "--prettier"],
              description: "Optional path to the Prettier executable to format generated files",
              args: { name: "PRETTIER_BIN", template: "filepaths" },
            },
            disableOption,
            {
              name: "--experimental",
              description: "Include experimental methods and fields in the generated output",
            },
            helpOption,
          ],
        },
        {
          name: "generate-json-schema",
          description: "[experimental] Generate JSON Schema for the app server protocol",
          options: [
            configOption,
            {
              name: ["-o", "--out"],
              description: "Output directory where the schema bundle will be written",
              args: { name: "DIR", generators: folders },
            },
            enableOption,
            {
              name: "--experimental",
              description: "Include experimental methods and fields in the generated output",
            },
            disableOption,
            helpOption,
          ],
        },
        helpSubcommand,
      ],
      options: [
        ...commonOptions,
        strictConfigOption,
        {
          name: "--listen",
          description: "Transport endpoint URL (stdio://, unix://, unix://PATH, ws://IP:PORT, off)",
          exclusiveOn: ["--stdio"],
          args: { name: "URL", default: "stdio://" },
        },
        {
          name: "--stdio",
          description: "Use stdio as the transport (equivalent to --listen stdio://)",
          exclusiveOn: ["--listen"],
        },
        {
          name: "--analytics-default-enabled",
          description: "Enable analytics by default for first-party surfaces",
        },
        {
          name: "--ws-auth",
          description: "Websocket auth mode for non-loopback listeners",
          args: {
            name: "MODE",
            suggestions: ["capability-token", "signed-bearer-token"],
          },
        },
        {
          name: "--ws-token-file",
          description: "Absolute path to the capability-token file",
          args: { name: "PATH", template: "filepaths" },
        },
        {
          name: "--ws-token-sha256",
          description: "Hex-encoded SHA-256 digest of the capability token",
          args: { name: "HEX" },
        },
        {
          name: "--ws-shared-secret-file",
          description: "Absolute path to the shared secret file for signed JWT bearer tokens",
          args: { name: "PATH", template: "filepaths" },
        },
        {
          name: "--ws-issuer",
          description: "Expected issuer for signed JWT bearer tokens",
          args: { name: "ISSUER" },
        },
        {
          name: "--ws-audience",
          description: "Expected audience for signed JWT bearer tokens",
          args: { name: "AUDIENCE" },
        },
        {
          name: "--ws-max-clock-skew-seconds",
          description: "Maximum clock skew when validating signed JWT bearer tokens",
          args: { name: "SECONDS" },
        },
        helpOption,
      ],
    },
    {
      name: "remote-control",
      description: "[experimental] Manage the app-server daemon with remote control enabled",
      subcommands: [
        {
          name: "start",
          description: "Start the app-server daemon with remote control enabled",
          options: [{ name: "--json", description: "Emit machine-readable JSON" }, helpOption],
        },
        {
          name: "stop",
          description: "Stop the app-server daemon",
          options: [{ name: "--json", description: "Emit machine-readable JSON" }, helpOption],
        },
        {
          name: "pair",
          description: "Create and print a short-lived manual pairing code",
          options: [{ name: "--json", description: "Emit machine-readable JSON" }, helpOption],
        },
        helpSubcommand,
      ],
      options: [{ name: "--json", description: "Emit machine-readable JSON" }, helpOption],
    },
    {
      name: "app",
      description: "Launch the Codex desktop app (opens the app installer if missing)",
      options: [
        configOption,
        {
          name: "--download-url",
          description: "Override the app installer download URL (advanced)",
          args: { name: "DOWNLOAD_URL_OVERRIDE" },
        },
        enableOption,
        disableOption,
        helpOption,
      ],
      args: {
        name: "PATH",
        description: "Workspace path to open in Codex Desktop",
        isOptional: true,
        default: ".",
        generators: folders,
      },
    },
    {
      name: "completion",
      description: "Generate shell completion scripts",
      options: [helpOption],
      args: {
        name: "SHELL",
        description: "Shell to generate completions for",
        isOptional: true,
        default: "bash",
        suggestions: ["bash", "elvish", "fish", "powershell", "zsh"],
      },
    },
    {
      name: "update",
      description: "Update Codex to the latest version",
      options: [helpOption],
    },
    {
      name: "doctor",
      description: "Diagnose local Codex installation, config, auth, and runtime health",
      options: [
        { name: "--json", description: "Emit a redacted machine-readable report" },
        {
          name: "--summary",
          description: "Only show grouped check rows and the final count summary",
          exclusiveOn: ["--all"],
        },
        {
          name: "--all",
          description: "Expand long lists in detailed human output",
          exclusiveOn: ["--summary"],
        },
        { name: "--no-color", description: "Disable ANSI color in human output" },
        { name: "--ascii", description: "Use ASCII status labels and separators in human output" },
        helpOption,
      ],
    },
    {
      name: "sandbox",
      description: "Run commands within a Codex-provided sandbox",
      options: [
        {
          name: "--sandbox-state-json",
          description: "JSON value from `codex/sandbox-state-meta` to apply directly",
          args: { name: "JSON" },
        },
        {
          name: "--sandbox-state-readable-root",
          description: "Add a readable root to the supplied sandbox state",
          isRepeatable: true,
          args: { name: "SANDBOX_STATE_READABLE_ROOT", generators: folders },
        },
        {
          name: "--sandbox-state-disable-network",
          description: "Disable direct network access in the supplied sandbox state",
        },
        {
          name: ["-P", "--permission-profile"],
          description: "Named permissions profile to apply from the active configuration stack",
          args: { name: "NAME" },
        },
        profileOption,
        {
          name: ["-C", "--cd"],
          description: "Working directory used for profile resolution and command execution",
          args: { name: "DIR", generators: folders },
        },
        {
          name: "--include-managed-config",
          description: "Include managed requirements while resolving a permissions profile",
        },
        {
          name: "--allow-unix-socket",
          description: "Allow the sandboxed command to bind/connect AF_UNIX sockets under a path",
          isRepeatable: true,
          args: { name: "ALLOW_UNIX_SOCKETS", template: "filepaths" },
        },
        {
          name: "--log-denials",
          description: "Capture macOS sandbox denials via `log stream` and print them after exit",
        },
        helpOption,
      ],
      args: {
        name: "COMMAND",
        description: "Full command args to run under seatbelt",
        isOptional: true,
        isVariadic: true,
        isCommand: true,
      },
    },
    {
      name: "debug",
      description: "Debugging tools",
      subcommands: [
        {
          name: "models",
          description: "Render the raw model catalog as JSON",
          options: [
            ...commonOptions,
            {
              name: "--bundled",
              description: "Skip refresh and dump only the catalog shipped with this binary",
            },
            helpOption,
          ],
        },
        {
          name: "app-server",
          description: "Tooling: helps debug the app server",
          subcommands: [
            {
              name: "send-message-v2",
              description: "Send one app-server message and print the reply",
              options: [...commonOptions, helpOption],
            },
            helpSubcommand,
          ],
          options: [...commonOptions, helpOption],
        },
        {
          name: "prompt-input",
          description: "Render the model-visible prompt input list as JSON",
          options: [...commonOptions, imageOption, helpOption],
          args: {
            name: "PROMPT",
            description: "Optional user prompt to append after session context",
            isOptional: true,
          },
        },
        helpSubcommand,
      ],
      options: [...commonOptions, helpOption],
    },
    {
      name: ["apply", "a"],
      description: "Apply the latest diff produced by Codex agent as a `git apply`",
      options: [...commonOptions, helpOption],
      args: { name: "TASK_ID" },
    },
    {
      name: "resume",
      description: "Resume a previous interactive session (picker by default)",
      options: [
        ...commonOptions,
        { name: "--last", description: "Continue the most recent session without the picker" },
        {
          name: "--all",
          description: "Show all sessions (disables cwd filtering and shows CWD column)",
        },
        {
          name: "--include-non-interactive",
          description: "Include non-interactive sessions in the picker and --last selection",
        },
        remoteOption,
        remoteAuthTokenEnvOption,
        strictConfigOption,
        imageOption,
        modelOption,
        ossOption,
        localProviderOption,
        profileOption,
        sandboxOption,
        bypassOption,
        yoloOption,
        bypassHookTrustOption,
        cdOption,
        addDirOption,
        askForApprovalOption,
        searchOption,
        noAltScreenOption,
        helpOption,
        versionOption,
      ],
      args: [
        {
          name: "SESSION_ID",
          description: "Session id (UUID) or session name; omit it and use --last",
          isOptional: true,
          generators: sessionGenerator,
        },
        {
          name: "PROMPT",
          description: "Optional user prompt to start the session",
          isOptional: true,
        },
      ],
    },
    {
      name: "archive",
      description: "Archive a saved session by id or session name",
      options: [...sessionCommandOptions, helpOption],
      args: sessionArg,
    },
    {
      name: "delete",
      description: "Permanently delete a saved session by id or session name",
      isDangerous: true,
      options: [
        ...sessionCommandOptions,
        {
          name: "--force",
          description: "Delete without prompting. SESSION must be a UUID",
          isDangerous: true,
        },
        helpOption,
      ],
      args: sessionArg,
    },
    {
      name: "unarchive",
      description: "Unarchive a saved session by id or session name",
      options: [...sessionCommandOptions, helpOption],
      args: sessionArg,
    },
    {
      name: "fork",
      description: "Fork a previous interactive session (picker by default)",
      options: [
        ...commonOptions,
        { name: "--last", description: "Fork the most recent session without showing the picker" },
        {
          name: "--all",
          description: "Show all sessions (disables cwd filtering and shows CWD column)",
        },
        remoteOption,
        remoteAuthTokenEnvOption,
        strictConfigOption,
        imageOption,
        modelOption,
        ossOption,
        localProviderOption,
        profileOption,
        sandboxOption,
        bypassOption,
        yoloOption,
        bypassHookTrustOption,
        cdOption,
        addDirOption,
        askForApprovalOption,
        searchOption,
        noAltScreenOption,
        helpOption,
        versionOption,
      ],
      args: [
        {
          name: "SESSION_ID",
          description: "Conversation/session id (UUID) to fork; omit it and use --last",
          isOptional: true,
          generators: sessionGenerator,
        },
        {
          name: "PROMPT",
          description: "Optional user prompt to start the session",
          isOptional: true,
        },
      ],
    },
    {
      name: "cloud",
      description: "[EXPERIMENTAL] Browse tasks from Codex Cloud and apply changes locally",
      subcommands: [
        {
          name: "exec",
          description: "Submit a new Codex Cloud task without launching the TUI",
          options: [
            {
              name: "--env",
              description: "Target environment identifier (see `codex cloud` to browse)",
              args: { name: "ENV_ID" },
            },
            {
              name: "--attempts",
              description: "Number of assistant attempts (best-of-N)",
              args: { name: "ATTEMPTS", default: "1" },
            },
            {
              name: "--branch",
              description: "Git branch to run in Codex Cloud (defaults to current branch)",
              args: { name: "BRANCH" },
            },
            helpOption,
          ],
          args: {
            name: "QUERY",
            description: "Task prompt to run in Codex Cloud",
            isOptional: true,
          },
        },
        {
          name: "status",
          description: "Show the status of a Codex Cloud task",
          options: [helpOption],
          args: { name: "TASK_ID", description: "Codex Cloud task identifier to inspect" },
        },
        {
          name: "list",
          description: "List Codex Cloud tasks",
          options: [
            {
              name: "--env",
              description: "Filter tasks by environment identifier",
              args: { name: "ENV_ID" },
            },
            {
              name: "--limit",
              description: "Maximum number of tasks to return (1-20)",
              args: { name: "N", default: "20" },
            },
            {
              name: "--cursor",
              description: "Pagination cursor returned by a previous call",
              args: { name: "CURSOR" },
            },
            { name: "--json", description: "Emit JSON instead of plain text" },
            helpOption,
          ],
        },
        {
          name: "apply",
          description: "Apply the diff for a Codex Cloud task locally",
          options: [
            {
              name: "--attempt",
              description: "Attempt number to apply (1-based)",
              args: { name: "N" },
            },
            helpOption,
          ],
          args: { name: "TASK_ID", description: "Codex Cloud task identifier to apply" },
        },
        {
          name: "diff",
          description: "Show the unified diff for a Codex Cloud task",
          options: [
            {
              name: "--attempt",
              description: "Attempt number to display (1-based)",
              args: { name: "N" },
            },
            helpOption,
          ],
          args: { name: "TASK_ID", description: "Codex Cloud task identifier to display" },
        },
        helpSubcommand,
      ],
      options: [helpOption, versionOption],
    },
    {
      name: "exec-server",
      description: "[EXPERIMENTAL] Run the standalone exec-server service",
      options: [
        configOption,
        strictConfigOption,
        enableOption,
        {
          name: "--listen",
          description: "Transport endpoint URL: ws://IP:PORT (default), stdio, stdio://",
          args: { name: "URL" },
        },
        disableOption,
        {
          name: "--remote",
          description: "Register this exec-server as a remote environment using this base URL",
          args: { name: "URL" },
        },
        {
          name: "--environment-id",
          description: "Environment id to attach to when registering remotely",
          args: { name: "ID" },
        },
        {
          name: "--name",
          description: "Human-readable environment name",
          args: { name: "NAME" },
        },
        {
          name: "--use-agent-identity-auth",
          description: "Use Agent Identity auth from CODEX_ACCESS_TOKEN for remote registration",
        },
        helpOption,
      ],
    },
    {
      name: "features",
      description: "Inspect feature flags",
      subcommands: [
        {
          name: "list",
          description: "List known features with their stage and effective state",
          options: [...commonOptions, helpOption],
        },
        {
          name: "enable",
          description: "Enable a feature in config.toml",
          options: [...commonOptions, helpOption],
          args: { name: "FEATURE", description: "Feature key to update (e.g. unified_exec)" },
        },
        {
          name: "disable",
          description: "Disable a feature in config.toml",
          options: [...commonOptions, helpOption],
          args: { name: "FEATURE", description: "Feature key to update (e.g. unified_exec)" },
        },
        helpSubcommand,
      ],
      options: [...commonOptions, helpOption],
    },
    helpSubcommand,
  ],
  options: [
    ...interactiveOptions,
    {
      name: "--full-auto",
      description: "Removed; use --sandbox workspace-write instead",
      hidden: true,
      deprecated: {
        description: "`--full-auto` is deprecated; use `--sandbox workspace-write` instead",
      },
    },
    helpOption,
    versionOption,
  ],
  args: {
    name: "PROMPT",
    description: "Optional user prompt to start the session",
    isOptional: true,
  },
};

export default completionSpec;
