// `heroku apps --json`, limited to the fields this spec reads
interface HerokuApp {
  name: string;
}

export const getAppGenerator: Fig.Generator = {
  script: ["heroku", "apps", "--all", "--json"],
  cache: {
    strategy: "stale-while-revalidate",
  },
  scriptTimeout: 15000,
  postProcess: function (out) {
    try {
      const apps: HerokuApp[] = JSON.parse(out);
      return apps.map((app) => {
        return {
          name: app.name,
          description: app.name,
          icon: "https://www.herokucdn.com/favicon.ico",
        };
      });
    } catch (e) {
      return [];
    }
  },
};

export default {};
