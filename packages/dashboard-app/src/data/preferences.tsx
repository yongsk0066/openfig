import { PRODUCT_NAME } from "@/lib/constants";

const generalPreferences = [
  {
    title: "Startup",
    properties: [
      {
        id: "app.launchOnStartup",
        title: "Start with macOS",
        description: `Start ${PRODUCT_NAME} automatically when you sign in to your Mac. macOS may require approval in System Settings → General → Login Items. If you disabled it there, enable it there again.`,
        type: "boolean",
        default: true,
        popular: false,
      },
      {
        id: "app.hideMenubarIcon",
        title: "Display Menu Bar icon",
        description: `${PRODUCT_NAME} icon will appear in the Menu Bar while ${PRODUCT_NAME} is running.`,
        type: "boolean",
        default: false,
        inverted: true,
        popular: false,
      },
    ],
  },
];

export default generalPreferences;
