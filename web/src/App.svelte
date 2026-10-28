<script lang="ts">
  import { onMount } from "svelte";
  import { api, SESSION_LOST } from "./lib/api";
  import Overview from "./pages/Overview.svelte";
  import Users from "./pages/Users.svelte";
  import Data from "./pages/Data.svelte";
  import Audit from "./pages/Audit.svelte";
  import Settings from "./pages/Settings.svelte";
  import Login from "./pages/Login.svelte";

  type Page = "overview" | "users" | "data" | "audit" | "settings";
  const PAGES: { id: Page; icon: string; label: string }[] = [
    { id: "overview", icon: "◈", label: "Vue d'ensemble" },
    { id: "users", icon: "👥", label: "Utilisateurs" },
    { id: "data", icon: "🗄", label: "Données" },
    { id: "audit", icon: "📜", label: "Journal" },
    { id: "settings", icon: "⚙", label: "Réglages" }
  ];

  let me = $state<{ user_id: string; display_name: string } | null>(null);
  let checked = $state(false);
  let page = $state<Page>((location.hash.slice(1) as Page) || "overview");
  const expired = new URLSearchParams(location.search).get("login") === "expired";

  function go(next: Page) {
    page = next;
    history.replaceState(null, "", `#${next}`);
  }

  async function logout() {
    try {
      await api("POST", "/admin/logout");
    } finally {
      me = null;
    }
  }

  onMount(() => {
    api<{ user_id: string; display_name: string }>("GET", "/admin/me")
      .then((value) => (me = value))
      .catch(() => (me = null))
      .finally(() => (checked = true));
    const lost = () => (me = null);
    window.addEventListener(SESSION_LOST, lost);
    return () => window.removeEventListener(SESSION_LOST, lost);
  });
</script>

{#if !checked}
  <p class="muted loading">Chargement…</p>
{:else if !me}
  <Login {expired} />
{:else}
  <div class="shell">
    <aside>
      <div class="brand">
        <span class="logo">VS</span>
        <div>
          <strong>Sentinel Server</strong>
          <span class="muted small">Administration</span>
        </div>
      </div>
      <nav>
        {#each PAGES as item (item.id)}
          <button class="nav" class:active={page === item.id} onclick={() => go(item.id)}>
            <span class="icon">{item.icon}</span>{item.label}
          </button>
        {/each}
      </nav>
      <div class="me">
        <span class="muted small">Connecté en tant que</span>
        <strong>{me.display_name}</strong>
        <button class="ghost small" onclick={logout}>Se déconnecter</button>
      </div>
    </aside>
    <main>
      {#if page === "overview"}
        <Overview onnavigate={go} />
      {:else if page === "users"}
        <Users self={me.user_id} />
      {:else if page === "data"}
        <Data />
      {:else if page === "audit"}
        <Audit />
      {:else}
        <Settings />
      {/if}
    </main>
  </div>
{/if}

<style>
  .loading { padding: 40px; text-align: center; }
  .shell { display: grid; grid-template-columns: 230px minmax(0, 1fr); min-height: 100vh; }
  aside { position: sticky; top: 0; display: flex; flex-direction: column; gap: 18px; height: 100vh; padding: 20px 14px; border-right: 1px solid var(--line); background: #120f18cc; }
  .brand { display: flex; align-items: center; gap: 10px; }
  .brand div { display: grid; }
  .logo { display: grid; place-items: center; width: 38px; height: 38px; border-radius: 12px; background: linear-gradient(135deg, #8b5cf6, #3b82f6); font-weight: 900; }
  nav { display: grid; gap: 4px; }
  .nav { justify-content: flex-start; width: 100%; padding: 10px 12px; background: transparent; color: var(--muted); font-weight: 600; }
  .nav:hover { background: #ffffff08; filter: none; }
  .nav.active { background: #b49aff1f; color: var(--text); }
  .icon { width: 22px; text-align: center; }
  .me { display: grid; gap: 4px; margin-top: auto; }
  .me button { margin-top: 6px; }
  main { min-width: 0; padding: 28px clamp(16px, 4vw, 44px); }
  @media (max-width: 760px) {
    .shell { grid-template-columns: 1fr; }
    aside { position: static; height: auto; }
    nav { grid-template-columns: repeat(auto-fill, minmax(130px, 1fr)); }
    .me { margin-top: 0; }
  }
</style>
