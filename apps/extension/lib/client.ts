import { browser } from "wxt/browser";
export async function call<T = any>(
  message: Record<string, unknown>,
): Promise<T> {
  const response = await browser.runtime.sendMessage(message);
  if (response?.error) throw new Error(response.error);
  return response;
}
export const host = (op: string, payload: unknown = {}) =>
  call({ type: "host", op, payload });
export function openDashboard(section = "context") {
  return browser.tabs.create({
    url: browser.runtime.getURL("/dashboard.html") + "#" + section,
  });
}
