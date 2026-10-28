<script lang="ts">
  import { onMount } from "svelte";
  import { api } from "../lib/api";

  type ServerSettings = {
    registration_open: boolean;
    public_url: string;
    token_ttl_days: number;
  };

  let settings = $state<ServerSettings | null>(null);
  let error = $state("");
  let saved = $state(false);

  async function load() {
    try {
      settings = await api<ServerSettings>("GET", "/admin/settings");
    } catch (e) {
      error = (e as Error).message;
    }
  }

  async function toggleRegistration(open: boolean) {
    try {
      await api("POST", "/admin/settings", { registration_open: open });
      saved = true;
      setTimeout(() => (saved = false), 2000);
      await load();
    } catch (e) {
      error = (e as Error).message;
    }
  }

  onMount(() => {
    void load();
  });
</script>

<header><h1>Réglages</h1></header>

{#if error}<p class="error">{error}</p>{/if}

{#if settings}
  <section class="card">
    <h3>Inscriptions</h3>
    <label class="switch">
      <input type="checkbox" checked={settings.registration_open} onchange={(e) => toggleRegistration(e.currentTarget.checked)} />
      <div>
        <strong>Accepter les nouvelles demandes</strong>
        <span class="muted small">
          Ouvertes : tout compte VRChat peut demander l’accès, il reste en attente jusqu’à ta validation. Fermées :
          seuls les comptes déjà connus peuvent se connecter.
        </span>
      </div>
    </label>
    {#if saved}<span class="ok small">✓ Enregistré</span>{/if}
  </section>

  <section class="card">
    <h3>Validation des appareils</h3>
    <p>
      Chaque appli Sentinel se connecte avec sa propre clé, générée sur le PC du modo (la clé privée ne le quitte
      jamais). Un nouvel appareil reste en attente : compare son code avec celui que le modo voit dans
      <em>Paramètres → Base de données</em> avant de le valider.
    </p>
    <p class="muted small">
      Le compte VRChat annoncé par l’appli n’est pas vérifié auprès de VRChat : c’est ta validation qui l’établit.
      Confirme le code par un canal sûr (Discord, en jeu…).
    </p>
  </section>

  <section class="card">
    <h3>Serveur</h3>
    <dl>
      <dt>Adresse publique</dt><dd><code>{settings.public_url}</code></dd>
      <dt>Durée des jetons</dt><dd>{settings.token_ttl_days} jours sans utilisation (renouvelée à chaque usage)</dd>
    </dl>
    <p class="muted small">Ces valeurs se règlent dans le fichier .env (PUBLIC_URL, TOKEN_TTL_DAYS).</p>
  </section>
{/if}

<style>
  header { margin-bottom: 16px; }
  h1 { font-size: 26px; }
  h3 { margin-bottom: 10px; font-size: 15px; }
  section { display: grid; gap: 8px; margin-bottom: 14px; }
  .switch { display: flex; gap: 12px; align-items: flex-start; }
  .switch input { margin-top: 3px; accent-color: var(--accent); }
  .switch div { display: grid; gap: 3px; }
  p { margin: 0; }
  .ok { color: var(--ok); }
  dl { display: grid; grid-template-columns: 170px 1fr; gap: 6px 12px; margin: 0; }
  dt { color: var(--muted); }
  dd { margin: 0; }
  .error { margin-bottom: 12px; }
</style>
