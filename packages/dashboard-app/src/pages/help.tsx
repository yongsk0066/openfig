import { Code } from "@/components/text/code";
import { Link } from "@/components/ui/link";
import support from "@/data/help";

export default function Page() {
  return (
    <div className="flex flex-col items-start gap-8 pb-4">
      <div className="flex flex-col justify-between gap-2 self-stretch">
        <Link to="/integrations" variant="blue">
          Check permissions and terminal setup →
        </Link>
      </div>
      <div className="flex flex-col justify-between gap-4 self-stretch">
        <h1 className="text-xl font-bold select-none mb-2">Still having issues?</h1>
        <div className="flex flex-col gap-4">
          <ol className="flex flex-col gap-2">
            {support.steps.map((step, i) => {
              const stringAsArray = step.split("`");
              return (
                <li key={i} className="flex gap-2 items-baseline">
                  <span>{i + 1}.</span>
                  {stringAsArray.map((substr, partIndex) => {
                    if (partIndex === 1) {
                      return <Code key={partIndex}>{substr}</Code>;
                    }

                    return <span key={partIndex}>{substr}</span>;
                  })}
                </li>
              );
            })}
          </ol>
          <div className="flex flex-col">
            <span className="text-zinc-500 dark:text-zinc-400">
              You can also check out the following:
            </span>
            <div className="flex gap-4">
              {support.links.map((link, i) => {
                return (
                  <Link key={i} href={link.url} variant="blue" className="font-medium">
                    {link.text} →
                  </Link>
                );
              })}
            </div>
          </div>
        </div>
      </div>
    </div>
  );
}
