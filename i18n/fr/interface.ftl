# Traduction de ../en/interface.ftl, sous les mêmes clés.

category-calibration = Calibration

category-decisions = Décisions

category-developer = Développement

category-experimental = Expérimental

category-masks = Masques

category-node-graph = Graphe de nœuds

category-preferences = Préférences

category-workspace = Espace de travail

clip-application-x-helios-images-json =
    .title = images

clip-application-x-helios-mask-json =
    .title = formes

clip-application-x-helios-nodes-json =
    .title = nœuds

clip-application-x-helios-param-json =
    .title = un paramètre

command-graph-edit =
    .description = Demande une modification à un éditeur de nœuds, faite à sa prochaine image comme celle de l’utilisateur (une connexion vérifiée par type, puis son étape graph.*) : `edit` vaut `connect` (`from`, `to`, et `replaces` : les liens dont elle déplace un bout, en une seule modification), `disconnect` (`edges`), `move` (`moves`), `delete` (`nodes`, `edges`) ou `add` (`nodes`).

command-graph-export =
    .description = Le graphe d’un éditeur de nœuds en image, tiré du graphe et de sa disposition tels que l’éditeur les a dessinés en dernier : `format` (svg par défaut ; png ou pdf s’ils sont compilés), `selection` (true : seulement les nœuds sélectionnés), `level` (overview, ports ou detail ; ports par défaut), `wires` (curves, orthogonal ou straight ; ceux de l’éditeur par défaut), `background` (false : transparent), `title`, `scale` (les pixels par point d’un PNG, 1 par défaut). Donne `svg` en texte, ou `png` ou `pdf` en base64.

command-graph-read =
    .description = Le graphe qu’un éditeur de nœuds a dessiné en dernier : nœuds (place, ports, groupe, paramètres), liens, groupes, les sortes qu’il peut ajouter, la sélection (`selection` : les nœuds, `selected_edges` : les liens), le niveau et la partie visible.

command-graph-select =
    .description = Sélectionne des nœuds (`nodes`, leurs ids) et des liens (`edges` : [{"{"}from, to{"}"}]) d’un éditeur de nœuds à sa prochaine image : ce qui est nommé devient la sélection, nœuds et liens ; donnez l’un des deux ou les deux.

command-graph-view =
    .description = Ajuste le graphe d’un éditeur de nœuds (`fit` : true), zoome à un niveau (`level` : overview, ports ou detail) ou cible un nœud (`focus`), à sa prochaine image.

frame-helios-compare =
    .title = Comparaison

frame-helios-comparison =
    .title = Comparer

frame-helios-culling =
    .title = Tri

frame-helios-darkroom =
    .title = Chambre noire

frame-helios-library =
    .title = Bibliothèque

frame-helios-mask-editing =
    .title = Édition de masque

frame-helios-sub-workspace =
    .title = Sous-espace de travail

frame-helios-workspace =
    .title = Réseau

menu-helios-file-importFrom =
    .label = Importer depuis

menu-helios-masks-row-add =
    .label = Ajouter

menu-helios-system-profiles =
    .label = Profils

menu-helios-system-recent =
    .label = Espaces de travail récents

menu-helios-system-themes =
    .label = Thèmes

plugin-egui-pop-imaging =
    .title = Outils d’image

plugin-helios =
    .title = Helios

plugin-helios-clipboard =
    .title = Types de presse-papiers d’Helios

plugin-helios-commands =
    .title = Helios

plugin-helios-contexts =
    .title = Lieux et modes d’Helios

plugin-helios-display =
    .title = Affichage

plugin-helios-frame =
    .title = Cadre de Helios

plugin-helios-history =
    .title = Historique de Helios

plugin-helios-lists =
    .title = Listes de Helios

plugin-helios-platform =
    .title = Plateforme de Helios

plugin-helios-theme-editor =
    .title = Éditeur de thème

plugin-helios-workbench =
    .title = Zones de Helios

setting-display-wide-gamut-preview =
    .description = Affiche l’image en Rec.2020 là où la fenêtre peut la porter (Wayland avec la gestion des couleurs), pour que les couleurs hors du sRGB ne soient pas écrêtées. Ailleurs, l’image reste en sRGB.
    .label = Aperçu en gamut étendu

setting-health-destinationDays =
    .description = Le bilan de santé avertit quand un disque où copie la stratégie de sauvegarde n'a pas été branché depuis plus longtemps.
    .label = Avertir quand un disque de sauvegarde est absent depuis (jours)

setting-notifications-jobs =
    .description = Affiche une notification sur le bureau quand une longue tâche (un export, un lot, un import, une sauvegarde, un bilan de santé) se termine ou échoue alors qu’aucune fenêtre de Helios n’a le focus, quelle qu’en soit l’origine.
    .label = Notifier la fin d’une longue tâche

setting-catalogue-snapshotEvery =
    .description = Helios prend un instantané vérifié du catalogue à cet intervalle, le garde à côté du catalogue et le copie sur les disques de sauvegarde branchés.
    .label = Instantané du catalogue toutes les (heures)

setting-catalogue-snapshotsKept =
    .description = Combien d'instantanés du catalogue sont gardés à côté du catalogue et sur chaque disque de sauvegarde ; les plus anciens partent d'abord.
    .label = Instantanés du catalogue gardés

setting-import-automatic-ask-files =
    .description = Un import automatique de plus de photos que cela demande d’abord, dans une notification qui ouvre la fenêtre d’import. 0 : ne demande jamais.
    .label = Demander avant d’importer plus de photos que

setting-import-automatic-ask-gb =
    .description = Un import automatique plus gros que cela demande d’abord, dans une notification qui ouvre la fenêtre d’import. 0 : ne demande jamais.
    .label = Demander avant d’importer plus de

setting-import-automatic-paused =
    .description = Activé, aucune source n’importe d’elle-même : les cartes branchées et les nouveaux fichiers des dossiers surveillés attendent qu’il soit désactivé.
    .label = Suspendre les imports automatiques

category-helios = Helios
