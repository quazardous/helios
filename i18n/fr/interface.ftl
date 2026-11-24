# Traduction de ../en/interface.ftl, sous les mêmes clés.

category-calibration = Calibration

category-controls = Contrôles

category-decisions = Décisions

category-developer = Développement

category-display = Affichage

category-experimental = Expérimental

category-masks = Masques

category-node-graph = Graphe de nœuds

category-preferences = Préférences

category-queue = File d’attente

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

command-help-report-bundle =
    .description = Ce que joint Signaler un problème, en JSON : les dernières minutes du journal, expurgées, les versions, les points du bus et le contexte de l’application.

command-window-close =
    .description = Ferme une fenêtre : celle nommée, ou la fenêtre active.

command-window-pick =
    .description = Liste les fenêtres ouvertes par titre ; celle choisie passe au premier plan.

command-window-switch =
    .description = Fait passer une fenêtre au premier plan : celle nommée, ou la suivante.

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

menu-app-menu-window =
    .label = Fenêtre

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

menu-helios-window-panels =
    .label = Panneaux

menu-helios-window-studio =
    .label = Studio

plugin-egui-pop-imaging =
    .title = Outils d’image

plugin-egui-pop-windows =
    .title = Fenêtres

plugin-helios =
    .title = Helios

plugin-helios-agent-state =
    .title = État de l’agent

plugin-helios-clipboard =
    .title = Types de presse-papiers d’Helios

plugin-helios-commands =
    .title = Helios

plugin-helios-config =
    .title = Réglages de Helios

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

plugin-helios-quit =
    .title = Fermeture de Helios

plugin-helios-theme-editor =
    .title = Éditeur de thème

plugin-helios-windowMenu =
    .title = Menu Fenêtre de Helios

plugin-helios-workbench =
    .title = Zones de Helios

plugin-helios-workflow =
    .title = Files d’attente des flux de Helios

setting-display-wide-gamut-preview =
    .description = Affiche l’image en Rec.2020 là où la fenêtre peut la porter (Wayland avec la gestion des couleurs), pour que les couleurs hors du sRGB ne soient pas écrêtées. Ailleurs, l’image reste en sRGB.
    .label = Aperçu en gamut étendu

setting-health-destinationDays =
    .description = Le bilan de santé avertit quand un disque où copie la stratégie de sauvegarde n'a pas été branché depuis plus longtemps.
    .label = Avertir quand un disque de sauvegarde est absent depuis (jours)

setting-notifications-jobs =
    .description = Affiche une notification sur le bureau quand une longue tâche (un export, un lot, un import, une sauvegarde, un bilan de santé) se termine ou échoue alors qu’aucune fenêtre de Helios n’a le focus, quelle qu’en soit l’origine.
    .label = Notifier la fin d’une longue tâche

setting-project-decisions-mode =
    .description = Comment les nœuds de décision sont résolus par défaut dans ce projet.
    .label = Nœuds de décision

setting-project-decisions-mode-choice-ai-assisted = assisté par l’IA

setting-project-decisions-mode-choice-full-ai = tout par l’IA

setting-project-decisions-mode-choice-human = humain

setting-system-ai-calls =
    .description = Combien d’appels aux modèles d’IA (nœuds de décision) tournent en même temps.
    .label = Appels de modèle simultanés

setting-system-cache-budget =
    .description = La mémoire que le cache de rendu peut utiliser avant d’évincer ce qui a servi le moins récemment ; 0 : une part de la mémoire de la machine (le réglage ci-dessous).
    .label = Cache de rendu

setting-system-cache-share =
    .description = La part de la mémoire de la machine que le cache de rendu peut utiliser quand aucune taille n’est donnée ci-dessus ; 0 : sans limite.
    .label = Part du cache de rendu

setting-system-cache-states =
    .description = La mémoire que peuvent occuper les états calculés des photos (l’étape pleine taille de la photo affichée, gardée pour que l’ajustement et un zoom ne la recalculent pas, et ceux des photos récentes) ; 0 : 12,5 % de la mémoire de la machine, entre 1 et 4 Gio.
    .label = États de photo gardés

setting-system-cache-textures =
    .description = La mémoire graphique que peuvent occuper les vignettes des grilles ; au-delà, les vignettes hors de vue sont libérées, les plus anciennement vues d’abord, et relues depuis le cache des vignettes quand elles reviennent.
    .label = Textures des vignettes

setting-system-cache-thumbnails =
    .description = L’espace disque que le cache des vignettes de la bibliothèque (.helios/thumbs à côté du catalogue) peut utiliser avant d’évincer les vignettes les moins récemment utilisées.
    .label = Cache des vignettes

setting-system-events-journal =
    .description = Combien d’événements récents le bus garde pour l’inspection et les attentes.
    .label = Journal des événements

setting-system-map-defaults =
    .description = Si la carte des images laisse de côté les dossiers du système, les données propres de Helios, les caches, les arbres de compilation, les dossiers cachés, la corbeille et les bibliothèques des autres applications photo ; désactivé, seules les règles ci-dessus restent.
    .label = Liste d’exclusion par défaut

setting-system-map-exclude =
    .description = Des règles ajoutées à la liste d’exclusion de la carte des images, en liste JSON : chaque `{"{"}"glob": "**/Private/**"{"}"}` correspond au chemin absolu, ou avec `"volume": "<id>"` au chemin dans ce volume ; un glob qui commence par `!` réinclut ce qu’un défaut exclut. La dernière règle qui correspond l’emporte.
    .label = Lieux jamais enregistrés

setting-system-map-keepMissing =
    .description = Combien de temps un fichier vu mais hors de la Bibliothèque reste dans la carte des images une fois absent de sa place.
    .label = Fichiers manquants gardés

setting-system-map-limit =
    .description = Combien de fichiers vus mais hors de la Bibliothèque la carte des images garde ; au-delà, les plus anciennement vus sont oubliés d’abord. Les fichiers de la Bibliothèque ne comptent jamais.
    .label = Fichiers vus gardés

setting-system-map-record =
    .description = Ce que la carte des images enregistre : partout (chaque liste de dossier, et les analyses en arrière-plan des volumes connus), les listes (ce que Helios affiche, plus les analyses des volumes qui ont un rôle) ou l’import (seulement ce qui est importé).
    .label = La carte des images enregistre

setting-system-map-record-choice-everywhere = partout

setting-system-map-record-choice-import = l’import

setting-system-map-record-choice-listings = les listes

setting-system-scheduler-workers =
    .description = Combien de tâches tournent en même temps sur cette machine ; 0 : une par processeur.
    .label = Fils de travail

setting-system-workflow-keep-days =
    .description = Combien de temps un lot terminé (une exécution du niveau flux de travail) reste dans le coffre, échecs compris, avant d’être retiré en entier ; 0 : il est retiré au prochain démarrage du nœud.
    .label = Lots terminés gardés

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

setting-import-show-in-library =
    .description = Quand une importation que vous avez lancée se termine, la bibliothèque montre ses photos, toutes sélectionnées. Jamais pour les importations automatiques ; pendant que vous retouchez une photo, sa notification le propose plutôt.
    .label = Après une importation, la montrer dans la bibliothèque

setting-interface-experimental-workshops =
    .description = Expérimental : le panneau Ateliers et ses commandes, pour trouver, appliquer et régler des macros selon leur but. Peut changer ; ce qu’il enregistre reste une macro. Prend effet au prochain démarrage.
    .label = Ateliers

setting-interface-graph-zoom =
    .description = Si la vue du graphe s’ouvre sur la vue d’ensemble ou sur tout le détail.
    .label = La vue du graphe s’ouvre sur

setting-interface-graph-zoom-choice-detail = le détail

setting-interface-graph-zoom-choice-overview = la vue d’ensemble

setting-interface-panels-queue-when-empty =
    .description = Ce que fait le panneau File d’attente quand le nœud sélectionné n’a pas de file.
    .label = Panneau File d’attente sans file

setting-library-double-click =
    .description = Où un double clic ou Entrée dans une galerie montre la photo : la surface principale (l’Image), la surface utilisée en dernier, celle de la même fenêtre que la galerie, ou demander à chaque fois. Avec deux surfaces ouvertes, celle qu’il atteint porte une marque de cible.
    .label = Le double clic montre la photo dans

setting-library-double-click-choice-primary = la surface principale

setting-library-double-click-choice-last-focused = la dernière utilisée

setting-library-double-click-choice-same-window = la même fenêtre

setting-library-double-click-choice-ask = demander
