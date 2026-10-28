<script lang="ts">
  import { onMount } from "svelte";
  import { api, type Overview } from "../lib/api";
  import { ago, bytes, count, duration } from "../lib/format";

  type Page = "overview" | "users" | "data" | "audit" | "settings";
  let { onnavigate }: { onnavigate: (page: Page) => void } = $props();

  let data = $state<Overview | null>(null);
  let error = $state("");

  async function load() {
    try {
      data = await api<Overview>("GET", "/admin/overview");
      error = "";
    } catch (e) {
      error = (e as Error).message;
    }
  }

  const maxRows = $derived(Math.max(1, ...(data?.tables.map((t) => t.rows) ?? [1])));

  onMount(() => {
    void load();
    const timer = setInterval(() => !document.hidden && void load(), 10_000);
    return () => clearInterval(timer);
  });
</script>

<header>
  <h1>Vue d’ensemble</h1>
  {#if data}<span class="muted small">v{data.version} · en ligne depuis {duration(data.uptime_secs)} · actualisé toutes les 10 s</span>{/if}
</header>

{#if error}<p class="error">{error}</p>{/if}

{#if data}
  {#if data.pending_devices}
    <button class="pending-banner" onclick={() => onnavigate("users")}>
      ⏳ {data.pending_devices} appareil(s) en attente de validation → Utilisateurs
    </button>
  {/if}

  <section class="stats">
    <article class="card"><span class="label">Modos actifs</span><strong>{count(data.users.active)}</strong><small class="muted">dont {data.users.admins} admin(s)</small></article>
    <article class="card"><span class="label">En ligne</span><strong class="ok">{data.online.length}</strong><small class="muted">{data.realtime_clients} appli(s) en temps réel</small></article>
    <article class="card"><span class="label">En attente</span><strong class:warn={data.users.pending}>{data.users.pending}</strong><small class="muted">{data.users.banned} banni(s)</small></article>
    <article class="card"><span class="label">Appareils</span><strong>{count(data.devices)}</strong><small class="muted" class:warn={data.pending_devices}>{data.pending_devices} en attente</small></article>
    <article class="card"><span class="label">Base</span><strong>{bytes(data.db_size_bytes)}</strong><small class="muted">écriture n° {count(data.head)}</small></article>
    <article class="card"><span class="label">Connexions</span><strong>{count(data.metrics.logins)}</strong><small class="muted" class:bad={data.metrics.login_failures}>{data.metrics.login_failures} refusée(s)</small></article>
  </section>

  <div class="columns">
    <section class="card">
      <h3>Modos en ligne</h3>
      {#if data.online.length}
        <ul class="online">
          {#each data.online as m (m.user_id)}
            <li>
              <span class="dot on"></span>
              <div>
                <strong>{m.display_name ?? m.user_id}</strong>
                <span class="muted small">
                  {m.world_name ? `${m.world_name} · ${m.user_count} joueurs` : "Pas dans une instance"}
                  {#if m.danger} · <span class="bad">{m.danger} danger</span>{/if}
                  {#if m.caution} · <span class="warn">{m.caution} à surveiller</span>{/if}
                </span>
              </div>
              <span class="muted small">{ago(m.last_seen_at)}</span>
            </li>
          {/each}
        </ul>
      {:else}
        <p class="muted small">Personne pour l’instant.</p>
      {/if}
    </section>

    <section class="card">
      <h3>Données partagées</h3>
      <ul class="bars">
        {#each data.tables as t (t.name)}
          <li>
            <span class="name">{t.name}</span>
            <span class="bar"><span style="width: {(t.rows / maxRows) * 100}%"></span></span>
            <span class="value">{t.exact ? "" : "≈ "}{count(t.rows)}</span>
          </li>
        {/each}
      </ul>
      <p class="muted small">
        Depuis le démarrage : {count(data.metrics.pushes)} envois ({count(data.metrics.rows_written)} lignes),
        {count(data.metrics.pulls)} relectures ({count(data.metrics.rows_read)} lignes).
      </p>
    </section>
  </div>
{/if}

<style>
  header { display: flex; flex-wrap: wrap; align-items: baseline; gap: 12px; margin-bottom: 18px; }
  h1 { font-size: 26px; }
  h3 { margin-bottom: 10px; font-size: 15px; }
  .pending-banner { width: 100%; justify-content: flex-start; margin-bottom: 14px; padding: 12px 16px; background: #ffcc3314; border: 1px solid #ffcc3366; color: var(--warn); }
  .error { margin-bottom: 14px; }
  .stats { display: grid; grid-template-columns: repeat(auto-fill, minmax(170px, 1fr)); gap: 12px; margin-bottom: 14px; }
  .stats article { display: grid; gap: 4px; }
  .label { color: var(--muted); font-size: 11px; letter-spacing: 0.06em; text-transform: uppercase; }
  .stats strong { font-size: 24px; }
  .ok { color: var(--ok); }
  .warn { color: var(--warn); }
  .bad { color: var(--bad); }
  .columns { display: grid; grid-template-columns: repeat(auto-fit, minmax(320px, 1fr)); gap: 14px; }
  .online { display: grid; gap: 8px; margin: 0; padding: 0; list-style: none; }
  .online li { display: flex; align-items: flex-start; gap: 10px; }
  .online li .dot { margin-top: 6px; }
  .online div { display: grid; flex: 1; gap: 2px; min-width: 0; }
  .bars { display: grid; gap: 6px; margin: 0 0 12px; padding: 0; list-style: none; }
  .bars li { display: grid; grid-template-columns: 130px minmax(0, 1fr) 80px; align-items: center; gap: 10px; font-size: 13px; }
  .name { overflow: hidden; text-overflow: ellipsis; color: var(--muted); }
  .bar { height: 8px; overflow: hidden; border-radius: 4px; background: #2a2434; }
  .bar span { display: block; height: 100%; border-radius: 4px; background: linear-gradient(90deg, var(--accent-2), var(--accent)); }
  .value { text-align: right; font-variant-numeric: tabular-nums; }
</style>
