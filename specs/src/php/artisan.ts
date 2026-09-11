// `php artisan list --format=json`, limited to the fields this spec reads
interface Argument {
  name: string;
  description: string;
  is_required: boolean;
}

interface Option {
  name: string;
  shortcut: string;
  description: string;
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
  name: "artisan",
  description: "Laravel Artisan Command",
  generateSpec: async (_, executeShellCommand) => {
    var { stdout } = await executeShellCommand({
      command: "php",
      args: ["artisan", "list", "--format=json"],
    });
    const subcommands: Fig.Subcommand[] = [];

    try {
      const commandDefinition: CommandList = JSON.parse(stdout);

      commandDefinition.commands
        .filter((command) => command.name !== "_complete")
        .map((command) => {
          subcommands.push({
            name: command.name,
            description: command.description,
            icon: "https://web.tinkerwell.app/img/laravel.3cab6a56.png",
            args: Object.keys(command.definition.arguments).map(
              (argumentKey) => {
                const argument = command.definition.arguments[argumentKey];

                return {
                  name: argument.name,
                  description: argument.description,
                  isOptional: !argument.is_required,
                };
              }
            ),
            options: Object.keys(command.definition.options).map(
              (optionKey) => {
                const option = command.definition.options[optionKey];
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
    } catch {
      //
    }

    return {
      name: "artisan",
      debounce: true,
      subcommands,
    };
  },
};

export default completionSpec;
