import { openDB } from "idb";
import type { State, Visit, Observation, Control } from "./types";
const db = openDB("serein", 1, {
  upgrade(db) {
    db.createObjectStore("state");
    db.createObjectStore("outbox", { keyPath: "event_id" });
    db.createObjectStore("controls", { keyPath: "id" });
  },
});
export async function getState(): Promise<State> {
  const d = await db;
  let s = await d.get("state", "config");
  if (!s) {
    s = {
      source_id: crypto.randomUUID(),
      policy: {
        consent: false,
        paused: false,
        recall_enabled: false,
        selected_only: false,
        selected_sites: [],
        excluded_sites: [],
        capture_epoch: 0,
      },
      siteEpochs: {},
      paired: false,
      dropped: 0,
      retry: 0,
    };
    await d.put("state", s, "config");
  }
  return s;
}
export async function saveState(s: State) {
  await (await db).put("state", s, "config");
}
export async function visit(): Promise<Visit | undefined> {
  return (await db).get("state", "visit");
}
export async function setVisit(v?: Visit) {
  const d = await db;
  if (v) await d.put("state", v, "visit");
  else await d.delete("state", "visit");
}
export async function enqueue(e: Observation) {
  const d = await db;
  const tx = d.transaction(["outbox", "state"], "readwrite");
  const store = tx.objectStore("outbox");
  await store.put(e);
  let items: Observation[] = await store.getAll();
  items.sort((a, b) => a.observed_at.localeCompare(b.observed_at));
  let bytes = new TextEncoder().encode(JSON.stringify(items)).length;
  let dropped = 0;
  while (items.length > 2000 || bytes > 2 * 1024 * 1024) {
    const first = items.shift()!;
    bytes -= new TextEncoder().encode(JSON.stringify(first)).length;
    await store.delete(first.event_id);
    dropped++;
  }
  if (dropped) {
    const s = await tx.objectStore("state").get("config");
    s.dropped += dropped;
    await tx.objectStore("state").put(s, "config");
  }
  await tx.done;
}
export async function pending() {
  return (await db).getAll("outbox") as Promise<Observation[]>;
}
export async function controls() {
  return (await db).getAll("controls") as Promise<Control[]>;
}
export async function removeEvents(ids: string[]) {
  const d = await db;
  const tx = d.transaction("outbox", "readwrite");
  for (const id of ids) await tx.store.delete(id);
  await tx.done;
}
export async function removeControl(id: string) {
  await (await db).delete("controls", id);
}
export async function privacyTransition(
  s: State,
  c: Control,
  forgetSite?: string,
) {
  const d = await db;
  const tx = d.transaction(["state", "controls", "outbox"], "readwrite");
  await tx.objectStore("state").put(s, "config");
  await tx.objectStore("state").delete("visit");
  await tx.objectStore("controls").put(c);
  const all = await tx.objectStore("outbox").getAll();
  for (const e of all) {
    if (
      !forgetSite ||
      e.site_key === forgetSite ||
      e.site_key.endsWith("." + forgetSite)
    )
      await tx.objectStore("outbox").delete(e.event_id);
  }
  await tx.done;
}
