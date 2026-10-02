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

// Wire shape: contracts/dashboard-response.schema.json.
export interface DashboardCard {
  id: string;
  site: string;
  text: string;
  state: "observed" | "confirmed";
  last_seen: string;
  sessions: number;
  sites: 1;
  kind: "visit" | "search";
  prominent: boolean;
  corrections: Array<{ action: string; text: string | null }>;
}
export interface DashboardMemory {
  id: string;
  label: string;
  last_seen: string;
  sessions: number;
  sites: number;
  evidence_count: number;
  items: DashboardCard[];
}
export interface DashboardResponse {
  cards: DashboardCard[];
  memories: DashboardMemory[];
  topics: unknown[];
  model_available?: boolean;
  index_mode?: "hybrid" | "lexical";
  atoms?: number;
  database_path?: string;
  vault_bytes?: number;
  evidence_generation?: number;
  privacy_generation?: number;
}
