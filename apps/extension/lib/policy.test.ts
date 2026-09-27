import { describe, it, expect } from "vitest";
import { sanitize, searchTerm, matches, clean } from "./policy";
import type { Policy } from "./types";
const policy: Policy = {
  consent: true,
  paused: false,
  recall_enabled: true,
  selected_only: false,
  selected_sites: [],
  excluded_sites: [],
  capture_epoch: 1,
};
describe("capture gates", () => {
  it("rejects before consent, during pause and in incognito", () => {
    const t = { url: "https://example.com", title: "Research" };
    expect(sanitize(t, { ...policy, consent: false })).toBeNull();
    expect(sanitize(t, { ...policy, paused: true })).toBeNull();
    expect(sanitize({ ...t, incognito: true }, policy)).toBeNull();
  });
  it.each([
    "http://127.0.0.1/foo",
    "http://10.0.0.3/a",
    "http://intranet/",
    "file:///private/data",
    "chrome://extensions",
    "https://mail.google.com/mail/",
    "https://example.com/account/",
    "https://user:pass@example.com/",
    "https://example.local/",
  ])("rejects %s", (url) =>
    expect(sanitize({ url, title: "Research" }, policy)).toBeNull(),
  );
  it("enforces host boundaries and selected-only mode", () => {
    expect(matches("www.example.com", "example.com")).toBe(true);
    expect(matches("notexample.com", "example.com")).toBe(false);
    expect(
      sanitize(
        { url: "https://example.com" },
        { ...policy, selected_only: true },
      ),
    ).toBeNull();
  });
  it("never preserves URL path, fragments or arbitrary query", () => {
    const out = sanitize(
      {
        url: "https://example.com/a/private-path?secret=123#token",
        title: "Useful title",
      },
      policy,
    );
    expect(out).toEqual({
      site_key: "example.com",
      title: "Useful title",
      kind: "visit",
      search_query: null,
    });
  });
  it("preserves units and redacts obvious identifiers", () => {
    expect(clean("Mock shelf spec 42 cm qa@example.com", 256)).toBe(
      "Mock shelf spec 42 cm [redacted email]",
    );
  });
  it("drops sensitive metadata", () =>
    expect(
      sanitize(
        { url: "https://example.com", title: "Patient diagnosis" },
        policy,
      ),
    ).toBeNull());
});
describe("search provider fixtures", () => {
  it.each([
    ["https://www.google.de/search?q=weite+hose", "weite hose"],
    ["https://www.amazon.nl/s?k=mock+shelf+42+cm", "mock shelf 42 cm"],
    ["https://www.amazon.co.jp/s?k=%E6%97%85%E8%A1%8C", "旅行"],
    [
      "https://www.youtube.com/results?search_query=desk+lamp+repair",
      "desk lamp repair",
    ],
    ["https://duckduckgo.com/?q=research", "research"],
    ["https://www.bing.com/search?q=desk%20lamp", "desk lamp"],
    ["https://search.brave.com/search?q=local", "local"],
    ["https://kagi.com/search?q=local", "local"],
  ])("parses %s", (url, text) => expect(searchTerm(new URL(url))).toBe(text));
  it.each([
    "https://evilgoogle.com/search?q=secret",
    "https://example.com/?k=secret",
    "https://amazon.com/product?k=secret",
    "https://youtube.com/watch?search_query=secret",
    "https://google.com/search?q=one&q=two",
  ])("ignores %s", (url) => expect(searchTerm(new URL(url))).toBeNull());
});
import { foregroundDelta, shouldRetain } from "./attention";
describe("attention accounting", () => {
  it("never credits inactive or unfocused time", () => {
    expect(foregroundDelta(0, 600000, false)).toBe(0);
  });
  it("clamps unknown intervals and clock reversal", () => {
    expect(foregroundDelta(0, 600000, true)).toBe(65);
    expect(foregroundDelta(1000, 0, true)).toBe(0);
  });
  it("retains short searches and drops brief ordinary visits", () => {
    expect(shouldRetain("visit", 4)).toBe(false);
    expect(shouldRetain("visit", 5)).toBe(true);
    expect(shouldRetain("search", 0)).toBe(true);
  });
});
