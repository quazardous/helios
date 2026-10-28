# Traduction de ../en/messages.ftl, sous les mêmes clés.

batch-cancelled = annulé

batch-cannot-create = impossible de créer { $path } : { $error }

batch-cannot-read = impossible de lire { $path } : { $error }

batch-failed = échec

batch-needs-network = batch demande --network FICHIER, ou un nœud auquel se rattacher

batch-no-source = le réseau n’a pas de source

batch-node-refused = le nœud a refusé

batch-node-unreachable = impossible de joindre le nœud à { $address } : { $error }

batch-several-ends = le réseau se termine par plusieurs nœuds ({ $names }) : précisez --node NOM

batch-several-sources = le réseau a plusieurs sources ({ $names }) : précisez --source NOM

batch-vault-needs-sidecar = --vault conserve les sidecars : il demande --sidecar

engine-failure-compile = compilation

engine-failure-device-lost = périphérique perdu

engine-failure-dispatch = exécution

engine-failure-other = autre

engine-failure-out-of-memory = mémoire épuisée

engine-failure-timeout = délai dépassé

engine-gpu-fell-back = { $module } a été calculé sur le CPU : sa version GPU a échoué ({ $failure } : { $reason }).

engine-gpu-lost = Le GPU ({ $gpu }) a échoué ({ $failure } : { $reason }) et n’a pas pu être redémarré ({ $why }) ; la suite de cette session est calculée sur le CPU.

engine-gpu-lost-recovered = Le GPU ({ $gpu }) a échoué ({ $failure } : { $reason }) ; il a été redémarré et le rendu rejoué.

export-cancelled = annulé

export-channels = impossible d’exporter une image à { $channels } canaux

export-encode-failed = impossible d’encoder l’image : { $error }

export-no-format = aucun format pour { $path }

export-not-a-file = { $path } n’est pas un chemin de fichier

export-not-a-mask = un masque a un canal, pas { $channels }

export-png-bits = le PNG a 8 ou 16 bits, pas { $bits }

export-write-failed = impossible d’écrire { $path } : { $error }

gui-about-crashes = Rapports de plantage : { $path }

gui-about-logs = Fichier journal : { $path }

gui-caption-decisions = Décisions en attente

gui-caption-decisions-count = { $count ->
    [one] { $count } suggestion de piles en attente de réponse
   *[other] { $count } suggestions de piles en attente de réponse
}

gui-caption-jobs = Afficher les tâches

gui-caption-profile = Profil utilisé : { $name }

gui-confirmed-by-policy = { $verb } : confirmé par la règle

gui-copied = copié { $text }

gui-copied-files = { $count ->
    [one] fichier copié
   *[other] { $count } fichiers copiés
}

gui-copied-files-missing = { $count ->
    [one] fichier copié
   *[other] { $count } fichiers copiés
} ; images sans fichier laissées de côté : { $missing }

gui-copied-image = image copiée ({ $width } × { $height })

gui-copy-image-failed = l’image n’a pas été copiée : { $error }

gui-decisions-accept = Accepter

gui-decisions-accept-above = Accepter les { $count } au-dessus de { $percent } %

gui-decisions-answer = Répondre…

gui-decisions-cancel = Annuler

gui-decisions-confidence = Confiance : { $percent } %

gui-decisions-correct = Corriger…

gui-decisions-correct-hint = Modifier la proposition avant de l’enregistrer

gui-decisions-count = { $count ->
    [one] { $count } décision en attente
   *[other] { $count } décisions en attente
}

gui-decisions-mode-assisted = { $node } : un modèle propose, vous tranchez

gui-decisions-mode-full-ai = { $node } : décidé par un modèle, vous pouvez répondre à sa place

gui-decisions-mode-human = { $node } : c’est vous qui répondez

gui-decisions-none = Aucune décision n’attend de réponse

gui-decisions-other-options = { $count ->
    [one] { $count } autre option
   *[other] { $count } autres options
}

gui-decisions-proposal = { $model } propose :

gui-decisions-record = Enregistrer

gui-decisions-threshold-hint = À quel point le modèle doit être sûr pour que sa proposition soit acceptée

gui-decisions-waiting-model = En attente du modèle

gui-decisions-why = Pourquoi : { $why }

gui-edit-does-not-fit = Une partie de la retouche enregistrée ne correspond pas à ce réseau : { $part }

gui-edit-not-saved = La retouche n’a pas été enregistrée : { $error }

gui-gesture-in-progress = { $gesture } est en cours

gui-graph-label = Le réseau de l’espace de travail

gui-graph-no-workspace = Aucun espace de travail n’est ouvert : son réseau s’affiche ici.

gui-graph-state-done = fait

gui-graph-state-failed = échec

gui-graph-state-idle = inactif

gui-graph-state-pending = en attente

gui-graph-state-running = en cours

gui-graph-state-stale = périmé

gui-graph-subworkspace-not-yet = Entrer dans un sous-espace de travail ({ $id }) n’est pas encore possible

gui-graph-wires = Liaisons

gui-graph-wires-curves = Courbes

gui-graph-wires-orthogonal = Orthogonales

gui-graph-wires-straight = Droites

gui-history-batch = { $label } ({ $count } valeurs)

gui-job-batch-done = Lot terminé

gui-job-batch-failed = Échec du lot

gui-job-export-done = Export terminé

gui-job-export-failed = Échec de l’export

gui-keys-from-template = modèle

gui-layout-not-saved = La disposition n’a pas été enregistrée : { $error }

gui-library-not-saved = Non enregistré, la bibliothèque montre ce que contient le coffre : { $error }

gui-mask-display = Afficher le masque

gui-mask-display-grey = le masque seul en gris

gui-mask-display-hint = Afficher { $mask } sur l’image, actuellement { $mode }. Ce bouton, M quand l’image a le focus et la palette alternent une superposition colorée, le masque seul en gris, et désactivé.

gui-mask-display-off = désactivé

gui-mask-display-overlay = une superposition colorée

gui-masks-add = Ajouter…

gui-masks-add-hint = Ajouter une forme ou un membre paramétrique au groupe sélectionné. L’image ayant le focus : C un cercle, E une ellipse, G un dégradé, P un chemin.

gui-masks-beside-refinement = rien ne va à côté d’un affinage : déposez sur un membre ou dans un groupe

gui-masks-empty = Le masque est vide : { $module } s’applique partout. Ajoutez une forme ou un membre paramétrique pour le limiter.

gui-masks-gesture-ctrl = Ctrl : déplacement fin

gui-masks-gesture-reset = double-clic : réinitialiser

gui-masks-gesture-shift-angle = Maj : aligner l’angle par pas de 15°

gui-masks-gesture-shift-axes = Maj : le long des axes et des diagonales

gui-masks-gesture-wheel = molette : ajuster

gui-masks-group-full = le groupe « { $group } » contient déjà { $count } membres

gui-masks-group-locked = le groupe « { $group } » est verrouillé : ses membres restent

gui-masks-hide = Masquer { $name } : son opacité éteinte

gui-masks-into-itself = un groupe ne peut pas aller en lui-même

gui-masks-invert = Inverser

gui-masks-invert-hint = Inverser le membre sélectionné : il couvre alors tout sauf ce qu’il couvrait (pour le masque du module, tout le masque)

gui-masks-lock = Verrouiller { $name } contre les déplacements et les modifications

gui-masks-locked = « { $name } » est verrouillé

gui-masks-menu-add-circle = Cercle

gui-masks-menu-add-ellipse = Ellipse

gui-masks-menu-add-gradient = Dégradé

gui-masks-menu-add-parametric = Paramétrique

gui-masks-menu-add-path = Chemin

gui-masks-menu-delete = Retirer du masque

gui-masks-menu-invert = Inverser

gui-masks-menu-lock = Verrouiller ou déverrouiller

gui-masks-menu-opacity = Opacité éteinte ou à fond

gui-masks-menu-rename = Renommer…

gui-masks-none = Aucun masque : le module sélectionné n’est pas masqué

gui-masks-not-a-group = « { $name } » n’est pas un groupe : seul un groupe accueille des membres

gui-masks-nothing-beside-root = rien ne va à côté du masque du module : déposez dedans

gui-masks-of = Masque de { $module }

gui-masks-one-at-a-time = les membres se déplacent un par un

gui-masks-op-difference = soustraire du premier

gui-masks-op-exclusion = exclure

gui-masks-op-maximum = union

gui-masks-op-minimum = intersection

gui-masks-op-product = multiplier

gui-masks-op-screen = union douce

gui-masks-op-sum = ajouter

gui-masks-opacity = Opacité

gui-masks-opacity-hint = Éteindre l’opacité du membre sélectionné, ou la remettre à fond (l’œil de sa ligne fait de même)

gui-masks-operator-hint = { $effect } : comment les membres se combinent, dans l’ordre des ports (l’opérateur { $operator })

gui-masks-refine-blur = flou

gui-masks-refine-feather = fondu

gui-masks-refine-levels = niveaux

gui-masks-refinement-stays = « { $name } » reste avec ce qu’il affine

gui-masks-root-stays = le masque du module reste la racine

gui-masks-row-hidden = masqué

gui-masks-row-inverted = inversé

gui-masks-row-not-inverted = non inversé

gui-masks-row-opacity = opacité { $percent } %

gui-masks-show = Afficher { $name } : son opacité à fond

gui-masks-take-out = Retirer

gui-masks-take-out-group = Retirer le groupe { $name } du masque ?

gui-masks-take-out-group-detail = Ses membres partent avec lui, et le retrait n’est pas encore une étape d’annulation.

gui-masks-take-out-hint = Retirer le membre sélectionné du masque (Suppr, l’arbre ayant le focus). Pas encore une étape d’annulation.

gui-masks-take-out-verb = Retirer

gui-masks-tree-hint = F2 renomme la ligne sélectionnée. Glissez une ligne pour la réordonner ou la mettre dans un groupe. Clic droit sur une ligne pour son menu : il ne supprime jamais rien.

gui-masks-unlock = Déverrouiller { $name }

gui-modules-bypass = Contourner

gui-modules-bypass-hint = Contourner { $node } : l’image est rendue comme s’il était absent, ses paramètres gardés ({ $key })

gui-modules-none = Aucun module : ouvrez une image pour voir les modules de son réseau

gui-modules-restore-hint = Rétablir { $node } : l’image le rend de nouveau ({ $key })

gui-new-workspace-created = espace de travail { $path } créé depuis { $template }

gui-new-workspace-directory = dossier

gui-new-workspace-form-title = Nouvel espace de travail : { $template }

gui-new-workspace-template = modèle

gui-new-workspace-title = Nouvel espace de travail depuis un modèle

gui-new-workspace-unknown = aucun modèle { $id }

gui-new-workspace-verb = Créer

gui-no-dialogs = aucune boîte de dialogue pour montrer ceci

gui-open-workspace-directory = dossier

gui-open-workspace-title = Ouvrir un espace de travail

gui-open-workspace-verb = Ouvrir

gui-opening = ouverture de { $path }

gui-params-hide-values = Masquer les valeurs

gui-params-no-members = aucun membre pour l’instant

gui-params-points = { $count } points

gui-params-port-hint = Port { $port } du groupe ; le panneau Masques le modifie sur la ligne de { $member }

gui-params-port-member = { $member } : { $value }

gui-params-show-values = Afficher les valeurs

gui-params-values = { $count } valeurs

gui-paste-import-copy = Les fichiers sont copiés dans le dossier de la bibliothèque.

gui-paste-import-image = L’image est enregistrée en fichier PNG et copiée dans le dossier de la bibliothèque.

gui-paste-import-reference = Les fichiers restent où ils sont.

gui-paste-import-source = Les fichiers sont placés comme le dit la source d’import de l’espace de travail.

gui-paste-import-title = { $count ->
    [one] Importer le fichier collé ?
   *[other] Importer les { $count } fichiers collés ?
}

gui-paste-import-verb = Importer

gui-pointer-choice-close = Fermer l’avis du profil de pointeur

gui-pointer-choice-darktable = Utiliser ceux de darktable

gui-pointer-choice-keep = Garder le style Photoshop

gui-pointer-choice-text = Reprendre les habitudes de souris et de molette de darktable : la molette change le curseur sous le pointeur et zoome l’image. Ce choix se change plus tard dans les Réglages (profil de pointeur).

gui-pointer-choice-title = Vous venez de darktable ?

gui-policy-ask = les confirmations sont de nouveau demandées

gui-policy-yes = les confirmations sont acceptées par la règle

gui-profile-applied = profil { $name } appliqué

gui-profile-exported = profil exporté vers { $path }

gui-profile-import-changes = profil { $name } : { $count } changements ; profile.import.apply pour les appliquer

gui-profile-keys-changed = profil { $name } : { $count } raccourcis changés

gui-profile-keys-conflict = profil { $name } : raccourcis en conflit : { $keys }

gui-profile-keys-error = profil { $name } : raccourcis : { $error }

gui-profile-keys-skipped = profil { $name } : raccourcis ignorés : { $keys }

gui-profile-layout-error = profil { $name } : disposition : { $error }

gui-profile-variants = profil { $name } : { $count } valeurs venant de variantes : { $variants }

gui-quick-open-image = image

gui-quick-open-module = module { $module }

gui-quick-open-source = source

gui-quick-open-title = Aller à un module, une image ou un espace de travail

gui-quick-open-workspace = espace de travail { $path }

gui-recording-saved = enregistrement sauvé dans { $path }

gui-recording-stopped = enregistrement arrêté : { $error }

gui-scene-saved = viewport.scene sauvé dans { $path }

gui-scopes-blue = Bleu

gui-scopes-green = Vert

gui-scopes-red = Rouge

gui-snapshot-failed = capture échouée : { $error }

gui-snapshot-saved = { $target } capturé dans { $path }

gui-status-culled = { $picked ->
    [one] { $picked } retenue
   *[other] { $picked } retenues
}, { $rejected ->
    [one] { $rejected } rejetée
   *[other] { $rejected } rejetées
}

gui-status-decisions = { $count ->
    [one] { $count } décision attend une réponse
   *[other] { $count } décisions attendent une réponse
}

gui-status-decisions-show = Afficher les décisions

gui-status-history = Historique

gui-status-history-detail = { $count ->
    [one] { $count } étape à annuler
   *[other] { $count } étapes à annuler
}

gui-status-history-redo = Rétablir

gui-status-history-undo = Annuler

gui-status-images = { $count ->
    [one] { $count } image
   *[other] { $count } images
}

gui-status-zoom = Zoom

gui-status-zoom-fill = Remplir

gui-status-zoom-fit = Ajuster

gui-template-keys-error = modèle { $name } : raccourcis : { $error }

gui-theme-exported = thème exporté vers { $path }

gui-theme-problem = thème { $name } : { $problem }

gui-theme-unknown = thème { $name } : pas de thème « { $name } » : { $default } est utilisé à la place

gui-vault-unreadable = Impossible de lire le coffre : { $error }

gui-window-frame-error = cadre de la fenêtre : { $error }

gui-library-file = { $size } · modifié le { $day }

gui-param-default = { $path } · par défaut { $value }

gui-param-held = { $label } : maintenu à { $value }

gui-param-hint = Glisser pour changer ({ $fine } plus fin, { $coarse } plus grand), flèches pour avancer d’un pas, { $wheel } ; double clic pour réinitialiser ; Entrée pour saisir une valeur ou une expression (x est la valeur)

gui-param-hint-wheel-focused = la molette une fois le focus pris

gui-param-hint-wheel-focused-or = la molette une fois le focus pris ou avec { $keys }

gui-param-hint-wheel-over = la molette au survol

gui-param-past-range = { $label } : au-delà de sa plage habituelle

gui-param-picker = Prélever { $label } dans l’image

gui-size-bytes = { $size } o

gui-size-gib = { $size } Gio

gui-size-kib = { $size } Kio

gui-size-mib = { $size } Mio

gui-workspace-description = description

gui-workspace-edit-title = Modifier l’espace de travail

gui-workspace-edit-verb = Enregistrer

gui-workspace-icon = icône

gui-workspace-import-next = Examiner

gui-workspace-import-none = aucune partie choisie

gui-workspace-import-parts = Parties à ajouter depuis { $source }

gui-workspace-import-review = Ajouter depuis { $source } : { $changes } changements, { $conflicts } à trancher

gui-workspace-import-title = Ajouter depuis un espace de travail ou un modèle

gui-workspace-import-verb = Ajouter

gui-workspace-library-advance = le tri avance après chaque marque

gui-workspace-library-duplicates = doublons exacts

gui-workspace-library-placement = emplacement

gui-workspace-library-rename = renommer les copies en

gui-workspace-library-source = source d’import

gui-workspace-library-stacks = proposer des piles après un import

gui-workspace-library-title = La photothèque de l’espace de travail

gui-workspace-name = nom

gui-workspace-not-saved = L’espace de travail n’a pas été enregistré : { $error }

gui-workspace-reload-failed = Les fichiers de l’espace de travail ont changé hors de Helios et n’ont pas pu être lus : { $error }

gui-workspace-reloaded = Les fichiers de l’espace de travail ont changé hors de Helios : { $parts } rechargé(s)

gui-workspace-reloaded-both = Les fichiers de l’espace de travail ont changé hors de Helios pendant que vous le modifiiez aussi : { $both } repris des fichiers (Édition > Annuler rend votre version)

gui-workspace-template-ask = demander à la création : { $name }

gui-workspace-template-description-fr = description en français

gui-workspace-template-id = identifiant du modèle

gui-workspace-template-keys = avec ses raccourcis

gui-workspace-template-library = avec ses réglages de photothèque

gui-workspace-template-name-fr = nom en français

gui-workspace-template-overwrite = remplacer un modèle de cet identifiant

gui-workspace-template-saved = modèle { $name } enregistré dans { $path }

gui-workspace-template-stripped = Jamais gardé : { $what }

gui-workspace-template-title = Enregistrer comme modèle

gui-workspace-template-toolbox = avec ses outils

gui-workspace-template-version = version

node-batch-only = le point d’accès des lots ne répond qu’à load-source, export, cancel, job.cancel, batch_start, batch_end et workflow.submit, workflow.runs, workflow.items, workflow.forget

node-edit-not-saved = fait, mais la retouche n’a pas été enregistrée : { $error }

node-job-unknown = la tâche { $job } est inconnue ou déjà terminée

node-load-refused = impossible de charger l’image : { $error }

node-no-extent = `{ $name }` n’a pas d’étendue : donnez une région

node-no-node = aucun nœud nommé `{ $name }`

node-param-refused = impossible de régler { $node }.{ $param } : { $error }

node-path-not-absolute = { $path } : donnez un chemin absolu, le nœud ne sait pas où vous êtes

node-source-refused = `{ $name }` ne peut pas prendre l’image : { $error }

resolve-failed = { $node } : { $reason }

resolve-not-asked = { $node } n’a pas été demandé : { $reason }

resolve-refused = { $node } : le modèle n’a pas répondu ({ $reason })

workflow-newer-format = l’exécution `{ $run }` a été conservée par un Helios plus récent : elle ne peut pas être traitée ici

workflow-no-items = une exécution demande au moins un élément

workflow-no-node = l’exécution `{ $run }` ne passe pas par une file à `{ $node }`

workflow-no-run = aucune exécution `{ $run }`

workflow-other-network = l’exécution `{ $run }` a été soumise à un autre réseau ({ $why }) : elle ne peut pas être traitée ici

workflow-run-at-work = l’exécution `{ $run }` est encore en cours : laissez-la finir, ou annulez-la, avant de l’oublier

workflow-run-elsewhere = l’exécution `{ $run }` est traitée par un autre nœud ({ $owner })

xmp-blend-details = le seuil de détails du masque

xmp-blend-feathering = l’adoucissement du masque

xmp-blend-mode = le mode de fusion { $mode }

xmp-blend-parametric = un masque paramétrique

xmp-blend-raster = un masque raster

xmp-blend-space = une fusion dans l’espace de couleur { $space }

xmp-mask-empty-group = le groupe de masques { $id } est vide

xmp-mask-lost = le masque perdrait { $what }

xmp-mask-missing = un groupe de masques renvoie à la forme { $id }, absente de l’historique

xmp-mask-size = des points de forme de { $bytes } octets, { $expected } attendus

xmp-mask-too-deep = des groupes de masques imbriqués trop profond

xmp-mask-too-many-points = une forme de { $count } points, { $max } au plus

xmp-mask-unknown-type = un type de forme inconnu ({ $kind })

xmp-mask-unsupported = une forme { $form }, que Helios ne dessine pas encore

xmp-mask-version = version de masques { $version } non prise en charge (lit 4 à 7)

xmp-not-applied = { $count ->
    [one] { $count } élément d’historique sur { $items } non appliqué, conservé pour plus tard : { $groups }
   *[other] { $count } éléments d’historique sur { $items } non appliqués, conservés pour plus tard : { $groups }
}

xmp-not-applied-group = { $labels } ({ $reason })

xmp-reason-blend = fusion non prise en charge : { $what }

xmp-reason-blending = fusion non prise en charge

xmp-reason-extra-instance = une seconde instance d’un module à instance unique

xmp-reason-mask = masque dessiné non pris en charge : { $why }

xmp-reason-mask-without-image = un masque dessiné, lu sans son image

xmp-reason-unknown-blend-version = paramètres de fusion v{ $version } non pris en charge

xmp-reason-unknown-module = pas de module Helios

xmp-reason-unreadable = illisible : { $error }

xmp-reason-unsupported-version = version non prise en charge (lit { $known })

gui-library-auto-advance = Avance automatique

gui-library-compare-hint = Sélectionnez de deux à quatre images pour les comparer

gui-library-count = { $shown } sur { $total }

gui-library-empty = Aucune image : importez un dossier dans le coffre

gui-library-images = Images

gui-library-keep-best = Garder la meilleure : la choisir, rejeter les autres

gui-library-no-cursor = Aucune image sous le curseur

gui-library-none = Aucune image

gui-library-search = Rechercher des images

gui-library-sync = Synchroniser le zoom et le déplacement

gui-library-tags = Mots-clés

gui-stacks-accept-all = Tout accepter

gui-stacks-accept-checked = Accepter les piles cochées

gui-stacks-bracket = Bracketing de { $count }

gui-stacks-burst = Rafale de { $count }

gui-stacks-decline = Refuser

gui-stacks-left-out = { $count } images laissées de côté

gui-stacks-none = Aucune suggestion de piles en attente

gui-stacks-proposal = { $groups } piles proposées par { $resolver }

gui-stacks-show = Montrer

gui-stacks-suggest = Suggérer des piles

gui-stacks-suggest-hint = Sur les images sélectionnées (deux ou plus), sinon sur toutes les images

gui-stacks-top = Proposée au-dessus : la plus nette

gui-duplicates-asked = La garder comme copie de plus, ou l’ignorer ?

gui-duplicates-clear = Effacer les réponses données

gui-duplicates-declined = Ignoré

gui-duplicates-keep = Garder

gui-duplicates-keep-all = Tout garder

gui-duplicates-kept = Gardé comme copie de plus

gui-duplicates-located = Gardé comme copie de plus, selon la règle de l’import

gui-duplicates-none = Aucun doublon trouvé à l’import

gui-duplicates-of-library = Déjà dans la bibliothèque : { $at }

gui-duplicates-of-source = Le même fichier que { $first }, plus tôt dans cet import

gui-duplicates-skip = Ignorer

gui-duplicates-skip-all = Tout ignorer

gui-duplicates-skipped = Ignoré, selon la règle de l’import

gui-duplicates-waiting = { $count } doublons attendent une réponse

gui-duplicates-waiting-note = { $count } doublons attendent une réponse dans le panneau Doublons

gui-import-form-title = Importer une carte ou un dossier

gui-import-path = carte ou dossier

gui-import-verb = Importer

gui-library-import = Importer…

gui-image-not-reopened = L’image laissée ouverte n’a pas pu être rouverte : { $error }

gui-image-opened = { $name } est ouverte

gui-image-opening = Ouverture de { $name }…

gui-library-read = { $count } images dans la bibliothèque

gui-library-reading = Lecture de la bibliothèque…

gui-queue-cancel = Annuler

gui-queue-column-attempts = Tentatives

gui-queue-column-item = Élément

gui-queue-column-since = Depuis

gui-queue-column-state = État

gui-queue-column-where = Où

gui-queue-empty = Rien en file d’attente à { $node }

gui-queue-here = Ce poste

gui-queue-hours = { $n } h

gui-queue-minutes = { $n } min

gui-queue-no-selection = Sélectionnez un nœud pour voir sa file d’attente

gui-queue-oldest-wait = Attente la plus longue : { $time }

gui-queue-on-node = Nœud { $name }

gui-queue-seconds = { $n } s

gui-queue-state-cancelled = Annulé

gui-queue-state-cancelling = Annulation

gui-queue-state-done = Terminé

gui-queue-state-failed = Échoué

gui-queue-state-paused = En pause

gui-queue-state-running = En cours

gui-queue-state-waiting = En attente

gui-queue-summary = { $waiting } en attente, { $running } en cours, { $done } terminés, { $failed } échoués

gui-queue-title = File d’attente de { $node }
