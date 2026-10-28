export function date(unix: number | null | undefined): string {
  if (!unix) return "—";
  return new Date(unix * 1000).toLocaleString("fr-FR", { dateStyle: "short", timeStyle: "short" });
}

export function ago(unix: number | null | undefined): string {
  if (!unix) return "jamais";
  const secs = Math.max(0, Math.floor(Date.now() / 1000) - unix);
  if (secs < 60) return `il y a ${secs} s`;
  if (secs < 3600) return `il y a ${Math.floor(secs / 60)} min`;
  if (secs < 86_400) return `il y a ${Math.floor(secs / 3600)} h`;
  return `il y a ${Math.floor(secs / 86_400)} j`;
}

export function bytes(n: number): string {
  const units = ["o", "Ko", "Mo", "Go", "To"];
  let value = n;
  let unit = 0;
  while (value >= 1024 && unit < units.length - 1) {
    value /= 1024;
    unit++;
  }
  return `${value.toFixed(unit ? 1 : 0)} ${units[unit]}`;
}

export function duration(secs: number): string {
  const d = Math.floor(secs / 86_400);
  const h = Math.floor((secs % 86_400) / 3600);
  const m = Math.floor((secs % 3600) / 60);
  return d ? `${d} j ${h} h` : h ? `${h} h ${m} min` : `${m} min`;
}

export function count(n: number): string {
  return n.toLocaleString("fr-FR");
}

export const STATUS_LABELS = { pending: "En attente", active: "Actif", banned: "Banni" } as const;
export const ROLE_LABELS = { moderator: "Modo", admin: "Admin" } as const;

/** Libellés lisibles des actions du journal d'audit. */
export const ACTION_LABELS: Record<string, string> = {
  "auth.logout": "Déconnexion d'une appli",
  "auth.bad_signature": "Signature d'appareil refusée",
  "device.new": "Nouvel appareil à valider",
  "device.approve": "Appareil validé",
  "device.approve.admin": "Appareil validé (console, admin)",
  "device.revoke": "Appareil révoqué",
  "devices.revoke_all": "Tous les appareils révoqués",
  "admin.login": "Connexion à l'administration",
  "user.active": "Compte validé",
  "user.pending": "Compte remis en attente",
  "user.banned": "Compte banni",
  "user.role.admin": "Promu admin",
  "user.role.moderator": "Repassé modo",
  "user.note": "Note modifiée",
  "user.delete": "Compte supprimé",
  "report.delete": "Signalement supprimé",
  "settings.registration": "Inscriptions modifiées"
};
