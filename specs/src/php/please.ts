interface Argument {
  name: string;
  is_required: boolean;
  is_array: boolean;
  description: string;
  default: string;
}

interface Option {
  name: string;
  shortcut: string;
  accept_value: boolean;
  is_value_required: boolean;
  is_multiple: false;
  description: string;
  default: string;
}

interface Command {
  name: string;
  description: string;
  definition: {
    arguments: Record<string, Argument>;
    options: Record<string, Option>;
  };
}

interface CommandList {
  commands: Command[];
}

const completionSpec: Fig.Spec = {
  name: "please",
  description: "Statamic Please command",
  generateSpec: async (tokens, executeShellCommand) => {
    const { stdout } = await executeShellCommand({
      command: "php",
      args: ["please", "list", "--format=json"],
    });
    const subcommands: Fig.Subcommand[] = [];

    try {
      const commandDefinition: CommandList = JSON.parse(stdout);

      commandDefinition.commands.map((command) => {
        subcommands.push({
          name: command.name,
          description: command.description,

          args: Object.values(command.definition.arguments).map(
            (argument) => {
              return {
                name: argument.name,
                description: argument.description,
                isOptional: !argument.is_required,
              };
            }
          ),
          options: Object.values(command.definition.options).map(
            (option) => {
              const names = [option.name];

              if (option.shortcut !== "") {
                names.push(option.shortcut);
              }

              return {
                name: names,
                description: option.description,
              };
            }
          ),
        });
      });
    } catch (err) {
      console.error(err);
    }

    return {
      name: "please",
      debounce: true,
      subcommands,
    };
  },
};

export default completionSpec;
