<script lang="ts">
  let { expired }: { expired: boolean } = $props();
</script>

<div class="login">
  <div class="card">
    <span class="logo">VS</span>
    <h1>Sentinel Server</h1>
    <p class="muted">Administration du serveur de synchronisation de VRC Sentinel.</p>
    {#if expired}
      <p class="error">Ce lien de connexion a expiré ou a déjà servi. Demandes-en un nouveau.</p>
    {/if}
    <h3>Se connecter</h3>
    <ol>
      <li>Dans l’appli <strong>VRC Sentinel</strong> d’un compte admin : <strong>Paramètres → Serveur Sentinel → Ouvrir l’administration</strong>.</li>
      <li>
        Ou sur la machine du serveur :
        <code>docker compose exec server sentinel-server admin-link usr_…</code>
      </li>
    </ol>
    <p class="muted small">
      Le lien n’est valable que 2 minutes et une seule fois. Aucun mot de passe n’est stocké ni demandé.
    </p>
  </div>
</div>

<style>
  .login { display: grid; place-items: center; min-height: 100vh; padding: 20px; }
  .card { display: grid; gap: 12px; max-width: 520px; padding: 28px; }
  .logo { display: grid; place-items: center; width: 52px; height: 52px; border-radius: 16px; background: linear-gradient(135deg, #8b5cf6, #3b82f6); font-size: 18px; font-weight: 900; }
  ol { display: grid; gap: 10px; margin: 0; padding-left: 20px; line-height: 1.5; }
  code { display: inline-block; margin-top: 4px; }
</style>
