import autocompleteDemo from "@assets/images/autocomplete_demo.gif";
import translateDemo from "@assets/images/translate_demo.gif";
import chatDemo from "@assets/images/chat_demo.gif";
import { Link } from "@/components/ui/link";
import { KIRO_CLI_URL } from "@/lib/constants";
import { Terminal } from "@/components/ui/terminal";
import { KiroLogo } from "@/components/svg/logo";

export default function Page() {
  return (
    <div className="flex flex-col items-start gap-4">
      <div className="flex flex-col">
        <h1 className="text-2xl font-bold dark:text-zinc-400">
          Getting started
        </h1>
      </div>
      <section className="flex flex-col p-6 gap-4 w-full rounded-2xl items-start text-white q-banner">
        <div className="flex flex-row gap-4 w-full items-center">
          <div className="flex-shrink-0">
            <KiroLogo size={60} />
          </div>
          <div className="flex flex-col gap-1">
            <h1 className="font-bold text-lg font-ember leading-none">
              Amazon Q Developer CLI users can now upgrade to the Kiro CLI
            </h1>
            <p className="text-sm leading-tight">
              Your existing Q Developer CLI workflows and subscription continue
              to work without any changes. You can update to Kiro CLI manually
              with{" "}
              <code style={{ color: "var(--q-banner-link)" }}>q update</code>{" "}
              today, or wait for the auto-update on 11/24 to apply the change.{" "}
              <Link
                href={KIRO_CLI_URL}
                className="font-medium no-underline"
                variant="primary"
                style={{ color: "var(--q-banner-link)" }}
              >
                Learn more &gt;
              </Link>
            </p>
          </div>
        </div>
      </section>
      <div className="flex flex-col gap-2 w-full border bg-zinc-50 border-zinc-200 dark:bg-zinc-900 dark:border-zinc-600 rounded-lg p-4 mb-4 overflow-x-auto">
        <h2 className="font-bold font-ember tracking-tight text-lg dark:text-zinc-300">
          Launch a new shell session to start using autocomplete!
        </h2>
        <ol className="flex flex-col gap-0.5 text-zinc-600 dark:text-zinc-400">
          <li>
            <span className="text-sm flex items-baseline gap-1">
              <span className="font-semibold">Not working?</span>
              <span>Check out</span>
              <Link to="/help" variant="blue">
                Help & support
              </Link>
            </span>
          </li>
          <li>
            <span className="text-sm">
              <span className="font-semibold">Want to customize settings?</span>{" "}
              Use the tabs in the sidebar.
            </span>
          </li>
        </ol>
      </div>
      <Terminal tabNames={["Autocomplete", "Translation", "Chat"]}>
        <Terminal.Tab>
          <img src={autocompleteDemo} alt="autocomplete demo" />
        </Terminal.Tab>
        <Terminal.Tab>
          <img src={translateDemo} alt="translation demo" />
        </Terminal.Tab>
        <Terminal.Tab>
          <img src={chatDemo} alt="chat demo" />
        </Terminal.Tab>
      </Terminal>
    </div>
  );
}
