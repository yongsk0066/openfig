import type { SpecFileImport } from "./specLoader";
export async function importSpecSource(source: string): Promise<SpecFileImport> {
  const url = URL.createObjectURL(new Blob([source], { type: "text/javascript" }));
  try {
    return (await import(/* @vite-ignore */ url)) as SpecFileImport;
  } finally {
    URL.revokeObjectURL(url);
  }
}
