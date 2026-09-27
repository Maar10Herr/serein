import { describe, expect, it } from "vitest";
import { messages, t, type MessageKey } from "./locales";

const translatedLocales = ["de", "nl", "zh-CN", "ja", "es"] as const;
const placeholders = (message: string) =>
  [...message.matchAll(/\{([A-Za-z0-9_]+)\}/gu)]
    .map((match) => match[1])
    .sort();

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
