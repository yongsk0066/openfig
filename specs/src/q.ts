const completion: Fig.Spec = {
  name: "q",
  description: "The CLI subcommands",
  subcommands: [
    {
      name: "hook",
      description: "Hook commands",
      hidden: true,
      subcommands: [
        {
          name: "clear-autocomplete-cache",
          options: [
            {
              name: "--cli",
              isRepeatable: true,
              args: {
                name: "cli",
                isOptional: true,
              },
            },
            {
              name: ["-v", "--verbose"],
              description: "Increase logging verbosity",
              isRepeatable: true,
            },
            {
              name: ["-h", "--help"],
              description: "Print help",
            },
          ],
        },
        {
          name: "help",
          description: "Print this message or the help of the given subcommand(s)",
          subcommands: [
            {
              name: "clear-autocomplete-cache",
            },
            {
              name: "help",
              description: "Print this message or the help of the given subcommand(s)",
            },
          ],
        },
      ],
      options: [
        {
          name: ["-v", "--verbose"],
          description: "Increase logging verbosity",
          isRepeatable: true,
        },
        {
          name: ["-h", "--help"],
          description: "Print help",
        },
      ],
    },
    {
      name: "debug",
      description: "Debug the app",
      subcommands: [
        {
          name: "app",
          description: "Debug the app",
          options: [
            {
              name: ["-v", "--verbose"],
              description: "Increase logging verbosity",
              isRepeatable: true,
            },
            {
              name: ["-h", "--help"],
              description: "Print help",
            },
          ],
        },
        {
          name: "autocomplete-window",
          description: "Toggle/set autocomplete window debug mode",
          options: [
            {
              name: ["-v", "--verbose"],
              description: "Increase logging verbosity",
              isRepeatable: true,
            },
            {
              name: ["-h", "--help"],
              description: "Print help",
            },
          ],
          args: {
            name: "mode",
            isOptional: true,
            suggestions: [
              "on",
              "off",
            ],
          },
        },
        {
          name: "logs",
          description: "Show debug logs",
          options: [
            {
              name: "--level",
              isRepeatable: true,
              args: {
                name: "level",
                isOptional: true,
              },
            },
            {
              name: ["-v", "--verbose"],
              description: "Increase logging verbosity",
              isRepeatable: true,
            },
            {
              name: ["-h", "--help"],
              description: "Print help",
            },
          ],
          args: {
            name: "files",
            isVariadic: true,
            isOptional: true,
          },
        },
        {
          name: "prompt-accessibility",
          description: "Prompt accessibility",
          options: [
            {
              name: ["-v", "--verbose"],
              description: "Increase logging verbosity",
              isRepeatable: true,
            },
            {
              name: ["-h", "--help"],
              description: "Print help",
            },
          ],
        },
        {
          name: "sample",
          description: "Sample desktop process",
          options: [
            {
              name: ["-v", "--verbose"],
              description: "Increase logging verbosity",
              isRepeatable: true,
            },
            {
              name: ["-h", "--help"],
              description: "Print help",
            },
          ],
        },
        {
          name: "verify-codesign",
          description: "Debug application codesigning",
          options: [
            {
              name: ["-v", "--verbose"],
              description: "Increase logging verbosity",
              isRepeatable: true,
            },
            {
              name: ["-h", "--help"],
              description: "Print help",
            },
          ],
        },
        {
          name: "accessibility",
          description: "Accessibility",
          options: [
            {
              name: ["-v", "--verbose"],
              description: "Increase logging verbosity",
              isRepeatable: true,
            },
            {
              name: ["-h", "--help"],
              description: "Print help",
            },
          ],
          args: {
            name: "action",
            isOptional: true,
            suggestions: [
              "refresh",
              "reset",
              "prompt",
              "open",
              "status",
            ],
          },
        },
        {
          name: "key-tester",
          description: "Key Tester",
          options: [
            {
              name: ["-v", "--verbose"],
              description: "Increase logging verbosity",
              isRepeatable: true,
            },
            {
              name: ["-h", "--help"],
              description: "Print help",
            },
          ],
        },
        {
          name: "diagnostics",
          description: "Watches diagnostics",
          options: [
            {
              name: "--rate",
              isRepeatable: true,
              args: {
                name: "rate",
                isOptional: true,
              },
            },
            {
              name: "--watch",
            },
            {
              name: ["-v", "--verbose"],
              description: "Increase logging verbosity",
              isRepeatable: true,
            },
            {
              name: ["-h", "--help"],
              description: "Print help",
            },
          ],
        },
        {
          name: "devtools",
          description: "Open up the devtools of a specific webview",
          options: [
            {
              name: ["-v", "--verbose"],
              description: "Increase logging verbosity",
              isRepeatable: true,
            },
            {
              name: ["-h", "--help"],
              description: "Print help",
            },
          ],
          args: {
            name: "app",
            suggestions: [
              "dashboard",
              "autocomplete",
            ],
          },
        },
        {
          name: "shell",
          description: "Disables sourcing of user shell config and instead uses a minimal shell config",
          options: [
            {
              name: ["-v", "--verbose"],
              description: "Increase logging verbosity",
              isRepeatable: true,
            },
            {
              name: ["-h", "--help"],
              description: "Print help",
            },
          ],
        },
        {
          name: "fix-permissions",
          description: "Update the shell config permissions to have the correct owner and access rights",
          options: [
            {
              name: ["-v", "--verbose"],
              description: "Increase logging verbosity",
              isRepeatable: true,
            },
            {
              name: ["-h", "--help"],
              description: "Print help",
            },
          ],
        },
        {
          name: "help",
          description: "Print this message or the help of the given subcommand(s)",
          subcommands: [
            {
              name: "app",
              description: "Debug the app",
            },
            {
              name: "autocomplete-window",
              description: "Toggle/set autocomplete window debug mode",
            },
            {
              name: "logs",
              description: "Show debug logs",
            },
            {
              name: "prompt-accessibility",
              description: "Prompt accessibility",
            },
            {
              name: "sample",
              description: "Sample desktop process",
            },
            {
              name: "verify-codesign",
              description: "Debug application codesigning",
            },
            {
              name: "accessibility",
              description: "Accessibility",
            },
            {
              name: "key-tester",
              description: "Key Tester",
            },
            {
              name: "diagnostics",
              description: "Watches diagnostics",
            },
            {
              name: "devtools",
              description: "Open up the devtools of a specific webview",
            },
            {
              name: "shell",
              description: "Disables sourcing of user shell config and instead uses a minimal shell config",
            },
            {
              name: "fix-permissions",
              description: "Update the shell config permissions to have the correct owner and access rights",
            },
            {
              name: "help",
              description: "Print this message or the help of the given subcommand(s)",
            },
          ],
        },
      ],
      options: [
        {
          name: ["-v", "--verbose"],
          description: "Increase logging verbosity",
          isRepeatable: true,
        },
        {
          name: ["-h", "--help"],
          description: "Print help",
        },
      ],
    },
    {
      name: ["settings", "setting"],
      description: "Customize appearance & behavior",
      subcommands: [
        {
          name: "open",
          description: "Open the settings file",
          options: [
            {
              name: ["-v", "--verbose"],
              description: "Increase logging verbosity",
              isRepeatable: true,
            },
            {
              name: ["-h", "--help"],
              description: "Print help",
            },
          ],
        },
        {
          name: "help",
          description: "Print this message or the help of the given subcommand(s)",
          subcommands: [
            {
              name: "open",
              description: "Open the settings file",
            },
            {
              name: "help",
              description: "Print this message or the help of the given subcommand(s)",
            },
          ],
        },
      ],
      options: [
        {
          name: ["-f", "--format"],
          description: "Format of the output",
          isRepeatable: true,
          args: {
            name: "format",
            isOptional: true,
            suggestions: [
              {
                name: "plain",
                description: "Outputs the results as markdown",
              },
              {
                name: "json",
                description: "Outputs the results as JSON",
              },
              {
                name: "json-pretty",
                description: "Outputs the results as pretty print JSON",
              },
            ],
          },
        },
        {
          name: ["-d", "--delete"],
          description: "Delete a key (No value needed)",
        },
        {
          name: ["-v", "--verbose"],
          description: "Increase logging verbosity",
          isRepeatable: true,
        },
        {
          name: ["-h", "--help"],
          description: "Print help (see more with '--help')",
        },
      ],
      args: [
        {
          name: "key",
          isOptional: true,
        },
        {
          name: "value",
          isOptional: true,
        },
      ]
    },
    {
      name: ["setup", "install"],
      description: "Setup cli components",
      options: [
        {
          name: "--dotfiles",
          description: "Install only the shell integrations",
        },
        {
          name: "--no-confirm",
          description: "Don't confirm automatic installation",
        },
        {
          name: "--force",
          description: "Force installation",
        },
        {
          name: "--global",
          description: "Install globally",
        },
        {
          name: ["-v", "--verbose"],
          description: "Increase logging verbosity",
          isRepeatable: true,
        },
        {
          name: ["-h", "--help"],
          description: "Print help",
        },
      ],
    },
    {
      name: "uninstall",
      description: "Uninstall the desktop app and its integrations",
      hidden: true,
      options: [
        {
          name: ["-y", "--no-confirm"],
          description: "Force uninstall",
        },
        {
          name: ["-v", "--verbose"],
          description: "Increase logging verbosity",
          isRepeatable: true,
        },
        {
          name: ["-h", "--help"],
          description: "Print help",
        },
      ],
    },
    {
      name: ["diagnostic", "diagnostics"],
      description: "Run diagnostic tests",
      options: [
        {
          name: ["-f", "--format"],
          description: "The format of the output",
          isRepeatable: true,
          args: {
            name: "format",
            isOptional: true,
            suggestions: [
              {
                name: "plain",
                description: "Outputs the results as markdown",
              },
              {
                name: "json",
                description: "Outputs the results as JSON",
              },
              {
                name: "json-pretty",
                description: "Outputs the results as pretty print JSON",
              },
            ],
          },
        },
        {
          name: "--force",
          description: "Force limited diagnostic output",
        },
        {
          name: ["-v", "--verbose"],
          description: "Increase logging verbosity",
          isRepeatable: true,
        },
        {
          name: ["-h", "--help"],
          description: "Print help (see more with '--help')",
        },
      ],
    },
    {
      name: "init",
      description: "Generate the dotfiles for the given shell",
      options: [
        {
          name: "--rcfile",
          isRepeatable: true,
          args: {
            name: "rcfile",
            isOptional: true,
          },
        },
        {
          name: ["-v", "--verbose"],
          description: "Increase logging verbosity",
          isRepeatable: true,
        },
        {
          name: ["-h", "--help"],
          description: "Print help (see more with '--help')",
        },
      ],
      args: [
        {
          name: "shell",
          suggestions: [
            {
              name: "bash",
              description: "Bash shell",
            },
            {
              name: "zsh",
              description: "Zsh shell",
            },
          ],
        },
        {
          name: "when",
          suggestions: [
            "pre",
            "post",
          ],
        },
      ]
    },
    {
      name: "theme",
      description: "Get or set theme",
      options: [
        {
          name: "--list",
          exclusiveOn: [
            "--folder",
          ],
        },
        {
          name: "--folder",
          exclusiveOn: [
            "--list",
          ],
        },
        {
          name: ["-v", "--verbose"],
          description: "Increase logging verbosity",
          isRepeatable: true,
        },
        {
          name: ["-h", "--help"],
          description: "Print help",
        },
      ],
      args: {
        name: "theme",
        isOptional: true,
      },
    },
    {
      name: "doctor",
      description: "Fix and diagnose common issues",
      options: [
        {
          name: ["-a", "--all"],
          description: "Run all doctor tests, with no fixes",
        },
        {
          name: ["-s", "--strict"],
          description: "Error on warnings",
        },
        {
          name: ["-v", "--verbose"],
          description: "Increase logging verbosity",
          isRepeatable: true,
        },
        {
          name: ["-h", "--help"],
          description: "Print help",
        },
      ],
    },
    {
      name: "completion",
      description: "Generate CLI completion spec",
      hidden: true,
      options: [
        {
          name: ["-v", "--verbose"],
          description: "Increase logging verbosity",
          isRepeatable: true,
        },
        {
          name: ["-h", "--help"],
          description: "Print help (see more with '--help')",
        },
      ],
      args: {
        name: "shell",
        isOptional: true,
        suggestions: [
          {
            name: "bash",
            description: "Bash shell completions",
          },
          {
            name: "zsh",
            description: "Zsh shell completions",
          },
          {
            name: "fig",
            description: "Fig completion spec",
          },
        ],
      },
    },
    {
      name: ["internal", "_"],
      description: "Internal subcommands",
      hidden: true,
      subcommands: [
        {
          name: "pre-cmd",
          description: "Command that is run during the PreCmd section of the shell integrations",
          options: [
            {
              name: "--alias",
              isRepeatable: true,
              args: {
                name: "alias",
                isOptional: true,
              },
            },
            {
              name: ["-v", "--verbose"],
              description: "Increase logging verbosity",
              isRepeatable: true,
            },
            {
              name: ["-h", "--help"],
              description: "Print help",
            },
          ],
        },
        {
          name: "local-state",
          description: "Change the local-state file",
          subcommands: [
            {
              name: "init",
              description: "Reload the state listener",
              options: [
                {
                  name: ["-v", "--verbose"],
                  description: "Increase logging verbosity",
                  isRepeatable: true,
                },
                {
                  name: ["-h", "--help"],
                  description: "Print help",
                },
              ],
            },
            {
              name: "all",
              description: "List all the settings",
              options: [
                {
                  name: ["-f", "--format"],
                  isRepeatable: true,
                  args: {
                    name: "format",
                    isOptional: true,
                    suggestions: [
                      {
                        name: "plain",
                        description: "Outputs the results as markdown",
                      },
                      {
                        name: "json",
                        description: "Outputs the results as JSON",
                      },
                      {
                        name: "json-pretty",
                        description: "Outputs the results as pretty print JSON",
                      },
                    ],
                  },
                },
                {
                  name: ["-v", "--verbose"],
                  description: "Increase logging verbosity",
                  isRepeatable: true,
                },
                {
                  name: ["-h", "--help"],
                  description: "Print help (see more with '--help')",
                },
              ],
            },
            {
              name: "help",
              description: "Print this message or the help of the given subcommand(s)",
              subcommands: [
                {
                  name: "init",
                  description: "Reload the state listener",
                },
                {
                  name: "all",
                  description: "List all the settings",
                },
                {
                  name: "help",
                  description: "Print this message or the help of the given subcommand(s)",
                },
              ],
            },
          ],
          options: [
            {
              name: ["-f", "--format"],
              description: "Format of the output",
              isRepeatable: true,
              args: {
                name: "format",
                isOptional: true,
                suggestions: [
                  {
                    name: "plain",
                    description: "Outputs the results as markdown",
                  },
                  {
                    name: "json",
                    description: "Outputs the results as JSON",
                  },
                  {
                    name: "json-pretty",
                    description: "Outputs the results as pretty print JSON",
                  },
                ],
              },
            },
            {
              name: ["-d", "--delete"],
              description: "Delete the state",
            },
            {
              name: ["-v", "--verbose"],
              description: "Increase logging verbosity",
              isRepeatable: true,
            },
            {
              name: ["-h", "--help"],
              description: "Print help (see more with '--help')",
            },
          ],
          args: [
            {
              name: "key",
              isOptional: true,
            },
            {
              name: "value",
              isOptional: true,
            },
          ]
        },
        {
          name: "install",
          description: "Install the cli",
          options: [
            {
              name: "--dotfiles",
              description: "Install only the shell integrations",
            },
            {
              name: "--no-confirm",
              description: "Don't confirm automatic installation",
            },
            {
              name: "--force",
              description: "Force installation",
            },
            {
              name: "--global",
              description: "Install globally",
            },
            {
              name: ["-v", "--verbose"],
              description: "Increase logging verbosity",
              isRepeatable: true,
            },
            {
              name: ["-h", "--help"],
              description: "Print help",
            },
          ],
        },
        {
          name: "get-shell",
          options: [
            {
              name: ["-v", "--verbose"],
              description: "Increase logging verbosity",
              isRepeatable: true,
            },
            {
              name: ["-h", "--help"],
              description: "Print help",
            },
          ],
        },
        {
          name: "hostname",
          options: [
            {
              name: ["-v", "--verbose"],
              description: "Increase logging verbosity",
              isRepeatable: true,
            },
            {
              name: ["-h", "--help"],
              description: "Print help",
            },
          ],
        },
        {
          name: "should-figterm-launch",
          description: "Detects if Figterm should be launched",
          options: [
            {
              name: ["-v", "--verbose"],
              description: "Increase logging verbosity",
              isRepeatable: true,
            },
            {
              name: ["-h", "--help"],
              description: "Print help (see more with '--help')",
            },
          ],
        },
        {
          name: "sockets-dir",
          options: [
            {
              name: ["-v", "--verbose"],
              description: "Increase logging verbosity",
              isRepeatable: true,
            },
            {
              name: ["-h", "--help"],
              description: "Print help",
            },
          ],
        },
        {
          name: "figterm-socket-path",
          options: [
            {
              name: ["-v", "--verbose"],
              description: "Increase logging verbosity",
              isRepeatable: true,
            },
            {
              name: ["-h", "--help"],
              description: "Print help",
            },
          ],
          args: {
            name: "session_id",
          },
        },
        {
          name: "uuidgen",
          options: [
            {
              name: ["-v", "--verbose"],
              description: "Increase logging verbosity",
              isRepeatable: true,
            },
            {
              name: ["-h", "--help"],
              description: "Print help",
            },
          ],
        },
        {
          name: "help",
          description: "Print this message or the help of the given subcommand(s)",
          subcommands: [
            {
              name: "pre-cmd",
              description: "Command that is run during the PreCmd section of the shell integrations",
            },
            {
              name: "local-state",
              description: "Change the local-state file",
              subcommands: [
                {
                  name: "init",
                  description: "Reload the state listener",
                },
                {
                  name: "all",
                  description: "List all the settings",
                },
              ],
            },
            {
              name: "install",
              description: "Install the cli",
            },
            {
              name: "get-shell",
            },
            {
              name: "hostname",
            },
            {
              name: "should-figterm-launch",
              description: "Detects if Figterm should be launched",
            },
            {
              name: "sockets-dir",
            },
            {
              name: "figterm-socket-path",
            },
            {
              name: "uuidgen",
            },
            {
              name: "help",
              description: "Print this message or the help of the given subcommand(s)",
            },
          ],
        },
      ],
      options: [
        {
          name: ["-v", "--verbose"],
          description: "Increase logging verbosity",
          isRepeatable: true,
        },
        {
          name: ["-h", "--help"],
          description: "Print help",
        },
      ],
    },
    {
      name: "launch",
      description: "Launch the desktop app",
      options: [
        {
          name: ["-v", "--verbose"],
          description: "Increase logging verbosity",
          isRepeatable: true,
        },
        {
          name: ["-h", "--help"],
          description: "Print help",
        },
      ],
    },
    {
      name: "quit",
      description: "Quit the desktop app",
      options: [
        {
          name: ["-v", "--verbose"],
          description: "Increase logging verbosity",
          isRepeatable: true,
        },
        {
          name: ["-h", "--help"],
          description: "Print help",
        },
      ],
    },
    {
      name: "restart",
      description: "Restart the desktop app",
      options: [
        {
          name: ["-v", "--verbose"],
          description: "Increase logging verbosity",
          isRepeatable: true,
        },
        {
          name: ["-h", "--help"],
          description: "Print help",
        },
      ],
      args: {
        name: "process",
        isOptional: true,
        suggestions: [
          {
            name: "app",
            description: "Desktop Process",
          },
        ],
      },
    },
    {
      name: ["integrations", "integration"],
      description: "Manage system integrations",
      subcommands: [
        {
          name: "install",
          subcommands: [
            {
              name: "dotfiles",
              options: [
                {
                  name: ["-v", "--verbose"],
                  description: "Increase logging verbosity",
                  isRepeatable: true,
                },
                {
                  name: ["-h", "--help"],
                  description: "Print help (see more with '--help')",
                },
              ],
              args: {
                name: "shell",
                isOptional: true,
                suggestions: [
                  {
                    name: "bash",
                    description: "Bash shell",
                  },
                  {
                    name: "zsh",
                    description: "Zsh shell",
                  },
                ],
              },
            },
            {
              name: "all",
              options: [
                {
                  name: ["-v", "--verbose"],
                  description: "Increase logging verbosity",
                  isRepeatable: true,
                },
                {
                  name: ["-h", "--help"],
                  description: "Print help",
                },
              ],
            },
            {
              name: "help",
              description: "Print this message or the help of the given subcommand(s)",
              subcommands: [
                {
                  name: "dotfiles",
                },
                {
                  name: "all",
                },
                {
                  name: "help",
                  description: "Print this message or the help of the given subcommand(s)",
                },
              ],
            },
          ],
          options: [
            {
              name: ["-s", "--silent"],
              description: "Suppress status messages",
            },
            {
              name: ["-v", "--verbose"],
              description: "Increase logging verbosity",
              isRepeatable: true,
            },
            {
              name: ["-h", "--help"],
              description: "Print help",
            },
          ],
        },
        {
          name: "uninstall",
          subcommands: [
            {
              name: "dotfiles",
              options: [
                {
                  name: ["-v", "--verbose"],
                  description: "Increase logging verbosity",
                  isRepeatable: true,
                },
                {
                  name: ["-h", "--help"],
                  description: "Print help (see more with '--help')",
                },
              ],
              args: {
                name: "shell",
                isOptional: true,
                suggestions: [
                  {
                    name: "bash",
                    description: "Bash shell",
                  },
                  {
                    name: "zsh",
                    description: "Zsh shell",
                  },
                ],
              },
            },
            {
              name: "all",
              options: [
                {
                  name: ["-v", "--verbose"],
                  description: "Increase logging verbosity",
                  isRepeatable: true,
                },
                {
                  name: ["-h", "--help"],
                  description: "Print help",
                },
              ],
            },
            {
              name: "help",
              description: "Print this message or the help of the given subcommand(s)",
              subcommands: [
                {
                  name: "dotfiles",
                },
                {
                  name: "all",
                },
                {
                  name: "help",
                  description: "Print this message or the help of the given subcommand(s)",
                },
              ],
            },
          ],
          options: [
            {
              name: ["-s", "--silent"],
              description: "Suppress status messages",
            },
            {
              name: ["-v", "--verbose"],
              description: "Increase logging verbosity",
              isRepeatable: true,
            },
            {
              name: ["-h", "--help"],
              description: "Print help",
            },
          ],
        },
        {
          name: "reinstall",
          subcommands: [
            {
              name: "dotfiles",
              options: [
                {
                  name: ["-v", "--verbose"],
                  description: "Increase logging verbosity",
                  isRepeatable: true,
                },
                {
                  name: ["-h", "--help"],
                  description: "Print help (see more with '--help')",
                },
              ],
              args: {
                name: "shell",
                isOptional: true,
                suggestions: [
                  {
                    name: "bash",
                    description: "Bash shell",
                  },
                  {
                    name: "zsh",
                    description: "Zsh shell",
                  },
                ],
              },
            },
            {
              name: "all",
              options: [
                {
                  name: ["-v", "--verbose"],
                  description: "Increase logging verbosity",
                  isRepeatable: true,
                },
                {
                  name: ["-h", "--help"],
                  description: "Print help",
                },
              ],
            },
            {
              name: "help",
              description: "Print this message or the help of the given subcommand(s)",
              subcommands: [
                {
                  name: "dotfiles",
                },
                {
                  name: "all",
                },
                {
                  name: "help",
                  description: "Print this message or the help of the given subcommand(s)",
                },
              ],
            },
          ],
          options: [
            {
              name: ["-s", "--silent"],
              description: "Suppress status messages",
            },
            {
              name: ["-v", "--verbose"],
              description: "Increase logging verbosity",
              isRepeatable: true,
            },
            {
              name: ["-h", "--help"],
              description: "Print help",
            },
          ],
        },
        {
          name: "status",
          subcommands: [
            {
              name: "dotfiles",
              options: [
                {
                  name: ["-v", "--verbose"],
                  description: "Increase logging verbosity",
                  isRepeatable: true,
                },
                {
                  name: ["-h", "--help"],
                  description: "Print help (see more with '--help')",
                },
              ],
              args: {
                name: "shell",
                isOptional: true,
                suggestions: [
                  {
                    name: "bash",
                    description: "Bash shell",
                  },
                  {
                    name: "zsh",
                    description: "Zsh shell",
                  },
                ],
              },
            },
            {
              name: "all",
              options: [
                {
                  name: ["-v", "--verbose"],
                  description: "Increase logging verbosity",
                  isRepeatable: true,
                },
                {
                  name: ["-h", "--help"],
                  description: "Print help",
                },
              ],
            },
            {
              name: "help",
              description: "Print this message or the help of the given subcommand(s)",
              subcommands: [
                {
                  name: "dotfiles",
                },
                {
                  name: "all",
                },
                {
                  name: "help",
                  description: "Print this message or the help of the given subcommand(s)",
                },
              ],
            },
          ],
          options: [
            {
              name: ["-f", "--format"],
              isRepeatable: true,
              args: {
                name: "format",
                isOptional: true,
                suggestions: [
                  {
                    name: "plain",
                    description: "Outputs the results as markdown",
                  },
                  {
                    name: "json",
                    description: "Outputs the results as JSON",
                  },
                  {
                    name: "json-pretty",
                    description: "Outputs the results as pretty print JSON",
                  },
                ],
              },
            },
            {
              name: ["-v", "--verbose"],
              description: "Increase logging verbosity",
              isRepeatable: true,
            },
            {
              name: ["-h", "--help"],
              description: "Print help (see more with '--help')",
            },
          ],
        },
        {
          name: "help",
          description: "Print this message or the help of the given subcommand(s)",
          subcommands: [
            {
              name: "install",
              subcommands: [
                {
                  name: "dotfiles",
                },
                {
                  name: "all",
                },
              ],
            },
            {
              name: "uninstall",
              subcommands: [
                {
                  name: "dotfiles",
                },
                {
                  name: "all",
                },
              ],
            },
            {
              name: "reinstall",
              subcommands: [
                {
                  name: "dotfiles",
                },
                {
                  name: "all",
                },
              ],
            },
            {
              name: "status",
              subcommands: [
                {
                  name: "dotfiles",
                },
                {
                  name: "all",
                },
              ],
            },
            {
              name: "help",
              description: "Print this message or the help of the given subcommand(s)",
            },
          ],
        },
      ],
      options: [
        {
          name: ["-v", "--verbose"],
          description: "Increase logging verbosity",
          isRepeatable: true,
        },
        {
          name: ["-h", "--help"],
          description: "Print help",
        },
      ],
    },
    {
      name: "version",
      description: "Version",
      hidden: true,
      options: [
        {
          name: ["-v", "--verbose"],
          description: "Increase logging verbosity",
          isRepeatable: true,
        },
        {
          name: ["-h", "--help"],
          description: "Print help",
        },
      ],
    },
    {
      name: "dashboard",
      description: "Open the dashboard",
      options: [
        {
          name: ["-v", "--verbose"],
          description: "Increase logging verbosity",
          isRepeatable: true,
        },
        {
          name: ["-h", "--help"],
          description: "Print help",
        },
      ],
    },
    {
      name: "help",
      description: "Print this message or the help of the given subcommand(s)",
      subcommands: [
        {
          name: "hook",
          description: "Hook commands",
          hidden: true,
          subcommands: [
            {
              name: "clear-autocomplete-cache",
            },
          ],
        },
        {
          name: "debug",
          description: "Debug the app",
          subcommands: [
            {
              name: "app",
              description: "Debug the app",
            },
            {
              name: "autocomplete-window",
              description: "Toggle/set autocomplete window debug mode",
            },
            {
              name: "logs",
              description: "Show debug logs",
            },
            {
              name: "prompt-accessibility",
              description: "Prompt accessibility",
            },
            {
              name: "sample",
              description: "Sample desktop process",
            },
            {
              name: "verify-codesign",
              description: "Debug application codesigning",
            },
            {
              name: "accessibility",
              description: "Accessibility",
            },
            {
              name: "key-tester",
              description: "Key Tester",
            },
            {
              name: "diagnostics",
              description: "Watches diagnostics",
            },
            {
              name: "devtools",
              description: "Open up the devtools of a specific webview",
            },
            {
              name: "shell",
              description: "Disables sourcing of user shell config and instead uses a minimal shell config",
            },
            {
              name: "fix-permissions",
              description: "Update the shell config permissions to have the correct owner and access rights",
            },
          ],
        },
        {
          name: "settings",
          description: "Customize appearance & behavior",
          subcommands: [
            {
              name: "open",
              description: "Open the settings file",
            },
          ],
        },
        {
          name: "setup",
          description: "Setup cli components",
        },
        {
          name: "uninstall",
          description: "Uninstall the desktop app and its integrations",
          hidden: true,
        },
        {
          name: "diagnostic",
          description: "Run diagnostic tests",
        },
        {
          name: "init",
          description: "Generate the dotfiles for the given shell",
        },
        {
          name: "theme",
          description: "Get or set theme",
        },
        {
          name: "doctor",
          description: "Fix and diagnose common issues",
        },
        {
          name: "completion",
          description: "Generate CLI completion spec",
          hidden: true,
        },
        {
          name: "internal",
          description: "Internal subcommands",
          hidden: true,
          subcommands: [
            {
              name: "pre-cmd",
              description: "Command that is run during the PreCmd section of the shell integrations",
            },
            {
              name: "local-state",
              description: "Change the local-state file",
              subcommands: [
                {
                  name: "init",
                  description: "Reload the state listener",
                },
                {
                  name: "all",
                  description: "List all the settings",
                },
              ],
            },
            {
              name: "install",
              description: "Install the cli",
            },
            {
              name: "get-shell",
            },
            {
              name: "hostname",
            },
            {
              name: "should-figterm-launch",
              description: "Detects if Figterm should be launched",
            },
            {
              name: "sockets-dir",
            },
            {
              name: "figterm-socket-path",
            },
            {
              name: "uuidgen",
            },
          ],
        },
        {
          name: "launch",
          description: "Launch the desktop app",
        },
        {
          name: "quit",
          description: "Quit the desktop app",
        },
        {
          name: "restart",
          description: "Restart the desktop app",
        },
        {
          name: "integrations",
          description: "Manage system integrations",
          subcommands: [
            {
              name: "install",
              subcommands: [
                {
                  name: "dotfiles",
                },
                {
                  name: "all",
                },
              ],
            },
            {
              name: "uninstall",
              subcommands: [
                {
                  name: "dotfiles",
                },
                {
                  name: "all",
                },
              ],
            },
            {
              name: "reinstall",
              subcommands: [
                {
                  name: "dotfiles",
                },
                {
                  name: "all",
                },
              ],
            },
            {
              name: "status",
              subcommands: [
                {
                  name: "dotfiles",
                },
                {
                  name: "all",
                },
              ],
            },
          ],
        },
        {
          name: "version",
          description: "Version",
          hidden: true,
        },
        {
          name: "dashboard",
          description: "Open the dashboard",
        },
        {
          name: "help",
          description: "Print this message or the help of the given subcommand(s)",
        },
      ],
    },
  ],
  options: [
    {
      name: ["-v", "--verbose"],
      description: "Increase logging verbosity",
      isRepeatable: true,
    },
    {
      name: "--help-all",
      description: "Print help for all subcommands",
    },
    {
      name: ["-h", "--help"],
      description: "Print help",
    },
    {
      name: ["-V", "--version"],
      description: "Print version",
    },
  ],
};

export default completion;

