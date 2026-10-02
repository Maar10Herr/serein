interface RefreshOptions<T> {
  load: () => Promise<T>;
  apply: (value: T) => void;
  error: (error: unknown) => void;
  loading: (loading: boolean) => void;
  visible: () => boolean;
}

/** One visible load at a time, with mutations invalidating older responses. */
export function dashboardRefresh<T>(options: RefreshOptions<T>) {
  let revision = 0;
  let mutations = 0;
  let running = false;
  let dirty = false;
  let disposed = false;
  let timer: ReturnType<typeof setTimeout> | undefined;

  function clearTimer() {
    if (timer !== undefined) clearTimeout(timer);
    timer = undefined;
  }

  function schedule(delay: number) {
    if (disposed || running || mutations || !options.visible()) return;
    clearTimer();
    timer = setTimeout(() => void load(), delay);
  }

  async function load() {
    timer = undefined;
    if (disposed || running || mutations || !options.visible() || !dirty) return;
    running = true;
    dirty = false;
    const capturedRevision = revision;
    options.loading(true);
    try {
      const value = await options.load();
      if (!disposed && capturedRevision === revision && mutations === 0)
        options.apply(value);
    } catch (error) {
      if (!disposed && capturedRevision === revision) options.error(error);
    } finally {
      running = false;
      if (!disposed) {
        options.loading(false);
        if (dirty) schedule(0);
      }
    }
  }

  return {
    currentRevision: () => revision,
    request(delay = 0) {
      if (disposed) return;
      dirty = true;
      schedule(delay);
    },
    visibilityChanged() {
      clearTimer();
      if (options.visible()) {
        dirty = true;
        schedule(150);
      }
    },
    beginMutation() {
      revision++;
      mutations++;
      dirty = true;
      clearTimer();
    },
    endMutation() {
      revision++;
      mutations = Math.max(0, mutations - 1);
      if (dirty) schedule(0);
    },
    dispose() {
      disposed = true;
      clearTimer();
    },
  };
}
