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

const basename = (path: string): string => path.slice(path.lastIndexOf("/") + 1);

/**
 * Session ids for `--resume`, read off disk rather than by starting Claude.
 *
 * Transcripts live at `~/.claude/projects/<key>/<session-id>.jsonl`, where `<key>` is the
 * working directory with every non-alphanumeric byte replaced by `-` — checked against the
 * real directory names, which turn `/Users/me/dev/x/.personal_docs` into
 * `-Users-me-dev-x--personal-docs`.
 */
const sessionIdGenerator: Fig.Generator = {
  script: [
    "bash",
    "-c",
    `dir="$HOME/.claude/projects/$(pwd | sed 's![^a-zA-Z0-9]!-!g')"; ls -t "$dir"/*.jsonl 2>/dev/null | head -40`,
  ],
  postProcess: (out) =>
    uniqueSuggestions(
      out
        .split("\n")
        .map((line) => line.trim())
        .filter((line) => line.endsWith(".jsonl"))
        .map((line) => ({
          name: basename(line).replace(/\.jsonl$/, ""),
          description: "Session recorded in this directory",
          icon: "fig://icon?type=commit",
        })),
    ),
  cache: { ttl: 5000, cacheByDirectory: true },
};

/**
 * Short ids of background sessions, for `attach`, `logs`, `stop`, `rm` and `respawn`.
 *
 * `claude --bg` prints the id and keeps its bookkeeping in `~/.claude/jobs/<id>/state.json`,
 * so the list (and each session's state) is available without running the CLI.
 */
const backgroundSessionGenerator: Fig.Generator = {
  script: [
    "bash",
    "-c",
    `for dir in "$HOME"/.claude/jobs/*/; do [ -f "$dir/state.json" ] || continue; printf '%s|' "$(basename "$dir")"; tr -d '\\n' < "$dir/state.json"; printf '\\n'; done 2>/dev/null`,
  ],
  postProcess: (out) =>
    uniqueSuggestions(
      out.split("\n").flatMap((line): Fig.Suggestion[] => {
        const separator = line.indexOf("|");
        if (separator <= 0) {
          return [];
        }
        const state = parseJson(line.slice(separator + 1));
        const status = isRecord(state) ? string(state.state) : undefined;
        const detail = isRecord(state) ? string(state.detail) : undefined;
        return [
          {
            name: line.slice(0, separator),
            description: [status ?? "Background session", detail].filter(Boolean).join(" — "),
            icon: "fig://icon?type=gear",
          },
        ];
      }),
    ),
  cache: { ttl: 5000 },
};

const mcpServerNames = (config: unknown): string[] =>
  isRecord(config) && isRecord(config.mcpServers) ? Object.keys(config.mcpServers) : [];

/**
 * MCP server names for `claude mcp get|remove|login|logout`.
 *
 * `claude mcp list` health-checks every server, so this reads the same three scopes off
 * disk instead: the user scope and the per-project scope in `~/.claude.json`, plus the
 * project's own `.mcp.json`.
 */
const mcpServerGenerator: Fig.Generator = {
  custom: async (_tokens, executeShellCommand, context) => {
    const read = async (path: string): Promise<unknown> => {
      try {
        const { stdout } = await executeShellCommand({
          command: "bash",
          args: ["-c", `cat ${path} 2>/dev/null`],
        });
        return parseJson(stdout);
      } catch (_err) {
        return undefined;
      }
    };

    const [userConfig, projectConfig] = await Promise.all([
      read(`"$HOME/.claude.json"`),
      read(".mcp.json"),
    ]);

    const projects = isRecord(userConfig) ? userConfig.projects : undefined;
    const currentProject = isRecord(projects)
      ? projects[context.currentWorkingDirectory]
      : undefined;

    return uniqueSuggestions(
      [
        ...mcpServerNames(userConfig),
        ...mcpServerNames(currentProject),
        ...mcpServerNames(projectConfig),
      ].map((name) => ({
        name,
        description: "Configured MCP server",
        icon: "fig://icon?type=box",
      })),
    );
  },
  cache: { ttl: 5000, cacheByDirectory: true },
};

/**
 * Installed plugins, keyed `name@marketplace` in
 * `~/.claude/plugins/installed_plugins.json`. Both spellings are suggested because the
 * plugin subcommands accept either.
 */
const pluginGenerator: Fig.Generator = {
  script: ["bash", "-c", `cat "$HOME/.claude/plugins/installed_plugins.json" 2>/dev/null`],
  postProcess: (out) => {
    const parsed = parseJson(out);
    if (!isRecord(parsed) || !isRecord(parsed.plugins)) {
      return [];
    }
    const ids = Object.keys(parsed.plugins);
    return uniqueSuggestions([
      ...ids.map((id) => ({
        name: id,
        description: "Installed plugin",
        icon: "fig://icon?type=box",
        priority: 75,
      })),
      ...ids.map((id) => ({
        name: id.split("@")[0],
        description: "Installed plugin",
        icon: "fig://icon?type=box",
      })),
    ]);
  },
  cache: { ttl: 10_000 },
};

/** Marketplaces registered in `~/.claude/plugins/known_marketplaces.json`. */
const marketplaceGenerator: Fig.Generator = {
  script: ["bash", "-c", `cat "$HOME/.claude/plugins/known_marketplaces.json" 2>/dev/null`],
  postProcess: (out) => {
    const parsed = parseJson(out);
    if (!isRecord(parsed)) {
      return [];
    }
    return uniqueSuggestions(
      Object.keys(parsed).map((name) => ({
        name,
        description: "Configured marketplace",
        icon: "fig://icon?type=box",
      })),
    );
  },
  cache: { ttl: 10_000 },
};

/** Custom agents: `.claude/agents/*.md` in the project, then the user-level ones. */
const agentGenerator: Fig.Generator = {
  script: ["bash", "-c", `ls -1 .claude/agents/*.md "$HOME"/.claude/agents/*.md 2>/dev/null`],
  postProcess: (out) =>
    uniqueSuggestions(
      out
        .split("\n")
        .map((line) => line.trim())
        .filter((line) => line.endsWith(".md"))
        .map((line) => ({
          name: basename(line).replace(/\.md$/, ""),
          description: "Custom agent",
          icon: "fig://icon?type=asterisk",
        })),
    ),
  cache: { ttl: 10_000, cacheByDirectory: true },
};

const helpOption: Fig.Option = {
  name: ["-h", "--help"],
  description: "Display help for command",
};

const jsonResultOption: Fig.Option = {
  name: "--json",
  description: "Print one machine-readable result line instead of the human message",
};

const modelOption: Fig.Option = {
  name: "--model",
  description: "Model for the current session",
  args: {
    name: "model",
    suggestions: [
      { name: "fable", description: "Latest Fable model" },
      { name: "opus", description: "Latest Opus model" },
      { name: "sonnet", description: "Latest Sonnet model" },
      { name: "haiku", description: "Latest Haiku model" },
      { name: "claude-fable-5", description: "Full model name" },
      { name: "claude-sonnet-5", description: "Full model name" },
    ],
  },
};

const permissionModeOption: Fig.Option = {
  name: "--permission-mode",
  description: "Permission mode to use for the session",
  args: {
    name: "mode",
    suggestions: [
      { name: "acceptEdits", description: "Auto-accept file edits" },
      { name: "auto", description: "Let the auto-mode classifier decide" },
      { name: "bypassPermissions", description: "Skip every permission check" },
      { name: "manual", description: "Ask for everything" },
      { name: "dontAsk", description: "Deny anything that would prompt" },
      { name: "plan", description: "Plan first, do not touch the workspace" },
    ],
  },
};

const effortOption: Fig.Option = {
  name: "--effort",
  description: "Effort level for the current session",
  args: {
    name: "level",
    suggestions: ["low", "medium", "high", "xhigh", "max"],
  },
};

const settingSourcesOption: Fig.Option = {
  name: "--setting-sources",
  description: "Comma-separated list of setting sources to load (user, project, local)",
  args: { name: "sources" },
};

const mcpScopeOption = (description: string): Fig.Option => ({
  name: ["-s", "--scope"],
  description,
  args: {
    name: "scope",
    suggestions: [
      { name: "local", description: "This project, only for you" },
      { name: "user", description: "Every project on this machine" },
      { name: "project", description: "Shared through the project's .mcp.json" },
    ],
  },
});

const pluginScopeOption = (description: string, includeManaged = false): Fig.Option => ({
  name: ["-s", "--scope"],
  description,
  args: {
    name: "scope",
    suggestions: includeManaged
      ? ["user", "project", "local", "managed"]
      : ["user", "project", "local"],
  },
});

const completionSpec: Fig.Spec = {
  name: "claude",
  description:
    "Claude Code - starts an interactive session by default, use -p/--print for non-interactive output",
  subcommands: [
    {
      name: "agents",
      description: "Manage background agents",
      options: [
        {
          name: "--add-dir",
          description: "Additional directory to allow tool access to in dispatched sessions",
          isRepeatable: true,
          args: { name: "directory", generators: folders },
        },
        {
          name: "--agent",
          description: "Default agent for sessions dispatched from agent view",
          args: { name: "agent", generators: agentGenerator },
        },
        {
          name: "--all",
          description: "With --json: also include completed background sessions",
        },
        {
          name: "--allow-dangerously-skip-permissions",
          description:
            "Make bypass-permissions mode available to dispatched sessions without defaulting to it",
        },
        {
          name: "--cwd",
          description: "Show only background sessions started under this path",
          args: { name: "path", generators: folders },
        },
        {
          name: "--dangerously-skip-permissions",
          description: "Alias for --permission-mode bypassPermissions",
          isDangerous: true,
        },
        { ...effortOption, description: "Default effort level for dispatched sessions" },
        helpOption,
        {
          name: "--json",
          description: "Print active sessions as a JSON array and exit (does not require a TTY)",
        },
        {
          name: "--mcp-config",
          description: "MCP server configuration to apply to dispatched sessions",
          isRepeatable: true,
          args: { name: "config", template: "filepaths" },
        },
        { ...modelOption, description: "Default model for sessions dispatched from agent view" },
        {
          ...permissionModeOption,
          description: "Default permission mode for sessions dispatched from agent view",
        },
        {
          name: "--plugin-dir",
          description: "Load plugins from a directory for the agent view and dispatched sessions",
          isRepeatable: true,
          args: { name: "path", generators: folders },
        },
        {
          name: "--restricted",
          description: "Start dispatched sessions in restricted mode",
        },
        settingSourcesOption,
        {
          name: "--settings",
          description: "Settings file or JSON string to apply to the agent view",
          args: { name: "file-or-json", template: "filepaths" },
        },
        {
          name: "--strict-mcp-config",
          description: "Only use MCP servers from --mcp-config in dispatched sessions",
        },
      ],
    },
    {
      name: "attach",
      description: "Open a background session in this terminal",
      args: {
        name: "id",
        description: "Short id that `claude --bg` prints and `claude agents` lists",
        generators: backgroundSessionGenerator,
      },
    },
    {
      name: "auth",
      description: "Manage authentication",
      subcommands: [
        {
          name: "login",
          description: "Sign in to your Anthropic account",
          options: [
            {
              name: "--claudeai",
              description: "Use Claude subscription (default)",
              exclusiveOn: ["--console"],
            },
            {
              name: "--console",
              description: "Use Anthropic Console (API usage billing) instead of a subscription",
              exclusiveOn: ["--claudeai"],
            },
            {
              name: "--email",
              description: "Pre-populate email address on the login page",
              args: { name: "email" },
            },
            helpOption,
            { name: "--sso", description: "Force SSO login flow" },
          ],
        },
        {
          name: "logout",
          description: "Log out from your Anthropic account",
          isDangerous: true,
          options: [helpOption],
        },
        {
          name: "status",
          description: "Show authentication status",
          options: [
            helpOption,
            { name: "--json", description: "Output as JSON (default)", exclusiveOn: ["--text"] },
            {
              name: "--text",
              description: "Output as human-readable text",
              exclusiveOn: ["--json"],
            },
          ],
        },
        {
          name: "help",
          description: "Display help for command",
          args: { name: "command", isOptional: true },
        },
      ],
      options: [helpOption],
    },
    {
      name: "auto-mode",
      description: "Inspect or reset auto mode classifier configuration",
      subcommands: [
        {
          name: "config",
          description: "Print the effective auto mode config as JSON",
          options: [helpOption],
        },
        {
          name: "critique",
          description: "Get AI feedback on your custom auto mode rules",
          options: [
            helpOption,
            { ...modelOption, description: "Override which model is used" },
          ],
        },
        {
          name: "defaults",
          description: "Print the default auto mode rules as JSON",
          options: [
            helpOption,
            {
              name: "--label",
              description: "Show only rules whose label starts with this prefix",
              args: { name: "prefix" },
            },
          ],
        },
        {
          name: "reset",
          description: "Reset auto mode configuration to the shipped defaults",
          isDangerous: true,
          options: [helpOption, { name: ["-y", "--yes"], description: "Skip the confirmation prompt" }],
        },
        {
          name: "help",
          description: "Display help for command",
          args: { name: "command", isOptional: true },
        },
      ],
      options: [helpOption],
    },
    {
      name: "doctor",
      description: "Check the health of your Claude Code installation",
      options: [helpOption],
    },
    {
      name: "gateway",
      description: "Run the enterprise auth/telemetry gateway",
      options: [
        {
          name: "--config",
          description: "Path to gateway YAML config",
          args: { name: "path", generators: filepaths({ extensions: ["yaml", "yml"] }) },
        },
        helpOption,
      ],
    },
    {
      name: "import",
      description: "Import config from another AI coding agent into Claude Code",
      options: [
        {
          name: "--dry-run",
          description: "Show what would be imported without writing anything",
        },
        helpOption,
        {
          name: "--yes",
          description: "Skip the interactive picker; on headless surfaces pass --yes=<digest>",
          requiresEquals: true,
          args: { name: "digest", isOptional: true },
        },
      ],
      args: {
        name: "source",
        description: "Which agent to import from",
        isOptional: true,
        suggestions: ["codex", "gemini", "cursor"],
      },
    },
    {
      name: "install",
      description: "Install Claude Code native build",
      options: [
        { name: "--force", description: "Force installation even if already installed" },
        helpOption,
      ],
      args: {
        name: "target",
        description: "Version to install",
        isOptional: true,
        suggestions: ["stable", "latest"],
      },
    },
    {
      name: "logs",
      description: "Print a background session's recent terminal output",
      args: { name: "id", generators: backgroundSessionGenerator },
    },
    {
      name: "mcp",
      description: "Configure and manage MCP servers",
      subcommands: [
        {
          name: "add",
          description: "Add an MCP server to Claude Code",
          options: [
            {
              name: "--callback-port",
              description: "Fixed port for OAuth callback",
              args: { name: "port" },
            },
            {
              name: "--client-id",
              description: "OAuth client ID for HTTP/SSE servers",
              args: { name: "clientId" },
            },
            {
              name: "--client-secret",
              description: "Prompt for OAuth client secret",
            },
            {
              name: ["-e", "--env"],
              description: "Set environment variables (e.g. -e KEY=value)",
              isRepeatable: true,
              args: { name: "env" },
            },
            {
              name: ["-H", "--header"],
              description: "Set headers for HTTP/SSE servers",
              isRepeatable: true,
              args: { name: "header" },
            },
            helpOption,
            mcpScopeOption("Configuration scope (local, user, or project)"),
            {
              name: ["-t", "--transport"],
              description: "Transport type, stdio unless specified",
              args: { name: "transport", suggestions: ["stdio", "sse", "http"] },
            },
          ],
          args: [
            { name: "name" },
            { name: "commandOrUrl" },
            { name: "args", isOptional: true, isVariadic: true },
          ],
        },
        {
          name: "add-from-claude-desktop",
          description: "Import MCP servers from Claude Desktop (Mac and WSL only)",
          options: [helpOption, mcpScopeOption("Configuration scope (local, user, or project)")],
        },
        {
          name: "add-json",
          description: "Add an MCP server (stdio, SSE, HTTP, or WebSocket) with a JSON string",
          options: [
            { name: "--client-secret", description: "Prompt for OAuth client secret" },
            helpOption,
            mcpScopeOption("Configuration scope (local, user, or project)"),
          ],
          args: [{ name: "name" }, { name: "json" }],
        },
        {
          name: "get",
          description: "Get details about an MCP server",
          options: [helpOption],
          args: { name: "name", generators: mcpServerGenerator },
        },
        {
          name: "list",
          description: "List configured MCP servers",
          options: [helpOption],
        },
        {
          name: "login",
          description: "Authenticate with an MCP server (HTTP, SSE, or claude.ai connector)",
          options: [
            helpOption,
            {
              name: "--no-browser",
              description: "Print the authorization URL instead of opening a browser",
            },
          ],
          args: { name: "name", generators: mcpServerGenerator },
        },
        {
          name: "logout",
          description: "Clear stored OAuth credentials for an MCP server",
          isDangerous: true,
          options: [helpOption],
          args: { name: "name", generators: mcpServerGenerator },
        },
        {
          name: "remove",
          description: "Remove an MCP server",
          isDangerous: true,
          options: [
            helpOption,
            mcpScopeOption("Configuration scope - defaults to wherever the server exists"),
          ],
          args: { name: "name", generators: mcpServerGenerator },
        },
        {
          name: "reset-project-choices",
          description: "Reset all approved and rejected project-scoped (.mcp.json) servers",
          options: [helpOption],
        },
        {
          name: "serve",
          description: "Start the Claude Code MCP server",
          options: [
            { name: ["-d", "--debug"], description: "Enable debug mode" },
            helpOption,
            { name: "--verbose", description: "Override verbose mode setting from config" },
          ],
        },
        {
          name: "help",
          description: "Display help for command",
          args: { name: "command", isOptional: true },
        },
      ],
      options: [helpOption],
    },
    {
      name: ["plugin", "plugins"],
      description: "Manage Claude Code plugins",
      subcommands: [
        {
          name: "details",
          description: "Show a plugin's component inventory and projected token cost",
          options: [helpOption],
          args: { name: "name", generators: pluginGenerator },
        },
        {
          name: "disable",
          description: "Disable an enabled plugin",
          options: [
            { name: ["-a", "--all"], description: "Disable all enabled plugins" },
            helpOption,
            jsonResultOption,
            pluginScopeOption("Installation scope: user, project, local (default: auto-detect)"),
          ],
          args: { name: "plugin", isOptional: true, generators: pluginGenerator },
        },
        {
          name: "enable",
          description: "Enable a disabled plugin",
          options: [
            helpOption,
            jsonResultOption,
            pluginScopeOption("Installation scope: user, project, local (default: auto-detect)"),
          ],
          args: { name: "plugin", generators: pluginGenerator },
        },
        {
          name: "eval",
          description: "Run eval cases against a plugin and report scored results",
          subcommands: [
            {
              name: "init",
              description: "Author an eval suite under the eval dir via an interview",
              options: [
                helpOption,
                {
                  name: "--bare",
                  description: "Start from a blank single-case template",
                  args: { name: "name", isOptional: true },
                },
              ],
              args: { name: "name", isOptional: true },
            },
          ],
          options: [
            {
              name: "--ablation",
              description: "Run a no-plugin baseline arm and report the score delta",
              args: { name: "mode", suggestions: ["none", "with-without"] },
            },
            {
              name: "--allow-tools",
              description: "Operator grant for gated tools (Bash, Write, Edit, WebFetch, mcp__*)",
              args: { name: "tools", isVariadic: true },
            },
            {
              name: "--case",
              description: "Filter cases by name glob",
              args: { name: "glob" },
            },
            {
              name: "--eval-dir",
              description: "Directory name (below the plugin) that holds the eval cases",
              args: { name: "dir", generators: folders },
            },
            helpOption,
            {
              name: "--json",
              description: "Print the full run result as JSON, or write it to this .json file",
              args: { name: "path", isOptional: true, template: "filepaths" },
            },
            {
              name: "--judge-model",
              description: "Override LLM-grader model (default: haiku)",
              args: { name: "model" },
            },
            { name: "--keep-temp", description: "Preserve scaffold dirs for debugging" },
            {
              name: "--max-cost-usd",
              description: "Optional hard cost ceiling; abort and report partial results if hit",
              args: { name: "usd" },
            },
            {
              name: "--mocks",
              description: "Mock stand-ins for MCP servers, from the eval dir's mocks/",
              args: { name: "mode", suggestions: ["record", "off"] },
            },
            { ...modelOption, description: "Override model for all cases" },
            {
              name: "--no-publish",
              description: "Keep the HTML report local only; skip publishing it to claude.ai",
              exclusiveOn: ["--publish-report"],
            },
            { name: "--no-scaffold", description: "Explicitly skip scaffold_script" },
            {
              name: "--output-dir",
              description: "Directory for aggregate-result.json",
              args: { name: "dir", generators: folders },
            },
            {
              name: "--publish-report",
              description: "Also require publishing the report to claude.ai",
              exclusiveOn: ["--no-publish"],
            },
            {
              name: "--report",
              description: "Write the self-contained HTML report to this path",
              args: { name: "path", template: "filepaths" },
            },
            {
              name: "--runs",
              description: "Override per-case runs (default: case.runs ?? 3)",
              args: { name: "n" },
            },
            {
              name: "--scaffold",
              description: "Run each case's scaffold_script (runs author-supplied bash as you)",
              isDangerous: true,
              exclusiveOn: ["--no-scaffold"],
            },
            {
              name: "--tag",
              description: "Filter cases by tag",
              isRepeatable: true,
              args: { name: "tag" },
            },
            {
              name: "--threshold",
              description: "Exit 1 if any case score is below this threshold (default: 1.0)",
              args: { name: "0..1" },
            },
            { name: "--verbose", description: "Log per-message trace events to the debug log" },
          ],
          args: {
            name: "target",
            description: "A path, a plugin name, or a `plugin@marketplace` id",
            isOptional: true,
            generators: pluginGenerator,
          },
        },
        {
          name: ["init", "new"],
          description: "Scaffold a new plugin at ~/.claude/skills/<name>/",
          options: [
            {
              name: "--author",
              description: "Author name (default: git config user.name)",
              args: { name: "name" },
            },
            {
              name: "--author-email",
              description: "Author email (default: git config user.email)",
              args: { name: "email" },
            },
            {
              name: "--description",
              description: "Manifest description",
              args: { name: "text" },
            },
            {
              name: ["-f", "--force"],
              description: "Overwrite an existing .claude-plugin/ at the target",
              isDangerous: true,
            },
            helpOption,
            {
              name: "--with",
              description: "Also scaffold extra components",
              args: {
                name: "components",
                isVariadic: true,
                suggestions: [
                  "skills",
                  "agents",
                  "hooks",
                  "mcp",
                  "lsp",
                  "output-style",
                  "channel",
                ],
              },
            },
          ],
          args: { name: "name" },
        },
        {
          name: ["install", "i"],
          description: "Install a plugin from available marketplaces",
          options: [
            {
              name: "--config",
              description: "Set a userConfig option declared in the plugin's manifest",
              isRepeatable: true,
              args: { name: "key=value" },
            },
            helpOption,
            jsonResultOption,
            pluginScopeOption("Installation scope: user, project, or local (default: user)"),
            {
              name: ["-y", "--yes"],
              description: "Accept the displayed marketplace-declared command without a prompt",
            },
          ],
          args: {
            name: "plugin",
            description: "Plugin name, or plugin@marketplace for a specific marketplace",
            generators: pluginGenerator,
          },
        },
        {
          name: "list",
          description: "List installed plugins",
          options: [
            {
              name: "--available",
              description: "Include available plugins from marketplaces (requires --json)",
              dependsOn: ["--json"],
            },
            helpOption,
            { name: "--json", description: "Output as JSON" },
          ],
        },
        {
          name: "marketplace",
          description: "Manage Claude Code marketplaces",
          subcommands: [
            {
              name: "add",
              description: "Add a marketplace from a URL, path, or GitHub repo",
              options: [helpOption],
              args: { name: "source" },
            },
            {
              name: "list",
              description: "List all configured marketplaces",
              options: [helpOption],
            },
            {
              name: ["remove", "rm"],
              description: "Remove a configured marketplace",
              isDangerous: true,
              options: [helpOption],
              args: { name: "name", generators: marketplaceGenerator },
            },
            {
              name: "update",
              description: "Update marketplace(s) from their source - updates all if no name given",
              options: [helpOption],
              args: { name: "name", isOptional: true, generators: marketplaceGenerator },
            },
            {
              name: "help",
              description: "Display help for command",
              args: { name: "command", isOptional: true },
            },
          ],
          options: [helpOption],
        },
        {
          name: ["prune", "autoremove"],
          description: "Remove auto-installed dependencies that are no longer needed",
          options: [
            { name: "--dry-run", description: "List what would be removed without removing" },
            helpOption,
            pluginScopeOption("Prune at scope: user, project, or local (default: user)"),
            { name: ["-y", "--yes"], description: "Skip the confirmation prompt" },
          ],
        },
        {
          name: "tag",
          description: "Create a {name}--v{version} git tag for a plugin release",
          options: [
            { name: "--dry-run", description: "Print what would be tagged without creating it" },
            {
              name: ["-f", "--force"],
              description: "Skip the dirty-working-tree and tag-already-exists checks",
              isDangerous: true,
            },
            helpOption,
            {
              name: ["-m", "--message"],
              description: "Tag annotation message (use %s for the version)",
              args: { name: "msg" },
            },
            { name: "--push", description: "Push the tag to --remote after creating it" },
            {
              name: "--remote",
              description: "Remote to push to with --push (default: origin)",
              args: { name: "name" },
            },
          ],
          args: { name: "path", isOptional: true, generators: folders },
        },
        {
          name: ["uninstall", "remove"],
          description: "Uninstall an installed plugin",
          isDangerous: true,
          options: [
            helpOption,
            { ...jsonResultOption, exclusiveOn: ["--prune"] },
            {
              name: "--keep-data",
              description: "Preserve the plugin's persistent data directory",
            },
            {
              name: "--prune",
              description: "Also remove auto-installed dependencies that are no longer needed",
              exclusiveOn: ["--json"],
            },
            pluginScopeOption("Uninstall from scope: user, project, or local (default: user)"),
            { name: ["-y", "--yes"], description: "Skip the --prune confirmation prompt" },
          ],
          args: { name: "plugin", generators: pluginGenerator },
        },
        {
          name: "update",
          description: "Update a plugin to the latest version (restart required to apply)",
          options: [
            helpOption,
            jsonResultOption,
            pluginScopeOption("Installation scope: user, project, local, managed", true),
            {
              name: ["-y", "--yes"],
              description: "Accept the displayed marketplace-declared command without a prompt",
            },
          ],
          args: { name: "plugin", generators: pluginGenerator },
        },
        {
          name: "validate",
          description: "Validate a plugin or marketplace manifest, or a directory of components",
          options: [
            helpOption,
            { name: "--json", description: "Output the validation report as JSON" },
            { name: "--strict", description: "Treat warnings as errors (exit 1)" },
          ],
          args: { name: "path", template: "filepaths" },
        },
        {
          name: "help",
          description: "Display help for command",
          args: { name: "command", isOptional: true },
        },
      ],
      options: [helpOption],
    },
    {
      name: "project",
      description: "Manage Claude Code project state",
      subcommands: [
        {
          name: "purge",
          description: "Delete all Claude Code state for a project",
          isDangerous: true,
          options: [
            {
              name: "--all",
              description: "Purge state for every project (mutually exclusive with [path])",
            },
            { name: "--dry-run", description: "List what would be deleted without deleting" },
            helpOption,
            {
              name: ["-i", "--interactive"],
              description: "Prompt for each item before deleting",
            },
            { name: ["-y", "--yes"], description: "Skip confirmation prompt" },
          ],
          args: { name: "path", isOptional: true, generators: folders },
        },
        {
          name: "help",
          description: "Display help for command",
          args: { name: "command", isOptional: true },
        },
      ],
      options: [helpOption],
    },
    {
      name: "respawn",
      description: "Restart a background session so it runs the current Claude Code version",
      options: [{ name: "--all", description: "Respawn every background session" }],
      args: { name: "id", isOptional: true, generators: backgroundSessionGenerator },
    },
    {
      name: "rm",
      description: "Delete a background session, and its worktree when that is safe",
      isDangerous: true,
      options: [
        {
          name: "--discard-unpushed",
          description: "Also discard the worktree's unpushed commits and uncommitted changes",
          isDangerous: true,
          args: { name: "commit@worktree-id" },
        },
        {
          name: "--force-remove-worktree",
          description: "Delete the worktree directory even though it could not be removed",
          isDangerous: true,
          args: { name: "worktree-id" },
        },
      ],
      args: { name: "id", generators: backgroundSessionGenerator },
    },
    {
      name: "setup-token",
      description: "Set up a long-lived authentication token (requires Claude subscription)",
      options: [helpOption],
    },
    {
      name: ["stop", "kill"],
      description: "Stop a background session, keeping its conversation",
      args: { name: "id", generators: backgroundSessionGenerator },
    },
    {
      name: "ultrareview",
      description: "Run a cloud-hosted multi-agent code review and print the findings",
      options: [
        helpOption,
        {
          name: "--json",
          description: "Print the raw bugs.json payload instead of formatted findings",
        },
        {
          name: "--no-post",
          description: "Do not post the findings to the PR (the default)",
          exclusiveOn: ["--post"],
        },
        {
          name: "--post",
          description: "Post the finished review's findings to the PR as you (PR targets only)",
          exclusiveOn: ["--no-post"],
        },
        {
          name: "--timeout",
          description: "Maximum minutes to wait for the review to finish (default: 45)",
          args: { name: "minutes" },
        },
      ],
      args: {
        name: "target",
        description: "A PR number or a base branch; defaults to the current branch",
        isOptional: true,
      },
    },
    {
      name: ["update", "upgrade"],
      description: "Check for updates and install if available",
      options: [helpOption],
    },
  ],
  options: [
    {
      name: "--add-dir",
      description: "Additional directories to allow tool access to",
      args: { name: "directories", isVariadic: true, generators: folders },
    },
    {
      name: "--agent",
      description: "Agent for the current session. Overrides the 'agent' setting",
      args: { name: "agent", generators: agentGenerator },
    },
    {
      name: "--agents",
      description: "JSON object defining custom agents",
      args: { name: "json" },
    },
    {
      name: "--allow-dangerously-skip-permissions",
      description: "Enable bypassing all permission checks as an option, not by default",
    },
    {
      name: ["--allowedTools", "--allowed-tools"],
      description: "Comma or space-separated list of tool names to allow",
      args: { name: "tools", isVariadic: true },
    },
    {
      name: "--append-system-prompt",
      description: "Append a system prompt to the default system prompt",
      args: { name: "prompt" },
    },
    {
      name: "--autocompact",
      description: "Auto-compact window size (auto, or 100k-1M tokens)",
      args: {
        name: "auto|tokens",
        suggestions: [{ name: "auto", description: "Let Claude Code pick the window" }],
      },
    },
    {
      name: "--ax-screen-reader",
      description: "Render screen-reader friendly output",
    },
    {
      name: ["--bg", "--background"],
      description: "Start the session in the background and return immediately",
    },
    {
      name: "--bare",
      description: "Minimal mode: skip hooks, LSP, plugin sync, auto-memory and CLAUDE.md discovery",
    },
    {
      name: "--betas",
      description: "Beta headers to include in API requests (API key users only)",
      args: { name: "betas", isVariadic: true },
    },
    {
      name: "--brief",
      description: "Enable SendUserMessage tool for agent-to-user communication",
    },
    {
      name: "--chrome",
      description: "Enable Claude in Chrome integration",
      exclusiveOn: ["--no-chrome"],
    },
    {
      name: "--cloud",
      description: "Create a cloud session, or attach to one by session ID or claude.ai/code URL",
      args: { name: "description|session_id|url", isOptional: true },
    },
    {
      name: ["-c", "--continue"],
      description: "Continue the most recent conversation in the current directory",
      exclusiveOn: ["-r", "--resume"],
    },
    {
      name: "--dangerously-skip-permissions",
      description: "Bypass all permission checks",
      isDangerous: true,
    },
    {
      name: ["-d", "--debug"],
      description: "Enable debug mode with optional category filtering",
      args: { name: "filter", isOptional: true },
    },
    {
      name: "--debug-file",
      description: "Write debug logs to a specific file path (implicitly enables debug mode)",
      args: { name: "path", template: "filepaths" },
    },
    { name: "--disable-slash-commands", description: "Disable all skills" },
    {
      name: ["--disallowedTools", "--disallowed-tools"],
      description: "Comma or space-separated list of tool names to deny",
      args: { name: "tools", isVariadic: true },
    },
    effortOption,
    {
      name: "--environment",
      description: "Create a cloud session on the given self-hosted environment (ccpool_...)",
      args: { name: "environment_id" },
    },
    {
      name: "--exclude-dynamic-system-prompt-sections",
      description: "Move per-machine system prompt sections into the first user message",
    },
    {
      name: "--fallback-model",
      description: "Fall back to these models when the default one is overloaded",
      args: { name: "model" },
    },
    {
      name: "--file",
      description: "File resources to download at startup (file_id:relative_path)",
      args: { name: "specs", isVariadic: true },
    },
    {
      name: "--fork-session",
      description: "When resuming, create a new session ID instead of reusing the original",
    },
    {
      name: "--forward-subagent-text",
      description: "Forward subagent text and thinking blocks as assistant/user messages",
    },
    {
      name: "--from-pr",
      description: "Resume a session linked to a PR by number/URL, or open the picker",
      args: { name: "value", isOptional: true },
    },
    helpOption,
    {
      name: "--ide",
      description: "Automatically connect to IDE on startup if exactly one is available",
    },
    {
      name: "--include-hook-events",
      description: "Include all hook lifecycle events in the output stream",
    },
    {
      name: "--include-partial-messages",
      description: "Include partial message chunks as they arrive",
    },
    {
      name: "--input-format",
      description: "Input format (only works with --print)",
      dependsOn: ["--print"],
      args: {
        name: "format",
        suggestions: [
          { name: "text", description: "Plain text (default)" },
          { name: "stream-json", description: "Realtime streaming input" },
        ],
      },
    },
    {
      name: "--json-schema",
      description: "JSON Schema for structured output validation",
      args: { name: "schema" },
    },
    {
      name: "--max-budget-usd",
      description: "Maximum dollar amount to spend on API calls (only works with --print)",
      dependsOn: ["--print"],
      args: { name: "amount" },
    },
    {
      name: "--mcp-config",
      description: "Load MCP servers from JSON files or strings (space-separated)",
      args: { name: "configs", isVariadic: true, template: "filepaths" },
    },
    modelOption,
    {
      name: ["-n", "--name"],
      description: "Set a display name for this session",
      args: { name: "name" },
    },
    {
      name: "--no-chrome",
      description: "Disable Claude in Chrome integration",
      exclusiveOn: ["--chrome"],
    },
    {
      name: "--no-session-persistence",
      description: "Disable session persistence - sessions cannot be resumed",
      dependsOn: ["--print"],
    },
    {
      name: "--output-format",
      description: "Output format (only works with --print)",
      dependsOn: ["--print"],
      args: {
        name: "format",
        suggestions: [
          { name: "text", description: "Plain text (default)" },
          { name: "json", description: "A single result object" },
          { name: "stream-json", description: "Realtime streaming" },
        ],
      },
    },
    permissionModeOption,
    {
      name: "--permission-prompts",
      description: "Who answers permission prompts with --print",
      args: {
        name: "target",
        suggestions: [
          { name: "host", description: "The SDK host or --permission-prompt-tool (default)" },
          { name: "none", description: "Nobody: anything that would prompt is denied" },
        ],
      },
    },
    {
      name: "--plugin-dir",
      description: "Load a plugin from a directory or .zip for this session only",
      isRepeatable: true,
      args: { name: "path", template: "filepaths" },
    },
    {
      name: "--plugin-url",
      description: "Fetch a plugin .zip from a URL for this session only",
      isRepeatable: true,
      args: { name: "url" },
    },
    {
      name: ["-p", "--print"],
      description: "Print response and exit (useful for pipes)",
    },
    {
      name: "--prompt-suggestions",
      description: "Enable prompt suggestions",
      args: {
        name: "value",
        isOptional: true,
        suggestions: ["true", "false", "1", "0", "yes", "no", "on", "off"],
      },
    },
    {
      name: "--remote-control",
      description: "Start an interactive session with Remote Control enabled",
      args: { name: "name", isOptional: true },
    },
    {
      name: "--remote-control-session-name-prefix",
      description: "Prefix for auto-generated Remote Control session names (default: hostname)",
      args: { name: "prefix" },
    },
    {
      name: "--replay-user-messages",
      description: "Re-emit user messages from stdin back on stdout for acknowledgment",
    },
    {
      name: "--restricted",
      description: "Restricted mode: no command-running tools, and user settings are ignored",
    },
    {
      name: ["-r", "--resume"],
      description: "Resume a conversation by session ID, or open the picker",
      exclusiveOn: ["-c", "--continue"],
      args: { name: "value", isOptional: true, generators: sessionIdGenerator },
    },
    {
      name: "--safe-mode",
      description: "Start with all customizations disabled - useful for troubleshooting",
    },
    {
      name: "--session-id",
      description: "Use a specific session ID for the conversation (must be a valid UUID)",
      args: { name: "uuid" },
    },
    settingSourcesOption,
    {
      name: "--settings",
      description: "Path to a settings JSON file or a JSON string of additional settings",
      args: { name: "file-or-json", template: "filepaths" },
    },
    {
      name: "--strict-mcp-config",
      description: "Only use MCP servers from --mcp-config, ignoring all other configurations",
    },
    {
      name: "--system-prompt",
      description: "System prompt to use for the session",
      args: { name: "prompt" },
    },
    {
      name: "--system-prompt-snapshot",
      description: "Record the system prompt once per conversation and reuse it verbatim",
      args: {
        name: "on|off",
        suggestions: [
          { name: "on", description: "Record on the first request and reuse (default)" },
          { name: "off", description: "Render the prompt fresh on every request" },
        ],
      },
    },
    {
      name: "--teleport",
      description: "Resume a teleport session, optionally specify session ID",
      args: { name: "session", isOptional: true },
    },
    {
      name: "--tmux",
      description: "Create a tmux session for the worktree (requires --worktree)",
      dependsOn: ["--worktree"],
    },
    {
      name: "--tools",
      description: 'Available tools from the built-in set ("" for none, "default" for all)',
      args: { name: "tools", isVariadic: true },
    },
    { name: "--verbose", description: "Override verbose mode setting from config" },
    { name: ["-v", "--version"], description: "Output the version number" },
    {
      name: ["-w", "--worktree"],
      description: "Create a new git worktree for this session",
      args: { name: "name", isOptional: true },
    },
  ],
  args: {
    name: "prompt",
    description: "Your prompt",
    isOptional: true,
  },
};

export default completionSpec;
