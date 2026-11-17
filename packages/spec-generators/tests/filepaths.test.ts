import { describe, expect, it, vi } from "vitest";
import {
  filepaths,
  folders,
  type FilepathsOptions,
  getCurrentInsertedDirectory,
  sortFilesAlphabetically,
} from "../src/filepaths";

const defaultHome = "/home/user";
const defaultCwd = `${defaultHome}/current_cwd`;

const defaultContext: Fig.GeneratorContext = {
  searchTerm: "",
  currentWorkingDirectory: defaultCwd,
  currentProcess: "zsh",
  sshPrefix: "",
  environmentVariables: {
    HOME: defaultHome,
  },
};

const defaultContextWithEnv = (env: Record<string, string>) => ({
  ...defaultContext,
  environmentVariables: {
    ...defaultContext.environmentVariables,
    ...env,
  },
});

describe("Test getCurrentInsertedDirectory", () => {
  it("returns root for null cwd", () => {
    expect(getCurrentInsertedDirectory(null, "foo/", defaultContext)).to.equal("/");
  });

  it("returns merged path when both cwd and search term are specified", () => {
    expect(getCurrentInsertedDirectory(defaultCwd, "test/", defaultContext)).to.equal(
      `${defaultCwd}/test/`,
    );
  });

  it("returns partial path when trailing slash is missing (1)", () => {
    expect(getCurrentInsertedDirectory(defaultCwd, "src/packages", defaultContext)).to.equal(
      `${defaultCwd}/src/`,
    );
  });

  it("returns partial path when trailing slash is missing (2)", () => {
    expect(getCurrentInsertedDirectory(defaultCwd, "src", defaultContext)).to.equal(
      `${defaultCwd}/`,
    );
  });

  it("returns the entire search term if it is an absolute path relative to ~", () => {
    expect(
      getCurrentInsertedDirectory(defaultCwd, "~/some_dir/src/test/", defaultContext),
    ).to.equal(`${defaultHome}/some_dir/src/test/`);
  });

  it("returns the entire search term if it is an absolute path relative to /", () => {
    expect(getCurrentInsertedDirectory(defaultCwd, "/etc/bin/tool", defaultContext)).to.equal(
      "/etc/bin/",
    );
  });

  it("returns the path with $HOME resolved", () => {
    expect(getCurrentInsertedDirectory(defaultCwd, "$HOME/src/test/", defaultContext)).to.equal(
      `${defaultHome}/src/test/`,
    );
  });

  it("returns the path with $DIR resolved", () => {
    expect(
      getCurrentInsertedDirectory(
        defaultCwd,
        "$DIR/src/test/",
        defaultContextWithEnv({
          DIR: "/tmp/folder",
        }),
      ),
    ).to.equal("/tmp/folder/src/test/");
  });

  it("returns the path with $DIR and $HOME resolved", () => {
    expect(
      getCurrentInsertedDirectory(
        defaultCwd,
        "$HOME/src/$DIR/test/",
        defaultContextWithEnv({
          DIR: "tmp/folder",
        }),
      ),
    ).to.equal(`${defaultHome}/src/tmp/folder/test/`);
  });

  it("returns the path when just $HOME is specified", () => {
    expect(getCurrentInsertedDirectory(defaultHome, "$HOME", defaultContext)).to.equal("/home/");
  });

  it("returns the path when $DIR is not specified", () => {
    expect(getCurrentInsertedDirectory(defaultCwd, "$DIR", defaultContext)).to.equal(
      `${defaultCwd}/`,
    );
  });

  it("returns the path with the complex $DIR and $HOME resolved", () => {
    expect(
      getCurrentInsertedDirectory(
        defaultCwd,
        // eslint-disable-next-line no-template-curly-in-string
        "${HOME}/src/${DIR}/test/",
        defaultContextWithEnv({
          DIR: "tmp/folder",
        }),
      ),
    ).to.equal(`${defaultHome}/src/tmp/folder/test/`);
  });

  it("returns the path with the complex $DIR and $HOME resolved with default", () => {
    expect(
      getCurrentInsertedDirectory(
        defaultCwd,
        // eslint-disable-next-line no-template-curly-in-string
        "${HOME:-/fallback}/src/${DIR:-fallback}/test/",
        defaultContext,
      ),
    ).to.equal(`${defaultHome}/src/fallback/test/`);
  });
});

describe("Test sortFilesAlphabetically", () => {
  const files = ["a", "e", "z", "t", "m", "b", "x"];

  it("should sort files alphabetically", () => {
    expect(sortFilesAlphabetically(files)).to.eql(["a", "b", "e", "m", "t", "x", "z", "../"]);
  });

  it("should sort files alphabetically and put dotfiles/folders at the end", () => {
    expect(sortFilesAlphabetically([...files, ".g", ".l"])).to.eql([
      "a",
      "b",
      "e",
      "m",
      "t",
      "x",
      "z",
      ".g",
      ".l",
      "../",
    ]);
  });

  it("should sort files alphabetically and skip files in the skip array", () => {
    expect(sortFilesAlphabetically(files, ["b", "t"])).to.eql(["a", "e", "m", "x", "z", "../"]);
  });
});

const names = (suggestions: Fig.Suggestion[]) => suggestions.map(({ name }) => name);

describe("filepaths generators", () => {
  const entries = ["src/", "README", "R", "index.ts", "types.d.ts", "archive.tar.gz", ".DS_Store"];
  const executeCommand = vi.fn<Fig.ExecuteCommandFunction>();
  const run = async (options: FilepathsOptions, files = entries) => {
    const generator = filepaths(options);
    return generator.custom!([], executeCommand, {
      ...defaultContext,
      listDirectory: async () => files,
    });
  };

  it("treats a string equals filter as one filename, not individual characters", async () => {
    expect(names(await run({ equals: "README", showFolders: "never" }))).toEqual(["README"]);
  });

  it("accepts multi-part extensions and leading dots", async () => {
    expect(names(await run({ extensions: [".d.ts", "tar.gz"], showFolders: "never" }))).toEqual([
      "archive.tar.gz",
      "types.d.ts",
    ]);
  });

  it("keeps folders unless folder filtering is requested", async () => {
    expect(names(await run({ equals: ["README"] }))).toEqual(["README", "src/", "../"]);
    expect(names(await run({ matches: /^src/, filterFolders: true }))).toEqual(["src/"]);
    expect(names(await run({ showFolders: "only" }))).toEqual(["src/", "../"]);
  });

  it("applies per-type edits and excludes Finder metadata", async () => {
    const result = await run({
      editFileSuggestions: { priority: 90 },
      editFolderSuggestions: { priority: 20 },
    });
    expect(result.some(({ name }) => name === ".DS_Store")).toBe(false);
    expect(result.every(({ type, priority }) => priority === (type === "file" ? 90 : 20))).toBe(
      true,
    );
  });

  it("uses the native directory port and preserves embedded newlines", async () => {
    const listDirectory = vi.fn(async () => ["line\nbreak.txt"]);
    const execute = vi.fn<Fig.ExecuteCommandFunction>();
    const result = await filepaths({ rootDirectory: "/root" }).custom!([], execute, {
      ...defaultContext,
      searchTerm: "sub/partial",
      listDirectory,
    });
    expect(listDirectory).toHaveBeenCalledWith("/root/sub/");
    expect(execute).not.toHaveBeenCalled();
    expect(names(result)).toEqual(["line\nbreak.txt", "../"]);
  });

  it("retains the command fallback for spec consumers without a directory port", async () => {
    const execute = vi
      .fn<Fig.ExecuteCommandFunction>()
      .mockResolvedValue({ stdout: "file.txt\nfolder/\n", stderr: "", status: 0 });
    const result = await folders.custom!([], execute, defaultContext);
    expect(execute).toHaveBeenCalledWith({ command: "ls", args: ["-1ApL"], cwd: `${defaultCwd}/` });
    expect(names(result)).toEqual(["folder/", "../"]);
  });

  it("does not suggest a parent directory when the fallback command fails", async () => {
    const execute = vi
      .fn<Fig.ExecuteCommandFunction>()
      .mockResolvedValue({ stdout: "", stderr: "permission denied", status: 1 });
    expect(await filepaths.custom!([], execute, defaultContext)).toEqual([]);
  });

  it("returns no suggestions after a directory read failure", async () => {
    const result = await filepaths.custom!([], executeCommand, {
      ...defaultContext,
      listDirectory: async () => {
        throw new Error("unreadable");
      },
    });
    expect(result).toEqual([]);
  });

  it("refreshes only when the directory changes and extracts the last segment", () => {
    const generator = filepaths();
    const trigger = generator.trigger;
    if (typeof trigger !== "function") throw new TypeError("expected a functional trigger");
    expect(trigger("src/a", "src/ab")).toBe(false);
    expect(trigger("src/a", "lib/a")).toBe(true);
    expect(trigger("src/a", "src/sub/a")).toBe(true);
    expect(trigger("a", "ab")).toBe(false);
    const getQueryTerm = generator.getQueryTerm;
    if (typeof getQueryTerm !== "function") throw new TypeError("expected a query function");
    expect(getQueryTerm("src/partial")).toBe("partial");
  });
});
