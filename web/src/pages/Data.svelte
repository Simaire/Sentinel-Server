<script lang="ts">
  import { onMount } from "svelte";
  import { api, type TableInfo, type TableRows } from "../lib/api";
  import { count, date } from "../lib/format";

  let tables = $state<TableInfo[]>([]);
  let current = $state("");
  let rows = $state<TableRows | null>(null);
  let q = $state("");
  let offset = $state(0);
  let error = $state("");
  let selected = $state<(string | number | null)[] | null>(null);
  let timer: ReturnType<typeof setTimeout> | undefined;

  /** Colonnes de date en secondes Unix : affichées lisiblement. */
  const UNIX_COLUMNS = new Set(["updated_at", "created_at", "first_seen_at", "last_seen_at", "profile_fetched_at", "api_checked_at", "external_checked_at", "analysis_checked_at", "contributed_at"]);

  async function loadTables() {
    try {
      tables = (await api<{ tables: TableInfo[] }>("GET", "/admin/tables")).tables;
      if (!current && tables.length) await open(tables[0].name);
    } catch (e) {
      error = (e as Error).message;
    }
  }

  async function loadRows() {
    try {
      const params = new URLSearchParams({ q, offset: String(offset) });
      rows = await api<TableRows>("GET", `/admin/tables/${current}?${params}`);
      error = "";
    } catch (e) {
      error = (e as Error).message;
    }
  }

  async function open(name: string) {
    current = name;
    offset = 0;
    q = "";
    selected = null;
    await loadRows();
  }

  function search() {
    clearTimeout(timer);
    timer = setTimeout(() => {
      offset = 0;
      void loadRows();
    }, 250);
  }

  function show(column: string, value: string | number | null): string {
    if (value === null) return "—";
    if (typeof value === "number" && UNIX_COLUMNS.has(column) && value > 1_000_000_000) return date(value);
    const text = String(value);
    return text.length > 80 ? `${text.slice(0, 80)}…` : text;
  }

  function pretty(value: string | number | null): string {
    if (typeof value !== "string") return String(value);
    try {
      return JSON.stringify(JSON.parse(value), null, 2);
    } catch {
      return value;
    }
  }

  async function deleteReport(id: string) {
    if (!confirm("Supprimer ce signalement ? Il disparaît de toutes les applis.")) return;
    try {
      await api("DELETE", `/admin/reports/${encodeURIComponent(id)}`);
      selected = null;
      await loadRows();
    } catch (e) {
      error = (e as Error).message;
    }
  }

  onMount(() => {
    void loadTables();
  });
</script>

<header>
  <h1>Données</h1>
  <span class="muted small">Tables partagées entre les applis, les plus récentes modifications d’abord.</span>
</header>

{#if error}<p class="error">{error}</p>{/if}

<div class="layout">
  <nav class="card">
    {#each tables as t (t.name)}
      <button class="ghost table" class:selected={current === t.name} onclick={() => open(t.name)}>
        <span>{t.name}</span><span class="muted small">{t.exact ? "" : "≈ "}{count(t.rows)}</span>
      </button>
    {/each}
  </nav>

  <section class="card content">
    <div class="toolbar">
      <strong>{current}</strong>
      <input bind:value={q} oninput={search} placeholder="Rechercher (clés, noms, identifiants)…" aria-label="Rechercher" />
    </div>
    {#if rows}
      <div class="scroll">
        <table>
          <thead><tr>{#each rows.columns as c (c)}<th>{c}</th>{/each}</tr></thead>
          <tbody>
            {#each rows.rows as row, i (i)}
              <tr class="clickable" onclick={() => (selected = row)}>
                {#each row as value, j (j)}<td>{show(rows.columns[j], value)}</td>{/each}
              </tr>
            {:else}
              <tr><td colspan={rows.columns.length} class="muted empty">Aucune ligne.</td></tr>
            {/each}
          </tbody>
        </table>
      </div>
      <div class="pager">
        <button class="ghost small" disabled={offset === 0} onclick={() => { offset = Math.max(0, offset - rows!.page_size); void loadRows(); }}>← Précédent</button>
        <span class="muted small">lignes {offset + 1} – {offset + rows.rows.length}</span>
        <button class="ghost small" disabled={rows.rows.length < rows.page_size} onclick={() => { offset += rows!.page_size; void loadRows(); }}>Suivant →</button>
      </div>
    {/if}
  </section>
</div>

{#if selected && rows}
  <div class="backdrop" role="presentation" onclick={(e) => e.target === e.currentTarget && (selected = null)}>
    <div class="card dialog" role="dialog" aria-modal="true" aria-label="Détail de la ligne">
      <div class="dialog-head">
        <h3>{current}</h3>
        {#if current === "reports"}
          <button class="danger small" onclick={() => deleteReport(String(selected![0]))}>Supprimer le signalement</button>
        {/if}
        <button class="ghost small" onclick={() => (selected = null)}>Fermer</button>
      </div>
      <dl>
        {#each rows.columns as c, j (c)}
          <dt>{c}</dt>
          <dd><pre>{pretty(selected[j])}{#if typeof selected[j] === "number" && UNIX_COLUMNS.has(c) && Number(selected[j]) > 1_000_000_000} ({date(Number(selected[j]))}){/if}</pre></dd>
        {/each}
      </dl>
    </div>
  </div>
{/if}

<style>
  header { display: grid; gap: 4px; margin-bottom: 16px; }
  h1 { font-size: 26px; }
  .layout { display: grid; grid-template-columns: 220px minmax(0, 1fr); gap: 14px; }
  nav { display: grid; align-content: start; gap: 2px; padding: 8px; }
  .table { justify-content: space-between; width: 100%; border-color: transparent; background: transparent; font-weight: 500; }
  .table.selected { background: #b49aff1f; border-color: #b49aff55; }
  .content { display: grid; gap: 10px; min-width: 0; }
  .toolbar { display: flex; flex-wrap: wrap; align-items: center; justify-content: space-between; gap: 10px; }
  .toolbar input { width: min(340px, 100%); }
  .scroll { max-height: 62vh; overflow: auto; }
  td { max-width: 260px; overflow: hidden; text-overflow: ellipsis; white-space: nowrap; font-size: 12px; }
  .empty { padding: 20px; text-align: center; }
  .pager { display: flex; justify-content: space-between; align-items: center; }
  .backdrop { position: fixed; inset: 0; z-index: 20; display: grid; place-items: center; padding: 16px; background: #07050b99; }
  .dialog { width: min(820px, 100%); max-height: 88vh; overflow: auto; }
  .dialog-head { display: flex; flex-wrap: wrap; align-items: center; gap: 8px; margin-bottom: 10px; }
  .dialog-head h3 { flex: 1; }
  dl { display: grid; grid-template-columns: 180px minmax(0, 1fr); gap: 6px 12px; margin: 0; }
  dt { color: var(--muted); font-size: 12px; }
  dd { margin: 0; min-width: 0; }
  pre { margin: 0; max-height: 300px; overflow: auto; font: 12px/1.45 ui-monospace, Consolas, monospace; white-space: pre-wrap; overflow-wrap: anywhere; }
  .error { margin-bottom: 12px; }
  @media (max-width: 760px) {
    .layout { grid-template-columns: 1fr; }
    dl { grid-template-columns: 1fr; }
  }
</style>
