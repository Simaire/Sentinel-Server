// Appels à l'API d'administration. La session est un cookie HttpOnly ; l'en-tête
// X-Sentinel protège les modifications contre les requêtes d'autres sites.

export class ApiError extends Error {
  constructor(
    public status: number,
    public code: string,
    message: string
  ) {
    super(message);
  }
}

/** Chemin du serveur : `""` à la racine, `/sentinel` s'il est publié sous un sous-chemin. */
export const BASE = location.pathname.replace(/\/admin(\/.*)?$/, "");

/** Session expirée ou absente : l'application affiche l'écran de connexion. */
export const SESSION_LOST = "sentinel:session-lost";

export async function api<T>(method: string, path: string, body?: unknown): Promise<T> {
  const response = await fetch(`${BASE}/api/v1${path}`, {
    method,
    credentials: "same-origin",
    headers: {
      "x-sentinel": "1",
      ...(body === undefined ? {} : { "content-type": "application/json" })
    },
    body: body === undefined ? undefined : JSON.stringify(body)
  });
  const data = await response.json().catch(() => null);
  if (!response.ok) {
    if (response.status === 401) window.dispatchEvent(new Event(SESSION_LOST));
    throw new ApiError(response.status, data?.code ?? "error", data?.message ?? `Erreur ${response.status}`);
  }
  return data as T;
}

export type Status = "pending" | "active" | "banned";
export type Role = "moderator" | "admin";

export type User = {
  vrc_id: string;
  display_name: string;
  status: Status;
  role: Role;
  created_at: number;
  approved_at: number | null;
  approved_by: string | null;
  last_seen_at: number | null;
  note: string | null;
  /** Appareils validés et appareils en attente de validation. */
  devices: number;
  pending_devices: number;
  online?: boolean;
};

export type Online = {
  user_id: string;
  display_name: string | null;
  location: string | null;
  world_name: string | null;
  user_count: number;
  danger: number;
  caution: number;
  version: string | null;
  last_seen_at: number;
};

export type Overview = {
  version: string;
  uptime_secs: number;
  head: number;
  users: { pending: number; active: number; banned: number; admins: number };
  devices: number;
  pending_devices: number;
  online: Online[];
  realtime_clients: number;
  db_size_bytes: number;
  tables: { name: string; rows: number; exact: boolean }[];
  metrics: {
    pushes: number;
    rows_written: number;
    pulls: number;
    rows_read: number;
    logins: number;
    login_failures: number;
  };
  registration_open: boolean;
};

export type Device = {
  id: number;
  /** Code de l'appareil, affiché aussi dans l'appli du modo. */
  fingerprint: string;
  name: string | null;
  status: "pending" | "active" | "revoked";
  created_at: number;
  approved_at: number | null;
  approved_by: string | null;
  last_used_at: number;
};

export type AuditEntry = {
  id?: number;
  at: number;
  actor: string | null;
  actor_name?: string | null;
  action: string;
  target: string | null;
  detail: string | null;
};

export type UserDetail = {
  user: User;
  online: Online | null;
  devices: Device[];
  audit: AuditEntry[];
  contributions: { reports: number; votes: number; actions: number };
};

export type TableInfo = { name: string; columns: string[]; keys: string[]; rows: number; exact: boolean };
export type TableRows = { columns: string[]; rows: (string | number | null)[][]; page_size: number };
