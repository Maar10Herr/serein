export interface Policy {
  consent: boolean;
  paused: boolean;
  recall_enabled: boolean;
  selected_only: boolean;
  selected_sites: string[];
  excluded_sites: string[];
  capture_epoch: number;
}
export interface Observation {
  event_id: string;
  visit_id: string;
  site_key: string;
  site_epoch: number;
  observed_at: string;
  kind: "visit" | "search";
  title: string;
  search_query: string | null;
  foreground_seconds: number;
}
export interface State {
  source_id: string;
  policy: Policy;
  siteEpochs: Record<string, number>;
  paired: boolean;
  ticket?: Ticket;
  lastError?: string;
  dropped: number;
  retry: number;
  retryAt?: number;
  batchSince?: number;
  lastStatus?: Record<string, any>;
  pauseUntil?: number;
}
export interface Ticket {
  protocol: 1;
  source_id: string;
  extension_id: string;
  browser: string;
  nonce: string;
  expires_at: number;
  adapters: string[];
  install_skills: boolean;
  skill_repository: string;
  label: string;
  consent: true;
}
export interface Control {
  id: string;
  op: "policy.update" | "forget";
  epoch: number;
  payload: unknown;
}
export interface Visit {
  tab: number;
  window: number;
  identity: string;
  event: Observation;
  lastTick: number;
  epoch: number;
}
