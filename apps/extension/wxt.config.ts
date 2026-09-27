import { defineConfig } from "wxt";
import preact from "@preact/preset-vite";
import metadata from "../../release/extension-identity.json";

const configuredSkillRepository =
  process.env.SEREIN_SKILL_REPOSITORY?.trim() ?? "";
const githubRepositoryPattern =
  /^https:\/\/github\.com\/([A-Za-z0-9](?:[A-Za-z0-9-]{0,38}))\/([A-Za-z0-9](?:[A-Za-z0-9._-]{0,99}))\/?$/;
const skillRepository = githubRepositoryPattern.test(configuredSkillRepository)
  ? configuredSkillRepository.replace(/\/$/, "")
  : "";

export default defineConfig({
  manifestVersion: 3,
  vite: () => ({
    plugins: [preact()],
    define: {
      __SEREIN_SKILL_REPOSITORY__: JSON.stringify(skillRepository),
    },
  }),
  manifest: ({ browser }) => ({
    name: "Serein",
    description: "Your context. Only when it helps.",
    version: "0.1.1",
    permissions: ["tabs", "storage", "alarms", "idle", "nativeMessaging"],
    ...(browser === "firefox"
      ? {
          browser_specific_settings: {
            gecko: {
              id: metadata.firefox_extension_id,
              strict_min_version: "140.0",
              data_collection_permissions: {
                required: ["browsingActivity", "searchTerms", "websiteContent"],
              },
            },
          },
        }
      : { key: metadata.chrome_public_key }),
    icons: {
      16: "/icon/icon-16.png",
      32: "/icon/icon-32.png",
      48: "/icon/icon-48.png",
      128: "/icon/icon-128.png",
    },
    action: { default_title: "Serein" },
  }),
});
