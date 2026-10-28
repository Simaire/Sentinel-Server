<script lang="ts">
  import { onMount } from "svelte";
  import { api, type Role, type Status, type UserDetail } from "../lib/api";
  import { ACTION_LABELS, ago, date, STATUS_LABELS } from "../lib/format";

  let {
    id,
    self,
    onclose,
    onchanged
  }: { id: string; self: string; onclose: () => void; onchanged: () => void | Promise<void> } = $props();

  let data = $state<UserDetail | null>(null);
  let note = $state("");
  let error = $state("");
  let busy = $state(false);
  const path = $derived(`/admin/users/${encodeURIComponent(id)}`);
  const isSelf = $derived(id === self);

  async function load() {
    try {
      data = await api<UserDetail>("GET", path);
      note = data.user.note ?? "";
      error = "";
    } catch (e) {
      error = (e as Error).message;
    }
  }

  async function act(run: () => Promise<unknown>) {
    busy = true;
    try {
      await run();
      await load();
      await onchanged();
    } catch (e) {
      error = (e as Error).message;
    } finally {
      busy = false;
    }
  }

  const setStatus = (status: Status) => act(() => api("POST", `${path}/status`, { status }));
  const setRole = (role: Role) => act(() => api("POST", `${path}/role`, { role }));
  const saveNote = () => act(() => api("POST", `${path}/note`, { note }));
  const revokeAll = () =>
    confirm("Révoquer tous les appareils de ce compte ?") && act(() => api("POST", `${path}/revoke-devices`));
  const revoke = (deviceId: number) =>
    confirm("Révoquer cet appareil ? Il devra être validé à nouveau.") && act(() => api("DELETE", `/admin/devices/${deviceId}`));
  const approve = (deviceId: number, code: string) =>
    confirm(`Le modo voit-il bien le code ${code} dans son appli ?`) &&
    act(() => api("POST", `/admin/devices/${deviceId}/approve`));
  const DEVICE_LABELS = { pending: "En attente", active: "Validé", revoked: "Révoqué" };
  async function remove() {
    if (!confirm("Supprimer ce compte ? Ses données partagées (votes, signalements) restent.")) return;
    await act(() => api("DELETE", path));
    onclose();
  }

  onMount(() => {
    void load();
  });
</script>

<svelte:window onkeydown={(e) => e.key === "Escape" && onclose()} />

<div class="backdrop" role="presentation" onclick={(e) => e.target === e.currentTarget && onclose()}>
  <div class="panel" role="dialog" aria-modal="true" aria-label="Fiche du compte">
    <button class="ghost close" onclick={onclose} aria-label="Fermer">✕</button>
    {#if error}<p class="error">{error}</p>{/if}
    {#if data}
      {@const u = data.user}
      <h2>{u.display_name}</h2>
      <code>{u.vrc_id}</code>
      <p class="tags">
        <span class="badge {u.status}">{STATUS_LABELS[u.status]}</span>
        {#if u.role === "admin"}<span class="badge admin">Admin</span>{/if}
        {#if data.online}<span class="badge active">En ligne · {data.online.world_name ?? "hors instance"}</span>{/if}
      </p>
      <dl>
        <dt>Inscrit</dt><dd>{date(u.created_at)}</dd>
        <dt>Validé</dt><dd>{u.approved_at ? `${date(u.approved_at)}${u.approved_by ? ` par ${u.approved_by}` : ""}` : "—"}</dd>
        <dt>Dernière activité</dt><dd>{ago(u.last_seen_at)}</dd>
        <dt>Contributions</dt><dd>{data.contributions.reports} signalement(s) · {data.contributions.votes} vote(s) · {data.contributions.actions} action(s)</dd>
      </dl>

      {#if !isSelf}
        <div class="buttons">
          {#if u.status === "banned"}<button class="ok" disabled={busy} onclick={() => setStatus("active")}>Débannir</button>{/if}
          {#if u.status !== "banned"}<button class="danger" disabled={busy} onclick={() => confirm("Bannir ce compte ?") && setStatus("banned")}>Bannir</button>{/if}
          {#if u.role === "moderator"}
            <button class="ghost" disabled={busy || u.status !== "active"} onclick={() => confirm("Donner les droits d’administration ?") && setRole("admin")}>Promouvoir admin</button>
          {:else}
            <button class="ghost" disabled={busy} onclick={() => setRole("moderator")}>Repasser modo</button>
          {/if}
          <button class="ghost" disabled={busy} onclick={revokeAll}>Révoquer tous les appareils</button>
          <button class="danger" disabled={busy} onclick={remove}>Supprimer le compte</button>
        </div>
      {:else}
        <p class="muted small">C’est ton compte : statut et rôle ne se modifient pas d’ici.</p>
      {/if}

      <h3>Note interne</h3>
      <textarea bind:value={note} rows="3" maxlength="1000" placeholder="Visible des admins seulement"></textarea>
      <button class="ghost small save" disabled={busy || note === (u.note ?? "")} onclick={saveNote}>Enregistrer la note</button>

      <h3>Appareils ({u.devices} validé(s){u.pending_devices ? `, ${u.pending_devices} en attente` : ""})</h3>
      {#if u.pending_devices}
        <p class="muted small">
          Avant de valider, demande au modo le code affiché dans son appli (Paramètres → Base de données) : il doit
          être identique.
        </p>
      {/if}
      <ul class="list">
        {#each data.devices as d (d.id)}
          <li class:inactive={d.status === "revoked"} class:waiting={d.status === "pending"}>
            <div>
              <strong>{d.name || "Appareil sans nom"}</strong>
              <code class="code">{d.fingerprint}</code>
              <span class="muted small">
                {DEVICE_LABELS[d.status]} · ajouté {date(d.created_at)} · utilisé {ago(d.last_used_at)}
                {#if d.approved_at} · validé {date(d.approved_at)}{d.approved_by ? ` par ${d.approved_by}` : ""}{/if}
              </span>
            </div>
            <div class="row">
              {#if d.status === "pending"}
                <button class="ok small" disabled={busy || u.status === "banned"} onclick={() => approve(d.id, d.fingerprint)}>Valider</button>
              {/if}
              {#if d.status !== "revoked"}<button class="danger small" disabled={busy} onclick={() => revoke(d.id)}>Révoquer</button>{/if}
            </div>
          </li>
        {:else}
          <li class="muted small">Aucun appareil.</li>
        {/each}
      </ul>

      <h3>Historique</h3>
      <ul class="list">
        {#each data.audit as a, i (i)}
          <li>
            <div>
              <strong>{ACTION_LABELS[a.action] ?? a.action}</strong>
              <span class="muted small">{date(a.at)}{a.actor && a.actor !== id ? ` · par ${a.actor}` : ""}{a.detail ? ` · ${a.detail}` : ""}</span>
            </div>
          </li>
        {:else}
          <li class="muted small">Rien pour l’instant.</li>
        {/each}
      </ul>
    {:else if !error}
      <p class="muted">Chargement…</p>
    {/if}
  </div>
</div>

<style>
  .backdrop { position: fixed; inset: 0; z-index: 20; display: flex; justify-content: flex-end; background: #07050b99; }
  .panel { position: relative; display: grid; align-content: start; gap: 10px; width: min(520px, 100%); height: 100%; overflow-y: auto; padding: 24px; border-left: 1px solid var(--line); background: var(--panel); }
  .close { position: absolute; top: 14px; right: 14px; }
  h2 { padding-right: 40px; font-size: 22px; }
  h3 { margin-top: 10px; font-size: 14px; color: var(--muted); }
  .tags { display: flex; flex-wrap: wrap; gap: 6px; margin: 0; }
  dl { display: grid; grid-template-columns: 140px 1fr; gap: 6px 12px; margin: 6px 0; font-size: 13px; }
  dt { color: var(--muted); }
  dd { margin: 0; }
  .buttons { display: flex; flex-wrap: wrap; gap: 8px; }
  textarea { width: 100%; resize: vertical; }
  .save { justify-self: start; }
  .list { display: grid; gap: 6px; margin: 0; padding: 0; list-style: none; }
  .list li { display: flex; justify-content: space-between; align-items: center; gap: 10px; padding: 8px 10px; border-radius: 9px; background: #120f18; }
  .list li div { display: grid; gap: 2px; min-width: 0; }
  .inactive { opacity: 0.55; }
  .waiting { outline: 1px solid #ffcc3366; }
  .code { font-size: 15px; letter-spacing: 0.06em; }
  .list li .row { display: flex; gap: 6px; flex-shrink: 0; }
</style>
