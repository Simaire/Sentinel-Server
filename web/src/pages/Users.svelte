<script lang="ts">
  import { onMount } from "svelte";
  import { api, type Status, type User } from "../lib/api";
  import { ago, date, ROLE_LABELS, STATUS_LABELS } from "../lib/format";
  import UserDetail from "./UserDetail.svelte";

  let { self }: { self: string } = $props();

  const FILTERS: { value: "" | Status; label: string }[] = [
    { value: "", label: "Tous" },
    { value: "pending", label: "En attente" },
    { value: "active", label: "Actifs" },
    { value: "banned", label: "Bannis" }
  ];

  let users = $state<User[]>([]);
  let status = $state<"" | Status>("");
  let q = $state("");
  let error = $state("");
  let busy = $state<string | null>(null);
  let openId = $state<string | null>(null);
  let timer: ReturnType<typeof setTimeout> | undefined;

  const pending = $derived(users.filter((u) => u.status === "pending" || u.pending_devices > 0).length);

  async function load() {
    try {
      const params = new URLSearchParams({ status, q });
      users = (await api<{ users: User[] }>("GET", `/admin/users?${params}`)).users;
      error = "";
    } catch (e) {
      error = (e as Error).message;
    }
  }

  function search() {
    clearTimeout(timer);
    timer = setTimeout(() => void load(), 250);
  }

  async function setStatus(user: User, next: Status) {
    if (next === "banned" && !confirm(`Bannir ${user.display_name} ? Ses appareils sont déconnectés aussitôt.`)) return;
    busy = user.vrc_id;
    try {
      await api("POST", `/admin/users/${encodeURIComponent(user.vrc_id)}/status`, { status: next });
      await load();
    } catch (e) {
      error = (e as Error).message;
    } finally {
      busy = null;
    }
  }

  async function reject(user: User) {
    if (!confirm(`Refuser la demande de ${user.display_name} ? Le compte est supprimé.`)) return;
    busy = user.vrc_id;
    try {
      await api("DELETE", `/admin/users/${encodeURIComponent(user.vrc_id)}`);
      await load();
    } catch (e) {
      error = (e as Error).message;
    } finally {
      busy = null;
    }
  }

  onMount(() => {
    void load();
  });
</script>

<header>
  <h1>Utilisateurs</h1>
  <div class="controls">
    <div class="filters">
      {#each FILTERS as f (f.value)}
        <button class="ghost small" class:selected={status === f.value} onclick={() => { status = f.value; void load(); }}>
          {f.label}{#if f.value === "pending" && pending} <span class="count">{pending}</span>{/if}
        </button>
      {/each}
    </div>
    <input bind:value={q} oninput={search} placeholder="Pseudo ou usr_…" aria-label="Rechercher" />
  </div>
</header>

{#if error}<p class="error">{error}</p>{/if}

<div class="card table-card">
  <table>
    <thead>
      <tr><th>Modo</th><th>Statut</th><th>Rôle</th><th>Appareils</th><th>Dernière activité</th><th>Inscrit</th><th></th></tr>
    </thead>
    <tbody>
      {#each users as u (u.vrc_id)}
        <tr class="clickable" onclick={() => (openId = u.vrc_id)}>
          <td>
            <span class="dot" class:on={u.online}></span>
            <strong>{u.display_name}</strong>
            {#if u.vrc_id === self}<span class="muted small">(toi)</span>{/if}
            <br /><code>{u.vrc_id}</code>
          </td>
          <td><span class="badge {u.status}">{STATUS_LABELS[u.status]}</span></td>
          <td>{#if u.role === "admin"}<span class="badge admin">Admin</span>{:else}{ROLE_LABELS[u.role]}{/if}</td>
          <td>
            {u.devices}
            {#if u.pending_devices}<span class="badge pending">+{u.pending_devices} à valider</span>{/if}
          </td>
          <td title={date(u.last_seen_at)}>{ago(u.last_seen_at)}</td>
          <td>{date(u.created_at)}</td>
          <td class="actions" onclick={(e) => e.stopPropagation()}>
            {#if u.status === "pending" || u.pending_devices}
              <!-- Validation dans la fiche : le code de l'appareil doit être comparé. -->
              <button class="ok small" disabled={busy === u.vrc_id} onclick={() => (openId = u.vrc_id)}>Vérifier</button>
              {#if u.status === "pending"}
                <button class="danger small" disabled={busy === u.vrc_id} onclick={() => reject(u)}>Refuser</button>
              {/if}
            {:else if u.status === "banned"}
              <button class="ghost small" disabled={busy === u.vrc_id} onclick={() => setStatus(u, "active")}>Débannir</button>
            {:else if u.vrc_id !== self}
              <button class="danger small" disabled={busy === u.vrc_id} onclick={() => setStatus(u, "banned")}>Bannir</button>
            {/if}
          </td>
        </tr>
      {:else}
        <tr><td colspan="7" class="muted empty">Aucun compte.</td></tr>
      {/each}
    </tbody>
  </table>
</div>

{#if openId}
  <UserDetail id={openId} {self} onclose={() => (openId = null)} onchanged={load} />
{/if}

<style>
  header { display: flex; flex-wrap: wrap; justify-content: space-between; align-items: center; gap: 12px; margin-bottom: 16px; }
  h1 { font-size: 26px; }
  .controls { display: flex; flex-wrap: wrap; gap: 10px; }
  .filters { display: flex; gap: 4px; }
  .selected { background: #b49aff26; border-color: var(--accent); }
  .count { padding: 0 6px; border-radius: 8px; background: var(--warn); color: #171025; font-size: 11px; }
  .table-card { padding: 4px; overflow-x: auto; }
  table { min-width: 760px; }
  td .dot { margin-right: 6px; }
  .actions { white-space: nowrap; text-align: right; }
  .actions button + button { margin-left: 6px; }
  .empty { padding: 24px; text-align: center; }
  .error { margin-bottom: 12px; }
</style>
