import { Code } from "@/components/text/code";
const isEven = (index: number) => index % 2 === 0;

export function parseBackticksToCode(string: string, className?: string) {
  if (!string.includes("`")) return <span>{string}</span>;

  const strArray = string.split("`");
  const startsWith = string.charAt(0) === "`";
  return (
    <span>
      {startsWith
        ? strArray.map((s, i) => {
            if (isEven(i)) {
              return (
                <Code key={i} className={className}>
                  {s}
                </Code>
              );
            } else {
              return <span key={i}>{s}</span>;
            }
          })
        : strArray.map((s, i) => {
            if (!isEven(i)) {
              return (
                <Code key={i} className={className}>
                  {s}
                </Code>
              );
            } else {
              return <span key={i}>{s}</span>;
            }
          })}
    </span>
  );
}
