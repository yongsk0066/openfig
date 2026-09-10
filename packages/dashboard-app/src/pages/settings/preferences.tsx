import { UserPrefView } from "@/components/preference/list";
import { Link } from "@/components/ui/link";
import settings from "@/data/preferences";

export default function Page() {
  return (
    <>
      <UserPrefView array={settings} />
      <section className={`py-4 gap-4`}>
        <h2
          id={`subhead-licenses`}
          className="font-bold text-medium text-zinc-400 leading-none mt-2"
        >
          Licenses
        </h2>
        <Link to="/licenses" variant="blue" className="text-sm">
          View licenses
        </Link>
      </section>
    </>
  );
}
