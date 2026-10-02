import { describe, expect, it } from "vitest";
import { messages, t, type MessageKey } from "./locales";

const translatedLocales = ["de", "nl", "zh-CN", "ja", "es"] as const;
const placeholders = (message: string) =>
  [...message.matchAll(/\{([A-Za-z0-9_]+)\}/gu)]
    .map((match) => match[1])
    .sort();
const bracketedExamples = (message: string) =>
  [...message.matchAll(/\[[^\]]+\]/gu)].map(() => "topic");

describe("locale catalogs", () => {
  it.each(translatedLocales)("covers every English message in %s", (locale) => {
    expect(Object.keys(messages[locale]).sort()).toEqual(
      Object.keys(messages.en).sort(),
    );
  });

  it.each(translatedLocales)("preserves interpolation keys in %s", (locale) => {
    for (const key of Object.keys(messages.en) as MessageKey[]) {
      expect(placeholders(messages[locale][key]), `${locale}: ${key}`).toEqual(
        placeholders(messages.en[key]),
      );
      expect(
        bracketedExamples(messages[locale][key]),
        `${locale}: ${key} bracketed examples`,
      ).toEqual(bracketedExamples(messages.en[key]));
    }
  });

  it("uses the specified English filtering and first-use copy", () => {
    expect(messages.en["context.searchLabel"]).toBe("Filter this view");
    expect(messages.en["context.searchPlaceholder"]).toBe("Filter this view");
    expect(messages.en["context.noMatch"]).toBe(
      "No matches in the displayed context.",
    );
    expect(messages.en["context.viewScopeNote"]).toBe(
      "This view shows recent activity and selected research evidence, not every saved observation.",
    );
    expect(messages.en["context.refresh"]).toBe("Refresh");
    expect(messages.en["context.stale"]).toBe(
      "Showing the last available context. Refresh to check for updates.",
    );
    expect(messages.en["context.unavailable"]).toBe(
      "Context is unavailable. Check the local connection and try Refresh.",
    );
    expect(messages.en["context.privacyUnconfirmed"]).toBe(
      "This privacy change is not confirmed. Affected context stays hidden until it is confirmed.",
    );
    expect(messages.en["journey.question"]).toBe(
      "What did I find about [the topic I researched]?",
    );
    expect(messages.en["journey.questionHelp"]).toBe(
      "Replace the bracketed text with a topic you researched after enabling collection.",
    );
    expect(messages.en["context.wrongTopicHelp"]).toBe(
      "This hides the observation from all assistant recall; it does not move it to another research group.",
    );
  });

  it.each(translatedLocales)("translates context status copy in %s", (locale) => {
    for (const key of [
      "context.stale",
      "context.unavailable",
      "context.privacyUnconfirmed",
    ] as const) {
      expect(messages[locale][key].trim(), `${locale}: ${key}`).not.toBe("");
      expect(messages[locale][key], `${locale}: ${key}`).not.toBe(
        messages.en[key],
      );
    }
  });

  it("resolves simplified Chinese locale aliases", () => {
    expect(t("zh-CN", "dashboard.language")).toBe("语言");
    expect(t("zh-Hans", "dashboard.language")).toBe("语言");
    expect(t("zh_CN", "dashboard.language")).toBe("语言");
  });

  it("interpolates placeholders and falls back for unsupported locales", () => {
    expect(t("ja-JP", "popup.connectionSummary", { count: 3 })).toBe(
      "アシスタント接続 3 件",
    );
    expect(t("fr", "dashboard.language")).toBe("Language");
  });
});
