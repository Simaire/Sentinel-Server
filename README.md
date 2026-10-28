# VRC Sentinel Server

Serveur de synchronisation des applis **VRC Sentinel** : une mémoire commune pour toute l'équipe de modération, avec une interface web de gestion.

- **Rust (axum) + PostgreSQL**, livré en image Docker de 15 Mo (binaire statique, image distroless sans shell, utilisateur non root, système de fichiers en lecture seule).
- **HTTPS automatique** par Caddy (Let's Encrypt).
- **Connexion par clé d'appareil** (Ed25519) : ni mot de passe, ni secret transmis, ni compte VRChat côté serveur.
- **Chaque appareil validé par un admin** d'après son code, révocable à tout moment, journal d'audit.

## Fonctionnement

```
Appli Sentinel ──HTTPS──▶ Caddy ──▶ sentinel-server ──▶ PostgreSQL (réseau interne, aucun port publié)
```

- **Envoi** (`POST /api/v1/sync/push`) : chaque ligne est vérifiée (colonnes, types, tailles, date plafonnée à « maintenant ») puis écrite au nom du compte connecté. Un modo ne peut pas signaler, voter ou sanctionner au nom d'un autre, ni modifier ou supprimer ce qu'un autre a écrit (sauf un admin).
- **Écriture groupée** : une seule tâche écrit en base. Les envois simultanés sont regroupés dans une transaction, une requête `UNNEST` par table. Chaque ligne reçoit un numéro d'écriture croissant (`seq`).
- **Relecture** (`POST /api/v1/sync/pull`) : chaque appli demande ce qui suit son dernier numéro reçu. Ce curseur est exact, et rien n'est ni manqué ni relu.
- **Temps réel** (`GET /api/v1/sync/wait`) : attente longue de 25 s au plus, qui répond dès qu'une écriture arrive. Aucune requête n'est envoyée en boucle à vide.
- **Présence** (`/api/v1/presence`) : les modos en ligne sont gardés en mémoire, sans écriture en base à chaque actualisation.

### Connexion d'un modo

1. À la première connexion, l'appli crée une paire de clés Ed25519. La clé privée reste dans le coffre-fort du système (Windows Credential Manager).
2. L'appli demande un défi (`POST /api/v1/auth/challenge`), puis le signe avec l'adresse du serveur, le compte VRChat connecté dans Sentinel et sa clé publique (`POST /api/v1/auth/login`).
3. Le serveur vérifie la signature et délivre un jeton de session (`sst_…`, 256 bits). Il n'en garde que l'empreinte SHA-256.
4. Un nouvel appareil reste **en attente**. Son code (`ABCD-EF12-3456-7890`, empreinte de la clé publique) s'affiche dans l'appli et dans l'administration. L'admin le compare avec celui que lui donne le modo, puis valide.
5. Ensuite, tout est automatique : quand la session expire, l'appli en rouvre une en signant un nouveau défi.

Le serveur n'interroge jamais VRChat. Le compte annoncé par l'appli est établi par la validation de l'admin, qui confirme le code par un canal sûr (Discord, en jeu…).

## Déploiement

Il faut une machine avec Docker et un nom de domaine qui pointe vers elle, ports 80 et 443 ouverts.

```bash
cp .env.example .env
```

Puis remplir `.env` :

| Variable | Rôle |
| --- | --- |
| `DOMAIN` | Domaine du serveur (`localhost` pour un essai local) |
| `POSTGRES_PASSWORD` | Mot de passe de la base, par exemple `openssl rand -hex 32` |
| `TOKEN_TTL_DAYS` | Durée d'une session sans utilisation (30 jours par défaut, prolongée à chaque usage ; l'appli en rouvre une seule avec sa clé) |

```bash
docker compose up -d --build
```

Chaque variable accepte aussi une forme `*_FILE` qui lit la valeur dans un fichier (secrets Docker).

### Derrière ton propre reverse proxy

Si la machine a déjà un proxy (nginx, Traefik, Nginx Proxy Manager, Caddy, Apache, Cloudflare Tunnel…), utilise `docker-compose.proxy.yml` : serveur et PostgreSQL, sans le Caddy intégré. Le serveur écoute en HTTP sur `127.0.0.1:8080`, et ton proxy termine le HTTPS.

```bash
docker compose -f docker-compose.proxy.yml up -d --build
```

| Variable | Rôle |
| --- | --- |
| `PUBLIC_URL` | Adresse vue par les applis et les navigateurs : `https://sentinel.exemple.fr`, ou un sous-chemin comme `https://exemple.fr/sentinel` |
| `TRUSTED_PROXIES` | Proxys autorisés à transmettre l'adresse du client : IP ou plages CIDR séparées par des virgules. `private` (par défaut) couvre cette machine et les réseaux privés (Docker, réseau local) |
| `REAL_IP_HEADER` | En-tête qui porte l'adresse du client : `x-forwarded-for` (par défaut), `forwarded`, `x-real-ip`, `cf-connecting-ip`… |
| `SERVER_PORT`, `BIND_HOST` | Port et interface publiés (`127.0.0.1:8080` par défaut) |

Exemples prêts à adapter dans [deploy/](deploy) : [nginx](deploy/nginx.conf), [Caddy](deploy/Caddyfile), [Traefik](deploy/traefik.yml). Avec Nginx Proxy Manager, crée un *Proxy Host* vers `http://<IP de la machine>:8080`, ou vers `server:8080` si NPM partage le réseau Docker. Ajoute aussi `client_max_body_size 16m;` dans l'onglet *Advanced*.

Points à respecter quel que soit le proxy :

- **Taille des envois** : jusqu'à 16 Mo. nginx bloque à 1 Mo par défaut.
- **Délai** : au moins 30 s de lecture. L'attente longue de la synchronisation dure 25 s.
- **Adresse du client** : le proxy doit ajouter `X-Forwarded-For`, ou l'en-tête choisi dans `REAL_IP_HEADER`. Le serveur ne le lit que si la connexion vient d'un proxy de `TRUSTED_PROXIES`. Il prend la première adresse en partant de la droite qui n'est pas un proxy. Une adresse inventée par le client, placée à gauche, est donc ignorée. Sans cela, les limites de débit et le journal d'audit ne verraient que l'IP du proxy.
- **Sous-chemin** : le proxy peut transmettre le préfixe (`/sentinel/…`) ou le retirer, les deux fonctionnent. Les redirections, les liens d'administration et le cookie de session suivent `PUBLIC_URL`. Le cookie est limité au chemin `/sentinel/`.
- **Cloudflare** (proxy orange ou Tunnel) : `REAL_IP_HEADER=cf-connecting-ip`, et `TRUSTED_PROXIES` avec les plages de Cloudflare (ou `private` pour un Tunnel local).

L'adresse saisie dans les applis doit être exactement `PUBLIC_URL` : l'appli refuse de signer si le serveur annonce une autre adresse.

### Premier admin

1. Dans ton appli : **Paramètres → Base de données**, adresse du serveur, puis **Se connecter**. L'appli affiche le code de ton appareil, par exemple `ABCD-EF12-3456-7890`.
2. Sur le serveur :

```bash
docker compose exec server sentinel-server approve ABCD-EF12-3456-7890 --admin
```

La commande valide ton appareil, te donne le rôle d'admin et affiche un lien vers l'administration. Les autres modos se valident ensuite depuis l'interface web.

### Ouvrir l'administration

Depuis l'appli d'un admin : **Paramètres → Base de données → Ouvrir l'administration**.

Ou depuis le serveur :

```bash
docker compose exec server sentinel-server admin-link usr_xxxxxxxx-xxxx-xxxx-xxxx-xxxxxxxxxxxx
```

Le lien est à usage unique et valable 2 minutes. Il ouvre une session web de 12 h.

L'interface (`/admin/`) permet de :

- valider les nouveaux appareils après comparaison de leur code ;
- bannir, promouvoir ou annoter les comptes ;
- révoquer un appareil ou tous ceux d'un compte ;
- parcourir toutes les tables et supprimer un signalement ;
- consulter le journal d'audit ;
- fermer les inscriptions.

## Sécurité

- Aucun secret des modos ne transite : ni mot de passe, ni cookie VRChat, ni clé privée. Le serveur ne connaît que la clé publique de chaque appareil.
- La signature couvre un défi à usage unique (2 min), l'adresse du serveur, le compte et la clé : elle ne peut être ni rejouée ni réutilisée auprès d'un autre serveur.
- Une clé reste liée au compte avec lequel elle a été validée. Un autre PC qui annonce le même compte est un nouvel appareil, en attente lui aussi.
- Jetons de session et liens d'administration aléatoires (256 bits), stockés seulement sous forme d’empreinte.
- Cookie d'administration `HttpOnly`, `SameSite=Strict`, `Secure` en HTTPS. En-tête anti-CSRF exigé sur toute modification. CSP stricte et en-têtes de sécurité sur toutes les réponses.
- Limites de débit par IP et par compte (défis, connexions, synchronisation), sur l'adresse réelle du client derrière les proxys de confiance. Corps de requête limité à 16 Mo et délai maximal de 40 s par requête.
- PostgreSQL n'est joignable que par le serveur, sur un réseau Docker interne sans accès à Internet.
- Avec `docker-compose.yml`, le serveur lui-même n'a aucun accès à Internet : seul Caddy sort, pour les certificats HTTPS.
- Conteneurs sans privilèges (`cap_drop: ALL`, `no-new-privileges`) et serveur en lecture seule.

## Sauvegarde

```bash
docker compose exec db pg_dump -U sentinel sentinel | gzip > sentinel-$(date +%F).sql.gz
```

## Développement

### Serveur de test sur ce PC

`docker-compose.local.yml` lance PostgreSQL et le serveur, sans Caddy ni HTTPS, en écoute sur `127.0.0.1` seulement :

```bash
docker compose -f docker-compose.local.yml up -d --build
```

1. Dans l'appli, adresse `http://localhost:8080` (exactement : c'est le `PUBLIC_URL` du test), puis **Se connecter au serveur**.
2. Valide ton appareil et passe admin : `docker compose -f docker-compose.local.yml exec server sentinel-server approve <code> --admin`. Le lien affiché ouvre l'administration.
3. Journal : `docker compose -f docker-compose.local.yml logs -f server`. La base est joignable sur `127.0.0.1:55432` (`sentinel` / `test`).
4. Arrêt : `docker compose -f docker-compose.local.yml down`, avec `-v` en plus pour effacer les données.

### Sans Docker

```bash
cd web && npm ci && npm run build && cd ..
DATABASE_URL=postgres://… PUBLIC_URL=http://localhost:8080 cargo run -- serve
```

L'interface se développe à part avec `cd web && npm run dev`, qui redirige `/api` vers `localhost:8080`.

Tests :

```bash
cargo test
SENTINEL_TEST_DATABASE=postgres://… cargo test -- --ignored
```

La commande `--ignored` lance les tests d'intégration et demande une base vide.
