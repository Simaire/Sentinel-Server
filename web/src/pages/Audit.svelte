<script lang="ts">
  import { onMount } from "svelte";
  import { api, type AuditEntry } from "../lib/api";
  import { ACTION_LABELS, date } from "../lib/format";

  let entries = $state<AuditEntry[]>([]);
  let pageSize = $state(100);
  let offset = $state(0);
  let q = $state("");
  let error = $state("");
  let timer: ReturnType<typeof setTimeout> | undefined;

  async function load() {
    try {
      const params = new URLSearchParams({ q, offset: String(offset) });
      const data = await api<{ entries: AuditEntry[]; page_size: number }>("GET", `/admin/audit?${params}`);
      entries = data.entries;
      pageSize = data.page_size;
      error = "";
    } catch (e) {
      error = (e as Error).message;
    }
  }

  function search() {
    clearTimeout(timer);
    timer = setTimeout(() => {
      offset = 0;
      void load();
    }, 250);
  }

  onMount(() => {
    void load();
  });
</script>

<header>
  <h1>Journal d’audit</h1>
  <input bind:value={q} oninput={search} placeholder="Action, compte, pseudo…" aria-label="Rechercher" />
</header>

{#if error}<p class="error">{error}</p>{/if}

<div class="card">
  <table>
    <thead><tr><th>Date</th><th>Action</th><th>Par</th><th>Sur</th><th>Détail</th></tr></thead>
    <tbody>
      {#each entries as e (e.id)}
        <tr>
          <td class="nowrap">{date(e.at)}</td>
          <td>{ACTION_LABELS[e.action] ?? e.action}</td>
          <td>{e.actor_name ?? e.actor ?? "—"}</td>
          <td><code>{e.target ?? ""}</code></td>
          <td class="muted small">{e.detail ?? ""}</td>
        </tr>
      {:else}
        <tr><td colspan="5" class="muted empty">Rien à afficher.</td></tr>
      {/each}
    </tbody>
  </table>
  <div class="pager">
    <button class="ghost small" disabled={offset === 0} onclick={() => { offset = Math.max(0, offset - pageSize); void load(); }}>← Plus récents</button>
    <button class="ghost small" disabled={entries.length < pageSize} onclick={() => { offset += pageSize; void load(); }}>Plus anciens →</button>
  </div>
</div>

<style>
  header { display: flex; flex-wrap: wrap; justify-content: space-between; align-items: center; gap: 12px; margin-bottom: 16px; }
  h1 { font-size: 26px; }
  input { width: min(320px, 100%); }
  .card { overflow-x: auto; }
  table { min-width: 680px; }
  .nowrap { white-space: nowrap; }
  .empty { padding: 20px; text-align: center; }
  .pager { display: flex; justify-content: space-between; margin-top: 10px; }
  .error { margin-bottom: 12px; }
</style>
