import { afterEach, describe, expect, it, vi } from "vitest";
import { dashboardRefresh } from "./dashboard-refresh";

function deferred<T>() {
  let resolve!: (value: T) => void;
  let reject!: (error: Error) => void;
  const promise = new Promise<T>((yes, no) => { resolve = yes; reject = no; });
  return { promise, resolve, reject };
}

afterEach(() => vi.useRealTimers());

describe("dashboard refresh concurrency", () => {
  it("refreshes a previously clean view when returning from a hidden page", async () => {
    vi.useFakeTimers();
    let visible = true;
    const load = vi.fn().mockResolvedValue(1);
    const refresh = dashboardRefresh({ load, apply: vi.fn(), error: vi.fn(), loading: vi.fn(), visible: () => visible });
    refresh.request(); await vi.runAllTimersAsync();
    visible = false; refresh.visibilityChanged();
    visible = true; refresh.visibilityChanged();
    await vi.advanceTimersByTimeAsync(149);
    expect(load).toHaveBeenCalledTimes(1);
    await vi.advanceTimersByTimeAsync(1);
    expect(load).toHaveBeenCalledTimes(2);
  });
  it("coalesces focus/visibility and keeps one trailing request", async () => {
    vi.useFakeTimers();
    const first = deferred<number>();
    const load = vi.fn().mockReturnValueOnce(first.promise).mockResolvedValue(2);
    const apply = vi.fn();
    const refresh = dashboardRefresh({ load, apply, error: vi.fn(), loading: vi.fn(), visible: () => true });
    refresh.request(150); refresh.request(150);
    await vi.advanceTimersByTimeAsync(149);
    expect(load).not.toHaveBeenCalled();
    await vi.advanceTimersByTimeAsync(1);
    expect(load).toHaveBeenCalledTimes(1);
    refresh.request(); refresh.request(150); refresh.request();
    await vi.advanceTimersByTimeAsync(500);
    expect(load).toHaveBeenCalledTimes(1);
    first.resolve(1);
    await vi.runAllTimersAsync();
    expect(load).toHaveBeenCalledTimes(2);
    expect(apply.mock.calls).toEqual([[1], [2]]);
  });

  it("rejects a pre-mutation response and waits for all mutations to settle", async () => {
    vi.useFakeTimers();
    const first = deferred<string>();
    const load = vi.fn().mockReturnValueOnce(first.promise).mockResolvedValue("current");
    const apply = vi.fn();
    const refresh = dashboardRefresh({ load, apply, error: vi.fn(), loading: vi.fn(), visible: () => true });
    refresh.request(); await vi.advanceTimersByTimeAsync(0);
    refresh.beginMutation(); refresh.beginMutation();
    first.resolve("revoked"); await vi.runAllTimersAsync();
    expect(apply).not.toHaveBeenCalled();
    refresh.endMutation(); refresh.request(); await vi.runAllTimersAsync();
    expect(load).toHaveBeenCalledTimes(1);
    refresh.endMutation(); await vi.runAllTimersAsync();
    expect(load).toHaveBeenCalledTimes(2);
    expect(apply.mock.calls).toEqual([["current"]]);
  });

  it("retains hidden dirty work and suppresses stale errors and unmounted responses", async () => {
    vi.useFakeTimers();
    let visible = false;
    const pending = deferred<number>();
    const next = deferred<number>();
    const load = vi.fn().mockReturnValueOnce(pending.promise).mockReturnValueOnce(next.promise);
    const apply = vi.fn(), error = vi.fn();
    const refresh = dashboardRefresh({ load, apply, error, loading: vi.fn(), visible: () => visible });
    refresh.request(); await vi.runAllTimersAsync();
    expect(load).not.toHaveBeenCalled();
    visible = true; refresh.visibilityChanged(); await vi.advanceTimersByTimeAsync(150);
    refresh.beginMutation();
    pending.reject(new Error("obsolete error")); await vi.runAllTimersAsync();
    expect(error).not.toHaveBeenCalled();
    refresh.endMutation(); await vi.advanceTimersByTimeAsync(0);
    refresh.dispose(); next.resolve(2); await vi.runAllTimersAsync();
    expect(apply).not.toHaveBeenCalled();
    refresh.request(); await vi.runAllTimersAsync();
    expect(load).toHaveBeenCalledTimes(2);
  });
});
