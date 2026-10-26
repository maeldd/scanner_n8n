# Guide d'utilisation — Scanner Pièces + n8n

Ce guide est fait pour deux personnes :

- **l'opérateur** qui scanne les étiquettes au quotidien ;
- **l'administrateur** qui installe l'application et configure n8n + Notion.

> Règle d'or : **1 scan = 1 pièce**. La quantité n'est jamais dans le QR code.
> Le QR contient uniquement `IN|RÉFÉRENCE` (entrée) ou `OUT|RÉFÉRENCE` (sortie).

## Démarrage rapide

1. Installer l'application et jumeler le scanner en **SPP**.
2. Dans l'application : URL de production n8n, clé API, port série, `POSTE-01`.
3. Copier `references.csv` dans le dossier de données de l'application.
4. Créer les bases Notion `Références` et `Mouvements` (section 5).
5. Construire et activer le workflow n8n (section 6).
6. Tester : bouton **Tester le Webhook**, puis un scan `IN` et un scan `OUT`.

---

## 1. Vue d'ensemble

```text
Étiquette QR  ──►  Scanner Bluetooth  ──►  Application Scanner Pièces
                                                   │
                                                   │ 1 événement HTTP par mouvement
                                                   ▼
                                                n8n
                                                   │
                                                   ▼
                                                Notion
```

L'application garde les mouvements dans une file locale (`outbox.json`) avant
de les envoyer. Si le réseau ou n8n tombe, **aucun mouvement n'est perdu** :
l'application réessaie automatiquement au redémarrage.

---

## 2. Installation de l'application

### macOS

1. Télécharger `Scanner-n8n-1.2.0-arm64.dmg` depuis la page Releases.
2. Ouvrir le DMG et glisser l'application dans **Applications**.
3. Au premier lancement, si macOS bloque l'application : clic droit sur
   l'application → **Ouvrir**.

### Windows

1. Télécharger `Scanner-n8n-Setup-1.2.0-x64.exe`.
2. Lancer l'installateur et suivre les étapes.
3. Au premier lancement, si Windows SmartScreen apparaît : **Informations
   complémentaires** → **Exécuter quand même**.

### Connecter le scanner

Le scanner doit être en mode **SPP (Serial Port Profile)**, pas en mode HID
(clavier).

1. Mettre le scanner en mode SPP (voir la notice ou le QR code de configuration
   du fabricant).
2. Le jumeler dans les réglages Bluetooth de l'ordinateur.
3. Dans l'application, choisir le port série :
   - macOS : `/dev/cu.NomDuScanner` ;
   - Windows : `COM3`, `COM4`, etc.
4. Laisser **9600 bauds** sauf indication contraire du scanner.
5. Cocher **« Démarrer la capture au lancement »**.

> Si le scanner est en mode HID, il « tape » le code dans le champ qui a le
> focus et l'application ne le voit pas. Il faut le remettre en SPP.

---

## 3. Configurer l'application

Les réglages sont sauvegardés automatiquement dans :

- macOS : `~/Library/Application Support/scanner_n8n/scanner_config.json`
- Windows : `%APPDATA%\scanner_n8n\scanner_config.json`

| Réglage | Valeur |
|---|---|
| **URL du Webhook** | L'URL **de production** n8n, par exemple `https://TON-N8N/webhook/scanner-stock` |
| **Clé API** | Le même secret que le Header Auth n8n `X-Scanner-Key` (recommandé) |
| **Port série** | Le port du scanner SPP |
| **Vitesse** | `9600` bauds (en général) |
| **Poste / Device ID** | Le nom du poste, par exemple `POSTE-01` |
| **Démarrer la capture au lancement** | Coché |

Cliquer sur **Tester le Webhook** :

- si le test est OK, le journal affiche un message de succès ;
- le payload de test contient `is_test: true` et **ne doit rien écrire dans
  Notion**.

> Utiliser l'URL `/webhook/...` pour la production. L'URL `/webhook-test/...`
> ne fonctionne que lorsque le bouton **Listen for test event** est actif dans
> l'éditeur n8n.

### Catalogue local (noms et stock)

L'application affiche instantanément le nom et le stock local à partir de
`references.csv`. Ce fichier est un **cache** : Notion reste la source de
vérité.

Emplacement attendu par l'application packagée :

- macOS : `~/Library/Application Support/scanner_n8n/references.csv`
- Windows : `%APPDATA%\scanner_n8n\references.csv`

Exemple macOS :

```bash
mkdir -p "$HOME/Library/Application Support/scanner_n8n"
cp references.csv "$HOME/Library/Application Support/scanner_n8n/"
```

Le stock affiché au démarrage vient de ce fichier. Pour le rafraîchir,
réexporter les références depuis Notion et remplacer `references.csv`, puis
relancer l'application. Après chaque mouvement, le stock affiché est mis à jour
avec la réponse `stock_after` de n8n.

---

## 4. Utilisation quotidienne

### Scanner un mouvement

1. Vérifier que la capture est active (indicateur vert / bouton
   **Arrêter la capture** visible).
2. Scanner l'étiquette :
   - `IN|DCO001` = entrée de stock ;
   - `OUT|DCO001` = sortie de stock.
3. Le premier scan met la quantité à **1**.
4. Pour plusieurs pièces :
   - scanner à nouveau la **même** étiquette : `+1` à chaque scan ;
   - ou taper la quantité au clavier numérique : le premier chiffre remplace
     `1`, les suivants s'ajoutent ;
   - `+` / `-` ajustent de 1 ;
   - `Retour arrière` efface le dernier chiffre.
5. Valider :
   - **Entrée** valide immédiatement ;
   - **Échap** annule le mouvement en cours ;
   - scanner une **autre** référence valide automatiquement le mouvement
     précédent ;
   - sans action, le mouvement est validé automatiquement après **5 secondes**.

La carte du mouvement affiche le stock local projeté. Le stock serveur reste
la référence.

### Annuler le dernier mouvement

Pendant **10 secondes**, le bouton **↩ Annuler** reste disponible :

- si le mouvement n'est pas encore parti, il est supprimé localement ;
- s'il est déjà parti, l'application envoie un mouvement inverse avec le motif
  `Annulation`.

### Stock insuffisant (refus 409)

Si n8n répond `409 INSUFFICIENT_STOCK` :

1. le mouvement est rouvert avec la quantité refusée ;
2. la validation automatique et les nouveaux scans sont **mis en pause** ;
3. corriger la quantité avec le clavier ou `+` / `-` ;
4. appuyer sur **Entrée** pour renvoyer, ou **Échap** pour annuler.

C'est volontaire : cela évite de rejouer automatiquement le même mouvement
refusé.

### Étiquettes QR (hors application)

Les étiquettes sont produites **à l'extérieur de l'application**, par exemple
avec le logiciel de l'imprimante 3D. Chaque référence/bin doit avoir deux QR :

- `IN|RÉFÉRENCE` pour une entrée ;
- `OUT|RÉFÉRENCE` pour une sortie.

Le QR ne contient **ni quantité ni stock**. La référence doit être identique à
celle de `references.csv` / Notion. Tester la première étiquette avec le
scanner avant de lancer la production.

Pour régénérer le catalogue après un changement dans Notion, voir
`scripts/import-notion-csv.py` (README).

### Lire les indicateurs

| Indicateur | Signification |
|---|---|
| **MOUVEMENTS** | Mouvements créés pendant la session |
| **VALIDÉS** | Mouvements acceptés par n8n |
| **EN ATTENTE** | Mouvements dans la file locale, pas encore acceptés |
| **REFUSÉS** | Mouvements refusés (4xx métier), à corriger |

Le journal coloré indique ce qui se passe : scan, envoi, validation, erreur,
nouvelle tentative.

### Journal fichier (diagnostic)

Pour garder une trace sur le poste, lancer le binaire depuis un terminal avec
la variable `SCANNER_N8N_LOG_FILE`. Exemple macOS :

```bash
SCANNER_N8N_LOG_FILE="$HOME/scanner-pieces.log" \
  "/Applications/Scanner n8n.app/Contents/MacOS/scanner_n8n"
```

Le journal de l'interface est alors aussi écrit dans ce fichier. Sur Windows,
définir la variable dans le terminal ou le raccourci de lancement.

---

## 5. Préparer Notion

Créer (ou vérifier) deux bases.

### Base « Références »

| Propriété | Type | Exemple |
|---|---|---|
| `Référence` | Titre | `DCO001` |
| `Stock réel` | Nombre | `11` |
| `Nom` | Texte | `356-5538` (optionnel) |
| `Alerte stock` | Case à cocher / relation | (optionnel) |

> Le champ `sku` envoyé par l'application doit correspondre **exactement** au
> titre `Référence`. C'est la clé de rapprochement.
>
> Si votre base utilise `Stock en cours` au lieu de `Stock réel`, remplacez le
> nom dans le workflow n8n **et** dans le script d'import du catalogue.

### Base « Mouvements »

| Propriété | Type | Exemple |
|---|---|---|
| `Événement` | Titre | `3f1c9e2a-...` (event_id) |
| `Référence` | Relation → Références | `DCO001` |
| `Type` | Sélection | `Entrée` / `Sortie` |
| `Quantité` | Nombre | `4` |
| `Poste` | Texte | `POSTE-01` |
| `Date` | Date (avec heure) | `2026-10-26T10:00:00Z` |
| `Motif` | Texte | vide ou `Annulation` |
| `Annule` | Texte | event_id annulé (optionnel) |
| `Stock après` | Nombre | `12` |
| `Source` | Sélection | `Scanner` |

Créer les options de sélection `Entrée`, `Sortie` et `Scanner` avant le
premier test.

### Connecter Notion à n8n

1. Créer une intégration interne sur <https://www.notion.so/my-integrations>.
2. Copier le **token** de l'intégration.
3. Dans n8n, créer un identifiant **Notion** avec ce token.
4. Dans Notion, ouvrir chaque base → `•••` → **Connexions** → ajouter
   l'intégration.
5. Vérifier que les deux bases sont partagées avec l'intégration.

---

## 6. Construire le workflow n8n

### Principe

Le workflow doit :

1. recevoir un POST JSON ;
2. ignorer les tests (`is_test: true`) ;
3. vérifier que la requête est valide ;
4. vérifier si `event_id` a déjà été traité (idempotence) ;
5. retrouver la référence Notion ;
6. calculer le nouveau stock ;
7. refuser une sortie si le stock est insuffisant (HTTP 409) ;
8. mettre à jour le stock dans Notion ;
9. créer une ligne dans `Mouvements` ;
10. répondre à l'application avec `stock_after`.

### Créer le Webhook

1. Nouveau workflow → nœud **Webhook**.
2. Méthode : `POST`.
3. Path : `scanner-stock` (ou ce que vous voulez).
4. **Respond** : `Using Respond to Webhook Node`.
5. **Authentication** : `Header Auth`.
6. Créer l'identifiant :
   - Name : `X-Scanner-Key`
   - Value : un secret long et aléatoire.
7. Copier l'**URL de production** et la mettre dans l'application, avec la même
   clé API.

> Si vous ne mettez pas d'authentification, l'application fonctionne quand
> même, mais n'importe qui connaissant l'URL peut créer des mouvements.
> L'authentification est fortement recommandée.

### Nœud 1 — Normaliser

Ajouter un nœud **Code** nommé `Normaliser`, mode **Run Once for All Items** :

```js
const body = $json.body ?? $json;

return [{
  json: {
    event_id: body.event_id ?? '',
    sku: String(body.sku ?? '').trim().toUpperCase(),
    action: String(body.action ?? '').trim().toUpperCase(),
    qty: Number(body.qty ?? 0),
    station_id: body.station_id ?? body.device_id ?? '',
    scanned_at: body.scanned_at ?? body.timestamp ?? new Date().toISOString(),
    raw: body.raw ?? body.barcode ?? '',
    motif: body.motif ?? '',
    cancels_event_id: body.cancels_event_id ?? '',
    is_test: body.is_test === true,
  },
}];
```

Ce nœud accepte aussi bien le format actuel que les anciens champs
`barcode` / `timestamp` / `device_id`.

### Nœud 2 — Test ?

Ajouter un nœud **IF** nommé `Test ?` :

- Condition : `{{ $json.is_test }}` est vrai ;
- branche **true** → `Réponse test` ;
- branche **false** → `Valide ?`.

`Réponse test` : **Respond to Webhook**, code `200`, JSON :

```json
{ "ok": true, "message": "Test ignoré" }
```

### Nœud 3 — Valide ?

Ajouter un **IF** nommé `Valide ?` :

```js
{{ !!$json.event_id && ['IN','OUT'].includes($json.action) && Number($json.qty) > 0 && !!$json.sku }}
```

- **true** → `Mouvement déjà traité ?` ;
- **false** → `Réponse 400`.

`Réponse 400` : code `400`, JSON :

```json
{ "ok": false, "code": "BAD_REQUEST", "message": "Requête invalide" }
```

### Nœud 4 — Mouvement déjà traité ?

Ajouter un nœud **Notion** nommé `Mouvement déjà traité ?` :

- Resource : **Database Page**
- Operation : **Get Many**
- Database : `Mouvements`
- Limit : `1`
- Filter :

```json
{
  "property": "Événement",
  "title": {
    "equals": "{{ $('Normaliser').first().json.event_id }}"
  }
}
```

Puis un **IF** `Déjà traité ?` :

```js
{{ $json.results.length > 0 }}
```

- **true** → `Réponse doublon` ;
- **false** → `Trouver référence`.

`Réponse doublon` : code `200`, JSON :

```json
{
  "ok": true,
  "stock_after": {{ $json.results[0].properties['Stock après'].number }},
  "message": "Mouvement déjà enregistré"
}
```

C'est cette étape qui rend les retries de l'application sans danger : le même
`event_id` ne crée jamais deux mouvements.

### Nœud 5 — Trouver référence

Ajouter un nœud **Notion** nommé `Trouver référence` :

- Resource : **Database Page**
- Operation : **Get Many**
- Database : `Références`
- Limit : `1`
- Filter :

```json
{
  "property": "Référence",
  "title": {
    "equals": "{{ $('Normaliser').first().json.sku }}"
  }
}
```

Puis un **IF** `Référence trouvée ?` :

```js
{{ $json.results.length > 0 }}
```

- **true** → `Calculer stock` ;
- **false** → `Réponse 422`.

`Réponse 422` : code `422`, JSON :

```json
{ "ok": false, "code": "UNKNOWN_SKU", "message": "Référence inconnue" }
```

### Nœud 6 — Calculer stock

Ajouter un **Code** nommé `Calculer stock` :

```js
const mouvement = $('Normaliser').first().json;
const page = $('Trouver référence').first().json.results[0];
const props = page.properties;

// Adaptez le nom si votre base utilise « Stock en cours ».
const stockActuel = Number(props['Stock réel']?.number ?? 0);
const qty = Number(mouvement.qty);

const nouveauStock =
  mouvement.action === 'IN' ? stockActuel + qty : stockActuel - qty;

return [{
  json: {
    ...mouvement,
    page_id: page.id,
    stock_actuel: stockActuel,
    nouveau_stock: nouveauStock,
    insuffisant: mouvement.action === 'OUT' && nouveauStock < 0,
  },
}];
```

Puis un **IF** `Stock insuffisant ?` :

```js
{{ $json.insuffisant }}
```

- **true** → `Réponse 409` ;
- **false** → `Mettre à jour stock`.

`Réponse 409` : code `409`, JSON :

```json
{
  "ok": false,
  "code": "INSUFFICIENT_STOCK",
  "available": {{ $('Calculer stock').first().json.stock_actuel }},
  "message": "Stock insuffisant"
}
```

L'application rouvre alors le mouvement et attend une correction.

### Nœud 7 — Mettre à jour le stock

Ajouter un **Notion** nommé `Mettre à jour stock` :

- Resource : **Database Page**
- Operation : **Update**
- Database Page ID : `{{ $('Calculer stock').first().json.page_id }}`
- Propriété `Stock réel` (Nombre) :
  `{{ $('Calculer stock').first().json.nouveau_stock }}`

### Nœud 8 — Créer le mouvement

Ajouter un **Notion** nommé `Créer mouvement` :

- Resource : **Database Page**
- Operation : **Create**
- Database : `Mouvements`

Propriétés :

| Propriété | Valeur |
|---|---|
| `Événement` | `{{ $('Normaliser').first().json.event_id }}` |
| `Référence` | `{{ $('Calculer stock').first().json.page_id }}` |
| `Type` | `{{ $('Normaliser').first().json.action === 'IN' ? 'Entrée' : 'Sortie' }}` |
| `Quantité` | `{{ $('Normaliser').first().json.qty }}` |
| `Poste` | `{{ $('Normaliser').first().json.station_id }}` |
| `Date` | `{{ $('Normaliser').first().json.scanned_at }}` |
| `Motif` | `{{ $('Normaliser').first().json.motif }}` |
| `Annule` | `{{ $('Normaliser').first().json.cancels_event_id }}` |
| `Stock après` | `{{ $('Calculer stock').first().json.nouveau_stock }}` |
| `Source` | `Scanner` |

> Pour la relation `Référence`, sélectionner la relation puis coller l'ID de
> page fourni par `Calculer stock`.

### Nœud 9 — Réponse OK

Ajouter un **Respond to Webhook** nommé `Réponse OK` :

- Response Code : `200`
- Respond With : `JSON`
- Body :

```json
{
  "ok": true,
  "stock_after": {{ $('Calculer stock').first().json.nouveau_stock }},
  "message": "Mouvement enregistré"
}
```

### Activer

1. Sauvegarder le workflow.
2. Cliquer sur **Active**.
3. Mettre l'URL de production dans l'application.
4. Cliquer sur **Tester le Webhook** : le workflow doit répondre `200` et ne
   rien écrire dans Notion.

> Tant que le workflow n'est pas **Active**, l'URL de production répond `404`
> et l'application garde les mouvements en attente.

---

## 7. Tests recommandés

### Test 1 — Payload de test

Dans l'application : **Tester le Webhook**.

Attendu :

- réponse `200` ;
- message « Test ignoré » ;
- aucune ligne créée dans Notion.

### Test 2 — Entrée

Scanner une étiquette `IN|DCO001` avec la quantité `1`.

Attendu :

- l'application affiche **VALIDÉ** ;
- le stock Notion de `DCO001` augmente de 1 ;
- une ligne `Mouvements` est créée avec `Type = Entrée`.

### Test 3 — Doublon

Envoyer deux fois le même `event_id` avec `curl` :

```bash
curl -X POST "https://TON-N8N/webhook/scanner-stock" \
  -H "Content-Type: application/json" \
  -H "X-Scanner-Key: TON-SECRET" \
  -H "Idempotency-Key: test-event-001" \
  -d '{
    "event_id": "test-event-001",
    "sku": "DCO001",
    "action": "IN",
    "qty": 1,
    "station_id": "POSTE-01",
    "scanned_at": "2026-10-26T10:00:00Z",
    "raw": "IN|DCO001",
    "is_test": false
  }'
```

Attendu :

- premier appel : `200`, `stock_after` = nouveau stock ;
- deuxième appel : `200`, message « Mouvement déjà enregistré » ;
- **le stock ne change pas** la deuxième fois.

### Test 4 — Stock insuffisant

Scanner `OUT|DCO001` avec une quantité supérieure au stock.

Attendu :

- n8n répond `409 INSUFFICIENT_STOCK` ;
- l'application rouvre le mouvement ;
- corriger la quantité puis valider ;
- le stock Notion ne doit pas être négatif.

### Test 5 — Panne réseau

1. Couper n8n ou le réseau.
2. Scanner un mouvement.
3. L'application doit afficher **EN ATTENTE**.
4. Rétablir n8n.
5. L'application renvoie automatiquement le mouvement et affiche
   **VALIDÉ**.

### Test 6 — Annulation

1. Scanner un mouvement.
2. Cliquer sur **↩ Annuler** dans les 10 secondes.
3. Vérifier dans Notion :
   - soit le mouvement a disparu s'il n'était pas encore envoyé ;
   - soit un mouvement inverse avec le motif `Annulation` a été créé.

---

## 8. Dépannage

| Problème | Solution |
|---|---|
| Le port n'apparaît pas | Vérifier le jumelage SPP, fermer les autres applications qui utilisent le port, débrancher/rebrancher, redémarrer l'application. |
| Le scanner bipe mais rien ne s'affiche | Le scanner est probablement en mode HID. Le remettre en SPP. Vérifier que le code commence par `IN` ou `OUT`. |
| « URL de webhook vide » | Renseigner l'URL de production dans l'application. |
| Le test échoue | Workflow non actif, URL de test sans « Listen for test event », mauvaise clé API, pare-feu. |
| Erreur 401 / 403 | La clé API de l'application ne correspond pas au Header Auth n8n. |
| Erreur 404 | Mauvaise URL ou workflow non activé. |
| Erreur 409 | Stock insuffisant : corriger la quantité ou réapprovisionner. |
| Erreur 422 | Référence absente de Notion ou titre différent. Corriger le titre, puis rescanner. |
| « REFUSÉS » augmente | Consulter le journal, corriger la cause, puis **Acquitter les refusés**. |
| « EN ATTENTE » augmente | n8n est injoignable ou lent. Les mouvements sont dans `outbox.json` ; ne pas supprimer ce fichier. |
| Le QR ne scanne pas | Vérifier le QR généré par le logiciel de l'imprimante 3D : contenu `IN` / `OUT` + référence, taille, contraste, zone blanche autour. Tester avec un téléphone. |
| Le stock affiché est faux au démarrage | Le stock local vient de `references.csv`. Réexporter le catalogue depuis Notion et relancer l'application. |

---

## 9. Fichiers et sauvegarde

Dossier de données :

- macOS : `~/Library/Application Support/scanner_n8n/`
- Windows : `%APPDATA%\scanner_n8n\`

Contenu :

| Fichier | Rôle |
|---|---|
| `scanner_config.json` | URL, port série, poste, clé API |
| `references.csv` | Cache local des références et du stock |
| `outbox.json` | Mouvements en attente / envoyés / refusés |

Sauvegarder ce dossier régulièrement. Ne pas éditer `outbox.json` à la main.

---

## 10. Sécurité et limites

- Utiliser l'URL **de production** et un Header Auth.
- Ne jamais partager `scanner_config.json` ni le secret `X-Scanner-Key`.
- Si une URL ou un secret a été exposé, le révoquer et le recréer.
- Une seule station est prévue pour l'instant.
- Notion n'est pas transactionnel. Si une erreur technique survient entre la
  mise à jour du stock et la création du mouvement, vérifier le stock dans
  Notion et corriger manuellement si nécessaire. Pour plusieurs stations ou des
  garanties plus fortes, déplacer la logique de stock vers Postgres/Supabase
  derrière n8n. Le contrat application ↔ n8n reste le même.
- Pas de suivi de lot ou de numéro de série.
- Les quantités sont des pièces entières.

---

## 11. Résumé express

**Opérateur**

1. Vérifier que la capture est active.
2. Scanner `IN|...` ou `OUT|...`.
3. Ajuster la quantité si besoin.
4. `Entrée` pour valider, `Échap` pour annuler.
5. En cas de refus, corriger la quantité et revalider.

**Administrateur**

1. Installer l'application et jumeler le scanner en SPP.
2. Configurer l'URL de production, la clé API, le port et le poste.
3. Copier `references.csv` dans le dossier de données.
4. Créer les bases Notion `Références` et `Mouvements`.
5. Importer le workflow n8n décrit ci-dessus et l'activer.
6. Tester : test payload, entrée, sortie, doublon, 409, panne réseau.
