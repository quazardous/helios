# Traduction de ../en/messages.ftl, sous les mêmes clés.

batch-asking = non exporté : l’exécution { $run } attend qu’une personne réponde à une décision

batch-cancelled = annulé

batch-cannot-create = impossible de créer { $path } : { $error }

batch-cannot-read = impossible de lire { $path } : { $error }

batch-held = non exporté : une étape avant l’export a échoué ou a été annulée

batch-needs-network = batch demande --network FICHIER, ou un nœud auquel se rattacher

batch-no-source = le réseau n’a pas de source

batch-node-refused = le nœud a refusé

batch-node-unreachable = impossible de joindre le nœud à { $address } : { $error }

batch-run-gone = l’exécution { $run } a quitté le nœud

batch-run-paused = non exporté : l’exécution { $run } est en pause

batch-several-ends = le réseau se termine par plusieurs nœuds ({ $names }) : précisez --node NOM

batch-several-sources = le réseau a plusieurs sources ({ $names }) : précisez --source NOM

batch-stopped = arrêté avant d’être écrit : relancer le même lot le termine

batch-vault-attached = --vault avec un nœud déjà lancé ne conserve que les sidecars : il demande --sidecar (le nœud garde ses exécutions dans son propre coffre)

calibrate-auto-no-light = l’image { $image } n’a pas de lumière de prise de vue pour choisir un profil (boîtier { $camera })

calibrate-auto-no-profile = aucun profil de calibration pour le boîtier { $camera } (image { $image })

calibrate-auto-too-far = aucun profil du boîtier { $camera } ne convient à la lumière de l’image { $image } : le plus proche, { $profile }, est à { $distance } en uv (au plus 0,02)

calibrate-bad-image = la prise de vue de la mire n’a aucun pixel ou moins de trois canaux

calibrate-bad-params = calibrate.chart : { $error }

calibrate-bad-profile = le profil de calibration « { $id } » est illisible : { $error }

calibrate-default-cameras = les profils d’un défaut sont tous du boîtier { $camera } ; « { $id } » est de { $other }

calibrate-default-no-camera = effacer le défaut d’un boîtier demande `camera`

calibrate-export-failed = impossible d’écrire { $path } : { $error }

calibrate-import-camera-key = la clé de boîtier « { $key } » ne correspond pas à la marque et au modèle « { $make } { $model } »

calibrate-import-format = le format du profil est { $found } ; ce Helios lit les formats { $expected }

calibrate-import-id = l’identifiant du profil n’est pas une empreinte hexadécimale de 64 chiffres

calibrate-import-illuminant = la chromaticité de la lumière { $x }, { $y } est hors du diagramme

calibrate-import-matrix = la matrice du profil n’est pas une matrice finie et inversible

calibrate-import-not-profile = le fichier n’est pas un profil de calibration Helios

calibrate-import-nothing = calibrate.profiles.import demande `path` ou `file`

calibrate-import-numbers = la correction ou la matrice du boîtier du profil contient des nombres non finis

calibrate-import-refused = le profil est refusé : { $reasons }

calibrate-import-table = la table résiduelle du profil n’est pas une table que ce Helios peut appliquer

calibrate-import-unreadable = le profil est illisible : { $error }

calibrate-import-unreadable-file = impossible de lire { $path } : { $error }

calibrate-image-not-at-hand = aucun fichier de l’image { $image } n’est disponible

calibrate-load-failed = impossible de lire la prise de vue de la mire : { $error }

calibrate-needs-vault = le nœud n’a pas de coffre : enregistrer et lire des profils en demande un (le lancer avec --vault)

calibrate-no-profile = aucun profil de calibration « { $id } »

calibrate-no-shot = calibrate.chart demande `path` ou `image`, la prise de vue de la mire

calibrate-outside-image = les cases de la mire tombent hors de l’image

calibrate-no-camera-matrix = le boîtier n’a pas de matrice de couleur utilisable pour mesurer le raw

calibrate-raw-graph = le raw n’a pu être développé pour la mesure : { $error }

calibrate-save-needed = une exécution qui écrit demande `save` : c’est le profil qu’elle écrit

calibrate-unknown-chart = aucune mire « { $chart } »

calibrate-unknown-command = aucune commande « { $id } »

calibrate-unknown-image = « { $image } » n’est ni un identifiant d’image ni un fichier connu du coffre

calibrate-unknown-patch = la mire n’a pas de case « { $patch } »

calibrate-no-neutral = la mire n’a pu être résolue à ces coins : aucune case neutre n’est retenue

calibrate-other-camera = le profil est pour le boîtier { $profile }, l’image { $image } vient de { $camera }

calibrate-photo-load-failed = impossible de lire la photo : { $error }

calibrate-too-few-patches = la mire n’a pu être résolue à ces coins : trop peu de cases sont retenues

calibrate-unsolved = la mire n’a pu être mesurée et résolue à ces coins

calibrate-vault-error = le coffre : { $error }

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

export-folder-failed = impossible de créer le dossier { $folder } : { $error }

export-encode-failed = impossible d’encoder l’image : { $error }

export-no-format = aucun format pour { $path }

export-not-a-file = { $path } n’est pas un chemin de fichier

export-not-a-mask = un masque a un canal, pas { $channels }

export-png-bits = le PNG a 8 ou 16 bits, pas { $bits }

export-write-failed = impossible d’écrire { $path } : { $error }

gui-about-description = Un framework de traitement d’image client/serveur, pensé d’abord pour l’IA.

gui-account-add = Ajouter un compte…

gui-account-another = Utiliser un autre compte…

gui-account-choose = Choisir des photos…

gui-account-disconnect = Déconnecter

gui-account-disconnect-explain = Helios demande au service de révoquer son accès, oublie les clés et jetons de ce compte sur cet ordinateur et retire le compte. Ce qui a été importé reste.

gui-account-disconnect-forget = Déconnecter et oublier ses sources

gui-account-disconnect-forget-explain = Helios demande au service de révoquer son accès, oublie les clés et jetons de ce compte sur cet ordinateur, retire le compte et oublie ces sources : { $sources }. Ce qui a été importé reste.

gui-account-disconnect-keep = Déconnecter, garder ses sources

gui-account-disconnect-keep-explain = Helios demande au service de révoquer son accès et oublie les clés et jetons de ce compte sur cet ordinateur. Ces sources restent, non connectées, jusqu'à ce que vous le reconnectiez : { $sources }.

gui-account-disconnect-title = Déconnecter { $account } ?

gui-account-identity-missing = Reconnectez pour voir le compte

gui-account-manage = Gérer les comptes…

gui-account-menu = Compte

gui-account-needs-reconnecting = à reconnecter

gui-account-never-used = Jamais

gui-account-status-connecting = Connexion en cours

gui-account-status-not-connected = Non connecté

gui-account-status-ok = OK

gui-account-status-reconnect = À reconnecter

gui-account-status-refused = Accès refusé

gui-backup-card-safe = { $card } peut être formatée : chaque photo a ses sauvegardes

gui-caches-clear = Vider

gui-caches-clear-all = Vider les caches

gui-caches-clear-all-ask = Les vignettes sont relues depuis les fichiers et les images recalculées à l'affichage : plus lent pendant un moment. Les photos, les retouches et le catalogue ne sont pas touchés.

gui-caches-clear-all-title = Vider tous les caches ?

gui-caches-none = Aucun cache ne contient encore rien.

gui-caches-size = { $size }, { $entries } entrées

gui-caches-size-of = { $size } sur { $budget }, { $entries } entrées
gui-caches-size-shown = { $size } : { $shown } affichées, { $prefetched } lues d’avance

gui-caches-tier-disk = sur disque

gui-caches-tier-ram = en mémoire

gui-caches-tier-vram = en mémoire vidéo

gui-caches-total = Les caches occupent { $size }

gui-caches-use = Utilisé

gui-caption-decisions = Décisions en attente

gui-caption-decisions-count = { $count ->
    [one] { $count } suggestion de piles en attente de réponse
   *[other] { $count } suggestions de piles en attente de réponse
}

gui-caption-jobs = Afficher les tâches

gui-caption-profile = Profil utilisé : { $name }

gui-choose-about = { $service } s'ouvre dans votre navigateur. Choisissez vos photos, appuyez sur OK, puis revenez ici.

gui-choose-chosen = Les photos choisies s'affichent au milieu : cochez celles à importer, puis appuyez sur Importer.

gui-choose-closed = La page { $service } s'est fermée. Ouvrez-la à nouveau pour choisir vos photos.

gui-choose-connect = Connecter { $service }

gui-choose-connecting = Connectez-vous dans votre navigateur, puis revenez ici.

gui-choose-copy-link = Copier le lien



gui-choose-identity = Reconnectez-vous une fois pour voir de quel compte il s'agit.


gui-choose-more-hint = Rouvre la page : les photos choisies là-bas s'ajoutent à celles-ci

gui-choose-more-short = En choisir d'autres…



gui-choose-not-connected = Ce compte n'est pas connecté.

gui-choose-not-open = La page ne s'est pas ouverte ?

gui-choose-nothing = Aucune photo n'a été choisie.

gui-choose-open-again = L'ouvrir à nouveau

gui-choose-opening = Ouverture de { $service }…

gui-choose-photos = Choisir des photos dans { $service }…

gui-choose-reconnect = Reconnecter { $account }

gui-choose-refused = ▲ { $service } ne laisse plus Helios lire ce compte.

gui-choose-refused-why = Cela arrive quand l'accès a été retiré dans les réglages du compte chez { $service }, ou quand l'accès donné à Helios a expiré.

gui-choose-signed-in = { $service } montre les photos du compte auquel votre navigateur est connecté : { $account }.

gui-choose-try-again = Réessayer

gui-choose-unreachable = Impossible de joindre { $service } : { $why }

gui-choose-waiting = Choisissez vos photos dans le navigateur, appuyez sur OK, puis revenez ici.

gui-confirmed-by-policy = { $verb } : confirmé par la règle

gui-connect-copy-link = Copier le lien

gui-connect-link-hint = L'adresse de la page, à coller dans votre navigateur s'il ne l'a pas ouverte.

gui-connect-open-again = Ouvrir à nouveau

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

gui-editor-card = Carte ou appareil photo

gui-editor-params = Paramètres

gui-editor-params-refused = Un paramètre ne correspond pas à ce que le connecteur accepte : { $why }

gui-editor-save = Enregistrer la source

gui-editor-title = Modifier la source

gui-gesture-in-progress = { $gesture } est en cours

gui-graph-add-macro = Macros › { $name }

gui-graph-label = Le réseau de l’espace de travail

gui-graph-no-workspace = Aucun espace de travail n’est ouvert : son réseau s’affiche ici.

gui-graph-state-cached = en cache

gui-graph-state-done = fait

gui-graph-state-failed = échec

gui-graph-state-idle = inactif

gui-graph-state-pending = en attente

gui-graph-state-running = en cours

gui-graph-state-stale = périmé

gui-graph-wires = Liaisons

gui-graph-wires-curves = Courbes

gui-calibration-default-applied = Profil de calibration « { $name } » appliqué : le défaut de { $camera }

gui-calibration-default-none = Aucun profil de calibration appliqué : { $reason }

gui-calibration-header-about = { $camera }, { $light }, qualité { $quality }

gui-calibration-header-choose = Profil de calibration

gui-calibration-header-empty = Aucun profil de calibration pour ce boîtier pour l’instant

gui-calibration-header-gone = Profil { $id } (absent de la bibliothèque)

gui-calibration-header-none = Aucun profil

gui-calibrate-after = Delta E après : moyenne { $mean }, max { $max }

gui-calibrate-as-default = Utiliser comme défaut pour ce boîtier

gui-calibrate-before = Delta E avant : moyenne { $mean }, max { $max }

gui-calibrate-clipped = Écrêtés, laissés de côté : { $patches }

gui-calibrate-corner = Coin de la mire { $n }

gui-calibrate-corner-tip = Glisser sur le centre de la pastille du coin

gui-calibrate-corners-undo = Remettre le coin

gui-calibrate-default-set = Défaut du boîtier défini

gui-calibrate-deleted = Profil supprimé

gui-calibrate-export-folder = Exporter dans le dossier

gui-calibrate-exported = Exporté dans { $path }

gui-calibrate-import = Importer le profil

gui-calibrate-import-file = Fichier de profil à importer

gui-calibrate-imported = Profil importé

gui-calibrate-is-default = Le défaut du boîtier

gui-calibrate-light = Lumière : { $kelvin } K

gui-calibrate-name = Nom du profil

gui-calibrate-no-shot = Ouvrir une photo d’une mire ColorChecker pour faire un profil

gui-calibrate-open-shot = Ouvrir

gui-calibrate-orientation = Mire vue tournée de { $turns } quarts de tour

gui-calibrate-patches = Pastilles

gui-calibrate-profiles = Profils

gui-calibrate-profiles-none = Aucun profil de calibration pour l’instant

gui-calibrate-quality = Qualité : { $quality }

gui-calibrate-report = Rapport

gui-calibrate-report-none = Placer les quatre coins sur les centres des pastilles des coins

gui-calibrate-residual-after = Delta E avec la table : moyenne { $mean }, max { $max }

gui-calibrate-residual-dropped = Table résiduelle : écartée

gui-calibrate-residual-held-out = Delta E hors ajustement : { $without } sans la table, { $with } avec

gui-calibrate-residual-kept = Table résiduelle : gardée

gui-calibrate-save = Enregistrer le profil

gui-calibrate-save-heading = Faire un profil

gui-calibrate-saved = Profil « { $name } » enregistré

gui-calibrate-shot = Photo de la mire

gui-calibrate-title = Calibrer les couleurs du boîtier

gui-calibrate-worst = Pastilles les plus éloignées : { $patches }

gui-calibration-header-make = Faire un profil…

gui-graph-wires-orthogonal = Orthogonales

gui-graph-wires-straight = Droites

gui-history-batch = { $label } ({ $count } valeurs)
gui-history-calibration-apply = Appliquer le profil de calibration « { $name } »
gui-history-calibration-default = Définir le défaut de calibration de { $name }
gui-history-calibration-delete = Supprimer le profil de calibration « { $name } »
gui-history-calibration-off = Désactiver le profil de calibration
gui-history-calibration-save = Enregistrer le profil de calibration « { $name } »
gui-history-calibration-import = Importer le profil de calibration « { $name } »
gui-history-import = Importer { $count } photos
gui-history-workshop-run = Atelier « { $name } » sur { $count } photos
gui-history-album-new = Nouvel album « { $name } »
gui-history-album-rename = Renommer l’album « { $name } »
gui-history-album-delete = Supprimer l’album « { $name } »
gui-history-album-add = Ajouter { $count } photos à « { $name } »
gui-history-album-remove = Retirer { $count } photos de « { $name } »
gui-history-album-new-smart = Nouvel album dynamique « { $name } »
gui-history-set-new = Nouvel ensemble d’albums « { $name } »
gui-history-album-move = Déplacer « { $name } » dans un ensemble
gui-history-album-move-out = Sortir « { $name } » de son ensemble
gui-history-library-remove = Retirer { $count } photos de la bibliothèque
gui-history-quick-add = Ajouter { $count } photos à la collection rapide
gui-history-quick-remove = Retirer { $count } photos de la collection rapide
gui-history-quick-clear = Vider la collection rapide
gui-history-quick-save = Enregistrer la collection rapide sous « { $name } »
gui-history-album-rules = Modifier les règles de « { $name } »

gui-history-camera-look-default = Modifier le rendu boîtier par défaut de { $name }

gui-history-camera-look-fit = Ajuster le rendu boîtier au JPEG de l’appareil

gui-history-preview-applied = appliqué

gui-history-preview-bypassed = contourné

gui-history-preview-param = { $label } ({ $node })

gui-history-snapshot-restored = « { $name } » restauré

gui-history-undo = Annuler

gui-import-backup-copies = { $copies } copies : { $disks }

gui-import-backup-days = { $disk } (pas vu depuis { $days } jours)

gui-import-backup-never = { $disk } (jamais vu)

gui-import-backup-no-disk = aucun disque pour elles pour l’instant

gui-import-backup-none = Aucune sauvegarde configurée :

gui-import-backup-one = 1 copie : { $disks }

gui-import-backup-one-day = { $disk } (pas vu depuis 1 jour)

gui-import-backup-plugged = { $disk } (branché)

gui-import-backup-storage = Stockage…

gui-import-backup-today = { $disk } (vu aujourd’hui)

gui-import-backup-unusable = { $disk } (branché, ne peut pas recevoir de copies)

gui-import-browse = Parcourir

gui-import-browse-collections = Albums

gui-import-browse-every-folder = Tous les dossiers

gui-import-browse-folder = Toutes les photos

gui-import-browse-not-listed = Helios ne liste pas encore ici les photos de cette source.

gui-import-browse-picker = Choisir dans la page du service

gui-import-browse-timeline = Par date

gui-import-browse-tree = Arborescence

gui-import-window-edit-source = Modifier la source…

gui-import-window-forget-source = Oublier cette source

gui-import-window-forget-source-hint = La retire de cette colonne, avec son nom et ses choix ; les photos et la bibliothèque ne sont pas touchées

gui-import-auto = Importer automatiquement

gui-import-auto-off = Non

gui-import-auto-plugged = À l’insertion

gui-import-auto-new-files = Quand de nouveaux fichiers apparaissent

gui-import-auto-started = Import automatique depuis { $source } : { $files } fichiers

gui-import-auto-failed = Import automatique depuis { $source } : { $error }

gui-import-window-services = Services

gui-job-backup-done = Sauvegarde terminée

gui-job-backup-failed = Échec de la sauvegarde

gui-job-batch-done = Lot terminé

gui-job-batch-failed = Échec du lot

gui-job-export-done = Export terminé

gui-job-export-failed = Échec de l’export

gui-job-health-done = Bilan de santé terminé

gui-job-health-failed = Échec du bilan de santé

gui-job-import-done = Import terminé

gui-job-import-failed = Échec de l’import

gui-job-other-done = Tâche terminée

gui-job-other-failed = Échec de la tâche

gui-keys-from-template = modèle

gui-layout-not-saved = La disposition n’a pas été enregistrée : { $error }

gui-library-badge-backed-up = sauvegardée

gui-library-badge-backup-short = sauvegarde incomplète

gui-library-badge-backup-unverified = non vérifiée

gui-library-badge-no-backup = sans sauvegarde

gui-library-not-removed = Les photos n’ont pas été retirées de la bibliothèque : { $error }
gui-library-not-saved = Non enregistré, la bibliothèque montre ce qui a été enregistré en dernier : { $error }

gui-look-fit-selection-done = Rendu boîtier de { $camera } ajusté sur { $fitted } photos sur { $photos } : Delta E { $held } (par défaut { $baseline })

gui-look-fit-selection-job = Ajuster un rendu boîtier sur { $count } photos

gui-look-fit-selection-none = Aucun rendu boîtier ajusté : { $reasons }

gui-look-header-chart = Moitié couleur désactivée : le profil de calibration décide des couleurs

gui-look-header-compare = Comparer au JPEG de l’appareil

gui-look-header-compare-about = Affiche le JPEG intégré de l’appareil à gauche de la séparation et le rendu à sa droite ; faites glisser la séparation pour la déplacer

gui-look-header-compare-no-jpeg = Aucun JPEG de l’appareil à comparer : le raw n’en contient pas

gui-look-header-compare-not-raw = Aucun JPEG de l’appareil à comparer : la photo n’est pas un raw

gui-look-header-default-clear = Effacer le défaut

gui-look-header-default-in-use = Le rendu par défaut de { $camera } : ses raws sont importés avec lui

gui-look-header-default-other = { $camera } a un autre rendu par défaut : ses raws sont importés avec celui-là

gui-look-header-default-save = Enregistrer comme défaut du boîtier

gui-look-header-default-save-about = Les raws de { $camera } importés désormais partent de ce rendu boîtier ; les photos déjà dans la bibliothèque gardent le leur

gui-look-header-fit = Ajuster au JPEG de l’appareil

gui-look-header-fit-about = Ajuste une matrice, une table teinte et saturation et des courbes de tons au JPEG intégré de l’appareil, vérifiés sur des pixels qui n’ont pas servi à l’ajustement

gui-look-header-fitted = Ajusté au JPEG de l’appareil : Delta E { $held } (par défaut { $baseline })

gui-look-header-none = Pas encore ajusté : le rendu part du JPEG de l’appareil lui-même

gui-look-header-presets = Préréglages

gui-look-header-refit = Réajuster

gui-look-header-refused = Non ajusté : { $reason }

gui-look-header-switch = Rendu boîtier

gui-look-preset-camera = Rendu boîtier

gui-look-preset-camera-about = Le rendu par défaut du boîtier, ou un rendu ajusté au JPEG de l’appareil quand le boîtier n’en a pas

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

gui-modules-none-in-scope = Aucun module du sous-espace de travail { $name } dans le réseau de l’image

gui-modules-restore-hint = Rétablir { $node } : l’image le rend de nouveau ({ $key })

gui-new-workspace-created = espace de travail { $path } créé depuis { $template }

gui-new-workspace-directory = dossier

gui-new-workspace-form-title = Nouvel espace de travail : { $template }

gui-new-workspace-template = modèle

gui-new-workspace-title = Nouvel espace de travail depuis un modèle

gui-new-workspace-unknown = aucun modèle { $id }

gui-new-workspace-verb = Créer

gui-no-dialogs = aucune boîte de dialogue pour montrer ceci

gui-notice-details = Détails

gui-notice-failed = Un problème est survenu

gui-notice-no-details = Aucun échec dont montrer les détails

gui-open-workspace-directory = dossier

gui-open-workspace-title = Ouvrir un espace de travail

gui-open-workspace-verb = Ouvrir

gui-opening = ouverture de { $path }

gui-params-fold-section = Replier { $group }

gui-params-unfold-section = Déplier { $group }

gui-params-hide-values = Masquer les valeurs

gui-params-no-members = aucun membre pour l’instant

gui-params-points = { $count } points

gui-params-port-hint = Port { $port } du groupe ; le panneau Masques le modifie sur la ligne de { $member }

gui-params-port-member = { $member } : { $value }

gui-params-show-values = Afficher les valeurs

gui-params-values = { $count } valeurs

gui-pick-choose-folder = Choisir le dossier de la bibliothèque…

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

gui-quick-open-subworkspace = sous-espace de travail

gui-quick-open-title = Aller à un module, un sous-espace de travail, une image ou un espace de travail
gui-recent-filters-title = Filtres récents
gui-recent-filters-pinned = épinglé
gui-recent-filters-recent = récent
gui-recent-filters-smart = album dynamique
gui-recent-filters-none = Aucun filtre récent pour l’instant : les filtres utilisés dans la bibliothèque sont listés ici

gui-quick-open-workspace = espace de travail { $path }

gui-recording-saved = enregistrement sauvé dans { $path }

gui-recording-stopped = enregistrement arrêté : { $error }

gui-scene-saved = viewport.scene sauvé dans { $path }

gui-scopes-blue = Bleu

gui-scopes-green = Vert

gui-scopes-red = Rouge

gui-settings-account-last-used = Dernière utilisation

gui-settings-account-rename = Nom

gui-settings-account-rename-help = Le nom de ce compte dans Helios ; l'adresse avec laquelle il s'est connecté reste à côté.

gui-settings-account-service = Service

gui-settings-account-status = État

gui-settings-account-used-by = Utilisé par

gui-settings-accounts-add = Ajouter un compte

gui-settings-accounts-no-service = Aucun service avec des comptes n'est installé.

gui-settings-accounts-none = Aucun compte pour l'instant : ajoutez-en un ci-dessous.

gui-snapshot-failed = capture échouée : { $error }

gui-snapshot-unreadable = L’instantané « { $name } » ne peut pas être lu : { $error }

gui-snapshot-saved = { $target } capturé dans { $path }

gui-snapshots-not-saved = Les instantanés n’ont pas été enregistrés : { $error }

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

gui-subworkspace-gone = Le sous-espace de travail { $id } n’existe plus : retour là où il avait été ouvert

gui-template-keys-error = modèle { $name } : raccourcis : { $error }

gui-theme-exported = thème exporté vers { $path }

gui-theme-problem = thème { $name } : { $problem }

gui-theme-unknown = thème { $name } : pas de thème « { $name } » : { $default } est utilisé à la place

gui-vault-unreadable = Impossible de lire la bibliothèque : { $error }

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

gui-wizard-account = Compte
gui-wizard-account-settings = Réglages du compte

gui-wizard-add = Ajouter la source

gui-wizard-back = Retour

gui-wizard-card = Les cartes et appareils photo apparaissent d’eux-mêmes dans la liste quand ils sont insérés, avec les choix par défaut : il n’y a rien à ajouter.

gui-wizard-choices = Comment ses photos sont importées. Vous pourrez les changer plus tard dans la colonne Choix.

gui-wizard-choose = Choisir…

gui-wizard-folder = Dossier

gui-wizard-kind = De quel type de source s’agit-il ?

gui-wizard-name = Nom de la source

gui-wizard-name-help = Son nom dans la colonne Sources

gui-wizard-next = Suivant

gui-wizard-no-account = Aucun compte pour l’instant

gui-wizard-no-folder = Aucun dossier à cet endroit

gui-wizard-no-library = Ouvrez d’abord une bibliothèque : les comptes y sont conservés

gui-wizard-sign-in = Se connecter à un compte…

gui-wizard-title = Ajouter une source

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

gui-workspace-library-query = s’ouvre sur (comme une recherche : flag:pick rating:3)

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

health-fix-gpu-docs = Comment choisir le GPU

health-fix-plugin-disable = Le désactiver

health-fix-plugin-folder = Ouvrir son dossier

health-fix-plugin-update = Le mettre à jour : ouvrir son dossier

health-fix-snapshots-folder = Ouvrir le dossier des instantanés

health-gpu-adapter = Le GPU

health-gpu-cpu = Helios calcule sur le processeur

health-gpu-cpu-detail = { $hardware } est présent mais n’a pas été ouvert : { $why }

health-gpu-kernel-cpu = { $module } se replie sur le processeur

health-gpu-kernel-cpu-detail = Son noyau GPU ne compile pas sur { $adapter } : { $error }

health-gpu-kernels = Noyaux GPU

health-gpu-kernels-ok = Tous les noyaux GPU compilent ({ $count })

health-gpu-kernels-ok-detail = Sur { $adapter }

health-gpu-none-detail = Aucun GPU trouvé.

health-gpu-ok = Helios calcule sur { $adapter }

health-gpu-ok-detail = { $used }

health-gpu-software = Helios calcule sur un adaptateur logiciel, { $adapter }

health-gpu-software-detail = { $used } tourne sur le processeur alors que { $hardware } est présent : choisissez le GPU dans les réglages.

health-plugin-crashed = { $plugin } ne démarre pas

health-plugin-crashed-detail = { $why }. Journal : { $log }

health-plugin-disabled = { $plugin } est désactivé

health-plugin-disabled-detail = Journal : { $log }

health-plugin-not-granted = { $plugin } attend une autorisation

health-plugin-not-granted-detail = Il lui faut { $capability } avant de démarrer.

health-plugin-ok = { $plugin } répond

health-plugin-ok-detail = Version { $version }, dans { $dir }

health-plugin-unsupported = { $plugin } ne convient pas à cet Helios

health-plugin-unsupported-detail = { $problems }. Mettez-le à jour.

health-plugins = Extensions

health-snapshots = Instantanés du catalogue

health-snapshots-broken = Le dernier instantané du catalogue ne peut pas être restauré

health-snapshots-file-detail = { $file }, vieux de { $days } jours

health-snapshots-none-yet = Pas encore d’instantanés du catalogue

health-snapshots-none-yet-detail = Helios ne prend pas encore d’instantanés du catalogue ; les sauvegardes gardent les photos.

health-snapshots-ok = Le dernier instantané du catalogue est récent et restaurable

health-snapshots-old = Le dernier instantané du catalogue a { $days } jours

health-snapshots-unverified = Aucun instantané du catalogue n’est vérifié

health-snapshots-unverified-detail = { $count } instantanés, aucun relu encore.

look-default-bad-params = les paramètres { $node } du rendu : { $error }

look-default-no-camera = le rendu par défaut d’un boîtier demande `camera`

look-default-not-fitted = aucun rendu boîtier ajusté à garder : ajustez-en d’abord un au JPEG de l’appareil

look-default-what = donnez un seul de `look`, `image` ou `clear`

look-fit-cancelled = l’ajustement du rendu boîtier a été annulé : rien n’a été écrit

look-mixed-cameras = les photos viennent de { $count } boîtiers ({ $cameras }) : un rendu boîtier s’ajuste sur les photos d’un seul boîtier ; sélectionnez les photos d’un boîtier

node-batch-only = le point d’accès des lots ne répond qu’à load-source, export, cancel, job.cancel, batch_start, batch_end, aux requêtes workflow.*, à library.import, à settings.layer.*, à commands.list et à command.run

node-command-debug-only = seul le point d’accès de contrôle de débogage l’exécute (helios-node --control, versions de débogage)

node-command-needs-vault = le nœud ne garde aucun coffre : lancez-le avec --vault

node-command-needs-window = aucune fenêtre du bureau ne tient cet espace de travail

node-command-origin = `{ $origin }` n’est pas une origine : user, cli, control, agent:NOM ou script:NOM

node-command-params = `{ $id }` a refusé ses paramètres : { $error }

node-command-unknown = aucune commande `{ $id }`

node-edit-not-saved = fait, mais la retouche n’a pas été enregistrée : { $error }

node-job-unknown = la tâche { $job } est inconnue ou déjà terminée

node-load-refused = impossible de charger l’image : { $error }

node-no-extent = `{ $name }` n’a pas d’étendue : donnez une région

node-no-node = aucun nœud nommé `{ $name }`

node-no-settings = le nœud ne garde aucun réglage : lancez-le avec --vault

node-no-vault = le nœud n’a pas de coffre où importer : lancez-le avec --vault

node-param-refused = impossible de régler { $node }.{ $param } : { $error }

node-path-not-absolute = { $path } : donnez un chemin absolu, le nœud ne sait pas où vous êtes

node-source-refused = `{ $name }` ne peut pas prendre l’image : { $error }

resolve-failed = { $node } : { $reason }

resolve-not-asked = { $node } n’a pas été demandé : { $reason }

resolve-refused = { $node } : le modèle n’a pas répondu ({ $reason })

workflow-newer-format = l’exécution `{ $run }` a été conservée par un Helios plus récent : elle ne peut pas être traitée ici

workflow-no-item = l’exécution `{ $run }` n’a pas d’élément `{ $item }`

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

xmp-not-applied-and-substituted = { $not_applied } ; { $substituted }

xmp-substituted = { $count ->
    [one] { $count } élément appliqué avec un mode de remplacement : { $items }
   *[other] { $count } éléments appliqués avec un mode de remplacement : { $items }
}

xmp-substituted-item = { $label } ({ $to } à la place de { $from })

xmp-reason-blend = fusion non prise en charge : { $what }

xmp-reason-blending = fusion non prise en charge

xmp-reason-extra-instance = une seconde instance d’un module à instance unique

xmp-reason-mask = masque dessiné non pris en charge : { $why }

xmp-reason-mask-without-image = un masque dessiné, lu sans son image

xmp-reason-unknown-blend-version = paramètres de fusion v{ $version } non pris en charge

xmp-reason-unknown-module = pas de module Helios

xmp-reason-unreadable = illisible : { $error }

xmp-reason-unsupported-part = pas encore pris en charge : { $part }

xmp-reason-unsupported-version = version non prise en charge (lit { $known })

gui-library-auto-advance = Avance automatique

gui-library-compare-hint = Sélectionnez de deux à quatre images pour les comparer

gui-library-count = { $shown } sur { $total }
gui-library-facet-rating = Note
gui-library-facet-label = Étiquette
gui-library-facet-flag = Marque
gui-library-facet-camera = Appareil
gui-library-facet-lens = Objectif
gui-library-facet-type = Type
gui-library-facet-presence = Présence
gui-library-facet-backup = Sauvegarde
gui-library-facet-cull = Qualité
gui-library-cull-sharp = nette
gui-library-cull-soft = floue
gui-library-cull-clipped = écrêtée
gui-library-cull-unmeasured = non mesurée
gui-library-badge-soft = floue
gui-library-badge-blown = hautes lumières brûlées
gui-library-badge-crushed = ombres bouchées
gui-library-sort-focus = Netteté
gui-library-folders = Dossiers
gui-library-folders-elsewhere = Autres dossiers
gui-library-volume-offline = { $volume } { $count }, hors ligne, vu pour la dernière fois le { $date }
gui-library-folder-show = Afficher dans le gestionnaire de fichiers
gui-library-folder-scan = Analyser ce dossier
gui-library-folder-import = Importer les nouvelles photos d’ici
gui-library-albums = Albums
gui-library-albums-none = Aucun album pour l’instant
gui-library-quick = Collection rapide
gui-library-quick-tip = B ajoute les photos sélectionnées ou les retire
gui-library-quick-toggle = Ajouter ou retirer les photos sélectionnées
gui-library-remove = Retirer de la bibliothèque
gui-library-trash = Mettre à la corbeille…
gui-library-delete-rejected = Supprimer les rejetées…
gui-trash-title = Mettre { $count } photos à la corbeille ?
gui-trash-rejected-title = Supprimer { $count } photos rejetées ?
gui-trash-detail = Leurs { $files } fichiers dans { $place } vont à la corbeille du système, qui peut les restaurer ; Annuler ne le peut pas, et elles sortent de la bibliothèque.
gui-trash-verb = Mettre à la corbeille
gui-trash-folders = { $count } dossiers
gui-trash-refused = { $count } fichiers restent où ils sont : { $reason }
gui-trash-nothing = Aucun fichier ne peut aller à la corbeille. { $why }
gui-trash-done = { $count } fichiers mis à la corbeille (tâche { $job })
gui-trash-some-failed = { $count } en échec : { $error }
gui-trash-failed = Les fichiers n’ont pas été mis à la corbeille : { $error }
gui-library-quick-save = Enregistrer comme album…
gui-library-quick-clear = Vider la collection rapide
gui-library-quick-save-title = Enregistrer la collection rapide comme album
gui-library-quick-save-clear = puis vider la collection rapide
gui-library-quick-save-verb = Enregistrer
gui-library-album-new = Nouvel album…
gui-library-album-add = Ajouter les photos sélectionnées
gui-library-album-remove = Retirer les photos sélectionnées
gui-library-album-rename = Renommer…
gui-library-album-delete = Supprimer l’album
gui-library-album-new-title = Nouvel album
gui-library-album-new-verb = Créer
gui-library-album-rename-title = Renommer l’album
gui-library-album-rename-verb = Renommer
gui-library-album-name = nom
gui-library-album-new-smart = Nouvel album dynamique…
gui-library-album-rules = Modifier les règles…
gui-library-album-smart-tip = Album dynamique : ses règles choisissent ses photos
gui-library-album-move = Déplacer vers un ensemble…
gui-library-set-new = Nouvel ensemble…
gui-library-set-new-in = Nouvel ensemble à l’intérieur…
gui-library-set-new-title = Nouvel ensemble d’albums
gui-library-set-delete = Supprimer l’ensemble, garder ses albums
gui-library-set-delete-all = Supprimer l’ensemble et ses albums
gui-library-set-toggle = Afficher ou masquer les albums de { $name }
gui-library-set-tip = Ensemble d’albums : les photos de ses albums réunies
gui-library-drop-add = Ajouter { $count } photos
gui-library-drop-smart = Pas ici : les règles d’un album dynamique choisissent ses photos
gui-library-drop-set = Pas ici : déposez les photos sur un des albums de l’ensemble
gui-library-set-top = Aucun ensemble (au premier niveau)
gui-library-set-hint = ensemble d’albums
gui-library-set-none = Aucun ensemble d’albums où le déplacer : créez-en un avec Nouvel ensemble…
gui-library-set-move-title = Déplacer « { $name } » vers un ensemble
gui-smart-new-title = Nouvel album dynamique
gui-smart-rules-title = Les règles de l’album dynamique
gui-smart-rules-verb = Appliquer
gui-rule-title = Les règles de l’album dynamique
gui-rule-add-condition = + Condition
gui-rule-add-group = + Groupe
gui-rule-remove = Retirer
gui-rule-up = Monter
gui-rule-down = Descendre
gui-rule-undo = Annuler
gui-rule-redo = Rétablir
gui-rule-cancel = Annuler
gui-rule-apply = Appliquer
gui-rule-of-rules = de ces règles
gui-rule-match-all = Toutes
gui-rule-match-any = Au moins une
gui-rule-match-none = Aucune
gui-rule-and = et
gui-rule-match-name = Règles à respecter
gui-rule-field-name = Champ
gui-rule-op-name = Comparaison
gui-rule-value-name = Valeur
gui-rule-high-name = Seconde valeur
gui-rule-unit-name = Unité
gui-rule-field-rating = note
gui-rule-field-label = étiquette de couleur
gui-rule-field-tag = mot-clé
gui-rule-field-flag = marque
gui-rule-field-folder = dossier
gui-rule-field-album = album
gui-rule-field-captured = date de prise de vue
gui-rule-field-camera = appareil
gui-rule-field-make = marque de l’appareil
gui-rule-field-lens = objectif
gui-rule-field-file-type = type de fichier
gui-rule-field-title = titre
gui-rule-field-description = description
gui-rule-field-iso = ISO
gui-rule-field-focal-length = focale
gui-rule-field-aperture = ouverture
gui-rule-field-shutter = vitesse d’obturation
gui-rule-field-focus = netteté (focus)
gui-rule-field-clip-white = hautes lumières brûlées (part)
gui-rule-field-clip-black = ombres bouchées (part)
gui-rule-op-eq = est
gui-rule-op-ne = n’est pas
gui-rule-op-lt = est inférieure à
gui-rule-op-le = est au plus
gui-rule-op-gt = est supérieure à
gui-rule-op-ge = est au moins
gui-rule-op-between = est entre
gui-rule-op-within = est dans
gui-rule-op-has = a
gui-rule-op-lacks = n’a pas
gui-rule-op-is = est
gui-rule-op-is-not = n’est pas
gui-rule-op-contains = contient
gui-rule-op-in = est dans
gui-rule-op-not-in = n’est pas dans
gui-rule-period-today = aujourd’hui
gui-rule-period-this-year = cette année
gui-rule-period-last = les derniers
gui-rule-unit-days = jours
gui-rule-unit-weeks = semaines
gui-rule-unit-months = mois
gui-smart-match = correspondance
gui-smart-rating = au moins (étoiles)
gui-smart-label = étiquette de couleur
gui-smart-flag = marque
gui-smart-tag = mot-clé
gui-smart-camera = appareil (contient)
gui-smart-file-type = type de fichier
gui-smart-folder = dossier (absolu)
gui-smart-taken-from = pris à partir du (AAAA-MM-JJ)
gui-smart-taken-to = pris jusqu’au (AAAA-MM-JJ)
gui-album-not-changed = L’album n’a pas été modifié : { $error }

gui-library-empty = Aucune photo pour l’instant : importez une carte ou un dossier, ou déposez des photos sur la fenêtre

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
gui-stacks-top-why = Proposée au-dessus : { $why }

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

gui-import-form-title = Importer des photos

gui-import-path = carte ou dossier

gui-import-verb = Importer

gui-import-window-add = Ajouter là où elles sont

gui-import-window-add-favourite = Ajouter aux favoris

gui-import-window-all-types = Tous les types

gui-import-window-add-source = Ajouter une source…

gui-import-window-camera-look = Appliquer le rendu par défaut du boîtier

gui-import-window-camera-look-help = Les raws d’un boîtier qui a un rendu boîtier par défaut partent de lui, comme première version

gui-import-window-cancel = Annuler

gui-import-window-card-inserted = { $card } insérée

gui-import-window-card-note = Les photos d’une carte sont copiées : la carte sera formatée, les copies restent.

gui-import-window-cards = Cartes et appareils

gui-import-window-change-into = Choisir où vont les copies

gui-import-window-choices = Choix

gui-import-window-choose-on-the-right = Choisissez les photos dans le panneau de droite : elles s'affichent ici.

gui-import-window-choose = Choisir un dossier…

gui-import-window-contents = Contenu

gui-import-window-copy = Copier dans la bibliothèque

gui-import-window-counts = { $photos } photos, { $new } nouvelles, { $size }

gui-import-window-darktable = Lire les retouches darktable

gui-import-window-darktable-help = Chaque fichier .xmp, duplicata compris, devient une version de sa photo

gui-import-window-duplicates = Doublons

gui-import-window-duplicates-ask = Demander

gui-import-window-duplicates-keep = Garder

gui-import-window-duplicates-skip = Ignorer

gui-import-window-empty = Aucune photo dans les sources cochées

gui-import-window-example = ex. { $example }

gui-import-history-imports = Importations

gui-import-window-folders = Dossiers

gui-import-window-favourites = Favoris

gui-import-window-import = Importer { $count }

gui-import-window-in-library = dans votre bibliothèque

gui-import-window-into = Dans

gui-import-window-jpeg-only = JPEG seulement

gui-import-window-layout-day = Par jour

gui-import-window-layout-flat = Un seul dossier

gui-import-window-layout-month = Par mois

gui-import-window-layout-year = Par année

gui-import-window-library-folder = Le dossier de la bibliothèque

gui-import-window-more = Plus

gui-import-window-more-note = Les modèles de métadonnées et les décalages horaires viendront plus tard.

gui-import-window-choices-own = { $source } : ses propres choix

gui-import-window-choices-default = { $source } : les choix par défaut

gui-import-window-own-choices = Ses propres choix d’import

gui-import-window-choices-reset = Revenir aux choix par défaut

gui-import-window-choices-reset-help = Cette source reprend les choix par défaut

gui-import-window-choices-undo = Annuler

gui-import-window-name = Nom

gui-import-window-name-custom = Personnalisé

gui-import-window-name-date-camera = Date, heure et appareil

gui-import-window-name-date-seq = Date et numéro

gui-import-window-name-date-stem = Date et nom

gui-import-window-name-original = Nom d’origine

gui-import-window-new-only = Nouvelles seulement

gui-import-window-no-cards = Aucune carte insérée

gui-import-window-no-favourites = Aucun pour l’instant

gui-import-window-no-sample = Cochez une photo pour voir un exemple

gui-import-window-none-ticked = Cochez les photos à importer

gui-import-window-nothing = Cochez une carte ou un dossier à importer

gui-import-window-offline = hors ligne

gui-import-window-open = Importer des photos…

gui-import-window-pair = RAW+JPG

gui-import-window-raw-only = Raw seulement

gui-import-window-reading = Lecture de { $path }

gui-import-window-last-import = Dernière importation le { $when }

gui-import-window-remove-favourite = Retirer des favoris

gui-import-window-forget = Oublier ce dossier

gui-import-window-forget-hint = Retire ce que la carte des images sait des fichiers de ce dossier hors de la Bibliothèque ; il n’est plus enregistré tant que vous ne le listez pas

gui-import-window-select = Sélection :

gui-import-window-select-all = tout

gui-import-window-select-day = jour

gui-import-window-select-none = rien

gui-import-window-sources = Sources

gui-import-window-title = Importer des photos

gui-import-window-types = Types

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

gui-macro-make-title = Faire une macro

gui-macro-fold-detail = Modifié : { $changes }

gui-macro-fold-discard = Abandonner

gui-macro-fold-discard-hint = Revenir à l'intérieur de la macro tel qu'il était

gui-macro-fold-expose = Les exposer

gui-macro-fold-expose-hint = Les paramètres modifiés deviennent ceux de la macro, leurs valeurs gardées sur cette image

gui-macro-fold-keep = La garder ouverte

gui-macro-fold-keep-hint = La replier plus tard

gui-macro-fold-title = { $node } a changé pendant qu'elle était ouverte

gui-macro-fold-version = Enregistrer comme version { $version }

gui-macro-fold-version-hint = Les modifications deviennent celles de la macro, dans { $file } ; ses autres nœuds gardent leur version

gui-macro-iface-title = Interface de { $node }

gui-macro-iface-verb = Enregistrer comme { $file }

gui-macro-iface-filter = Filtrer

gui-macro-iface-inner = Paramètres internes

gui-macro-iface-shown = Montrés par la macro

gui-macro-iface-unchanged = Rien n’a encore changé

gui-macro-iface-none = Rien n’est montré : cochez des paramètres internes à gauche

gui-macro-iface-up = Monter

gui-macro-iface-down = Descendre

gui-macro-iface-remove = Ne plus le montrer

gui-macro-iface-pick = Sélectionnez un paramètre montré pour régler son nom, sa plage et sa valeur par défaut

gui-macro-iface-name = Nom

gui-macro-iface-name-hint = Le même nom sur deux lignes les lie : une valeur règle les deux

gui-macro-iface-linked = Lié à la première ligne de ce nom : il est montré comme elle

gui-macro-iface-label = Libellé

gui-macro-iface-description = Description

gui-macro-iface-unit = Unité

gui-macro-iface-group = Groupe

gui-macro-iface-default = Sa propre valeur par défaut

gui-macro-iface-range = Plage

gui-macro-iface-soft = Une plage souple

gui-macro-iface-soft-range = Plage souple

gui-macro-iface-new-slider = Nouveau curseur sémantique

gui-macro-iface-new-slider-hint = Un curseur qui déplace plusieurs nombres internes, chacun selon sa courbe

gui-macro-iface-point = x : la valeur du curseur ; y : la valeur interne à cet endroit

gui-macro-iface-point-add = Ajouter un point

gui-macro-iface-drive-remove = Ne plus le piloter

gui-macro-iface-drive-add = Piloter un autre paramètre

gui-macro-iface-default-value = Valeur par défaut

gui-macro-iface-options = Options gardées

gui-macro-make-verb = Faire

gui-macro-name = nom

gui-macro-publish-detail = Écrit { $composite } dans { $library }, d’où chaque espace de travail l’ajoute depuis le menu Ajouter du graphe. La publication n’est pas annulée : Ctrl+Z laisse le fichier ; supprimez-le de ce dossier pour la retirer. Une modification ultérieure est publiée comme une nouvelle version.

gui-macro-publish-title = Publier { $name } dans votre bibliothèque ?

gui-macro-publish-verb = Publier

gui-macro-published = { $composite } publiée dans { $file } : l’annulation ne la retire pas

gui-modules-fold-hint = Replier { $node } en un seul nœud

gui-modules-macro = macro { $version }

gui-modules-open-hint = Ouvrir { $node } : ses modules affichés

gui-modules-make-macro = Faire une macro…

gui-modules-make-macro-hint = Regrouper les modules sélectionnés en un seul nœud (Ctrl+G dans le graphe)

gui-modules-macro-interface = Interface…

gui-modules-macro-interface-hint = Choisir les paramètres que { $node } montre, enregistrés comme une nouvelle version

gui-modules-macro-publish = Publier…

gui-modules-macro-publish-hint = Copier cette version de { $node } dans votre bibliothèque, pour le menu Ajouter de chaque espace de travail ; demandé d’abord, et non annulable

gui-modules-save-workshop = Enregistrer comme atelier…

gui-modules-save-workshop-hint = Donner à { $node } une intention et des cibles réglées par image, enregistrées comme nouvelle version (expérimental)

gui-modules-macro-update = Passer à la version { $version }

gui-modules-macro-update-hint = Passer { $node } à la version { $version } de sa macro, ses valeurs reprises par nom

gui-workshops-experimental = Expérimental : peut changer ; ce qu’il enregistre reste une macro

gui-workshops-none = Aucun atelier dans cet espace de travail pour l’instant

gui-workshops-aim = Régler à nouveau

gui-workshops-aim-hint = Résoudre à nouveau ses cibles sur l’image ouverte (une étape d’annulation)

gui-workshops-aimed = cible : { $measure } à { $value } { $unit }

gui-workshops-applied = Appliqué : { $node }

gui-workshops-applied-hint = Sélectionner son nœud, pour le régler ci-dessous

gui-workshops-apply = Appliquer

gui-workshops-apply-hint = Insérer { $name } après le nœud d’où l’image est montrée, réglé sur l’image ouverte (une étape d’annulation)

gui-workshops-find = Chercher

gui-workshops-find-hint = ce que vous voulez : plus clair, moins de contraste…

gui-workshops-library = Votre bibliothèque

gui-workshops-measure-clip = écrêtage

gui-workshops-measure-contrast = contraste

gui-workshops-measure-ratio = écart entre zones

gui-workshops-measure-high = hautes lumières

gui-workshops-measure-median = tons moyens

gui-workshops-no-image = Ouvrez une image pour le régler

gui-workshops-none-found = Aucun atelier trouvé pour ces mots

gui-workshops-open = Ouvrir dans le graphe

gui-workshops-shipped = Fournis avec Helios

gui-workshops-save-intent = à quoi il sert

gui-workshops-save-keywords = mots-clés, séparés par des virgules

gui-workshops-save-target = { $param } : cible

gui-workshops-save-target-now = { $param } : cible (l’image maintenant : { $now })

gui-workshops-save-title = Enregistrer { $node } comme atelier

gui-workshops-save-value = { $param } : valeur (IL, ou % pour clip.high)

gui-workshops-save-verb = Enregistrer sous { $file }

gui-workshops-unit-percent = %

gui-workshops-unit-stops = IL

gui-workshops-workspace = Cet espace de travail

gui-workshops-describe = Décrivez ce que vous voulez

gui-workshops-describe-hint = contre-jour, la montagne et le ciel trop contrastés…

gui-workshops-design = Concevoir un atelier

gui-workshops-design-cancel = Annuler

gui-workshops-design-hint = Demander à votre propre agent (par MCP) un atelier fait pour ces mots ; Helios le vérifie sur l’image ouverte et l’applique en une étape d’annulation

gui-workshops-design-waiting = En attente de votre agent : il lit helios_workshop_context et envoie un brouillon avec helios_workshop_design

gui-workshops-designed = Conçu : { $workshop }

gui-workshops-ghost = { $param } : { $value } proposé par { $author }

gui-workshops-ghost-accept = Accepter

gui-workshops-ghost-accept-hint = Mettre cette valeur (une étape d’annulation) ; déplacez plutôt le curseur pour la remplacer

gui-workshops-ghost-reject = Rejeter

gui-workshops-session = Session enregistrée : { $steps } étapes

gui-workshops-session-keep = Garder

gui-workshops-session-keep-hint = Garder l’enregistrement de cette session : qui a réglé quoi, pourquoi, et ce que mesurait l’image. Non gardée, elle est abandonnée à sa fin
gui-workshops-automatic = Rendre automatique
gui-workshops-automatic-hint = Faire de vos réglages un atelier pour d’autres photos : une cible là où une mesure explique une valeur, sinon la même valeur ; vous confirmez chaque ligne
gui-workshops-row-target = Même cible
gui-workshops-row-fixed = Même valeur { $value }
gui-workshops-row-ai = Garder l’IA
gui-workshops-zone-parametric = Masque paramétrique
gui-workshops-ai-none = IA par photo : aucune
gui-workshops-ai-left = IA par photo : { $junctions } jonctions, { $calls } appels
gui-history-workshop-accept = Atelier « { $name } » : une photo acceptée
gui-history-workshop-decide = Atelier « { $name } » : une jonction décidée
gui-history-workshop-fix = Atelier « { $name } » : une photo corrigée
gui-history-workshop-revert = Atelier « { $name } » : une photo rétablie
gui-workshops-flag-ai-budget = budget IA épuisé
gui-workshops-flag-at-limit = à sa limite
gui-workshops-flag-deviation = loin des photos de test
gui-workshops-flag-fallback = pas d’IA : valeur de repli
gui-workshops-flag-low-confidence = IA incertaine
gui-workshops-flag-not-monotonic = visée impossible
gui-workshops-flag-target-missed = cible manquée
gui-workshops-flag-unlike = différente des photos de test
gui-workshops-review-accept = Accepter
gui-workshops-review-accept-hint = Écrire cette photo avec les valeurs trouvées par la passe, signalements compris
gui-workshops-review-accepted = acceptée
gui-workshops-review-applied = appliquée
gui-workshops-review-close = Fermer
gui-workshops-review-counts = { $photos } photos, { $flagged } à revoir, { $held } retenues ; appels IA : { $calls }
gui-workshops-review-decide = Décider
gui-workshops-review-decide-hint = Répondre à cette jonction pour cette photo : inscrit dans le carnet de décisions de l’atelier, une nouvelle passe la reprend
gui-workshops-review-failed = en échec
gui-workshops-review-fixed = corrigée à la main
gui-workshops-review-flagged-only = Seulement les photos à revoir
gui-workshops-review-held = retenue
gui-workshops-review-last = Revoir la dernière passe
gui-workshops-review-refresh = Actualiser
gui-workshops-review-rerun = Relancer retenues et décidées ({ $count })
gui-workshops-review-rerun-hint = Relancer l’atelier sur les photos retenues et celles dont une jonction est décidée : une nouvelle passe, une étape d’annulation
gui-workshops-review-revert = Rétablir
gui-workshops-review-revert-hint = Retirer la version de la passe pour cette photo : son édition d’avant la passe redevient courante
gui-workshops-review-reverted = rétablie
gui-workshops-review-set = Fixer
gui-workshops-review-set-hint = Corriger cette valeur à la main sur cette photo : une nouvelle version, son signalement levé
gui-workshops-review-stopped = a arrêté la passe
gui-workshops-review-title = Revue : { $name }, passe { $n }
gui-workshops-review-unchanged = inchangée
gui-workshops-review-value = { $slider } : { $before } avant, { $after } après ({ $source })
gui-workshops-source-ai = IA
gui-workshops-source-aimed = visée
gui-workshops-source-fallback = repli
gui-workshops-source-fixed = fixe
gui-workshops-source-user = à la main
gui-workshop-run-job = Atelier « { $name } »
gui-workshops-confirm = Confirmer
gui-workshops-confirm-hint = Écrire la version suivante de l’atelier avec ces choix, en une seule étape d’annulation ; une ligne gardée en IA est demandée à chaque photo

gui-workshops-session-kept = Gardée

gui-workshops-send-proxy = Laisser l’agent voir un petit aperçu de la photo (cette session seulement)

gui-library-import-photos = Importer des photos…

gui-import-dropped-title = { $count ->
    [one] Importer { $count } élément
   *[other] Importer { $count } éléments
}

gui-import-copy = Copier les photos dans le dossier de la bibliothèque

gui-import-copy-help = Décoché : les photos restent où elles sont, référencées sur place

gui-import-darktable = Lire les retouches darktable (fichiers .xmp)

gui-import-darktable-help = Chaque fichier annexe, doublons compris, devient une version de sa photo

gui-import-remember = Mémoriser pour cet espace de travail

gui-import-remember-help = La source d’import de l’espace de travail : le prochain import ne demande plus

gui-import-darktable-failed = { $sidecar } : non conservé : { $error }

gui-import-darktable-report = { $sidecar } : { $report }

gui-import-failed = L’import a échoué : { $error }

gui-import-ended = { $photos } photos importées, { $edits } retouches darktable

gui-import-ended-reported = { $photos } photos importées, { $edits } retouches darktable ; { $reported } pas conservées en entier : voir le panneau Journal

gui-import-drop = Déposez pour importer dans la bibliothèque

gui-import-cancelled = L’import a été annulé : { $photos } photos importées

gui-import-metadata = { $photos } photos ont reçu leurs notes, étiquettes ou mots-clés de l’importateur

gui-import-no-library = Aucune bibliothèque n’est ouverte : lancez Helios avec une bibliothèque (--vault FICHIER) pour importer des photos

gui-import-no-library-error = Aucune bibliothèque n’est ouverte : { $error }

gui-importer-folder = dossier

gui-importer-file = fichier

gui-importer-form-title = Importer depuis { $importer }

gui-importer-scanned = { $importer } : { $found } photos trouvées dans { $source }, { $refused } écartées

gui-importer-refused = { $importer } : { $path } écarté : { $why }

gui-importer-warning = { $importer } : { $warning }

gui-log-empty = Rien à signaler pour l’instant

gui-log-clear = Effacer

gui-workspace-library-darktable = lire les retouches darktable (fichiers .xmp) à l’import

gui-export-open-folder = Ouvrir le dossier

gui-export-queued = Export de { $count } photos vers { $folder }

gui-export-scope = photos

gui-export-scope-help = selected : les { $selected } sélectionnées ; shown : les { $shown } que montre la bibliothèque

gui-export-format = format

gui-export-quality = qualité JPEG

gui-export-quality-help = de 1 à 100, pour le JPEG seulement

gui-export-bits = bits par canal du PNG

gui-export-long-edge = grand côté (pixels)

gui-export-long-edge-help = Le plus grand côté au plus ; 0 garde la pleine taille

gui-export-space = espace colorimétrique

gui-export-folder = dossier

gui-export-pattern = noms des fichiers

gui-export-pattern-help = Comme un import renomme : { $fields } ; l’extension suit le format

gui-export-clash = quand un nom de fichier est déjà pris

gui-export-remember = Retenir pour cet espace de travail

gui-export-remember-help = Les réglages d’export de l’espace de travail : le prochain export part d’eux

gui-export-form-title = Exporter des photos

gui-export-verb = Exporter

gui-export-image-failed = { $image } : non exportée : { $error }

gui-export-cancelled = L’export a été annulé : { $written } photos écrites dans { $folder }

gui-export-ended-failed = { $written } photos exportées dans { $folder } ; { $failed } en échec : voir le panneau Journal

gui-export-ended = { $written } photos exportées dans { $folder }

gui-export-ended-skipped = { $outcome } ({ $skipped } laissées de côté : leur fichier existe)

gui-export-failed = L’export n’a pas démarré : { $error }

gui-status-health-problem = { $count ->
    [one] Bilan de santé : { $count } point demande votre attention, un problème
   *[other] Bilan de santé : { $count } points demandent votre attention, dont un problème
}

gui-status-health-warning = { $count ->
    [one] Bilan de santé : { $count } point demande votre attention
   *[other] Bilan de santé : { $count } points demandent votre attention
}

gui-status-health-show = Ouvrir le bilan de santé

gui-health-checking = Vérification…

gui-health-check-now = Vérifier maintenant

gui-health-not-yet = Pas encore vérifié : les vérifications tournent en arrière-plan.

gui-health-undo-ignore = Afficher à nouveau

gui-health-fix = Corriger…

gui-health-about = Helios vérifie les sauvegardes, les disques, le GPU, les plugins et les instantanés du catalogue en arrière-plan : peu après le démarrage, chaque jour, et quand un disque arrive ou part.

gui-health-fix-confirm = Corriger

gui-health-ignored-row = ignoré

gui-health-status = État

gui-health-summary-ok = Tout va bien

gui-health-summary-problems = Problèmes : { $problems }, avertissements : { $warnings }

gui-health-summary-warnings = Avertissements : { $count }

gui-settings-health = Santé

gui-settings-health-check = Bilan de santé

gui-health-ignore = Ignorer

health-card-short = { $card } ne peut pas encore être formatée

health-card-short-detail = Photos sans toutes leurs sauvegardes : { $count }. Gardez la carte.

health-cards = Cartes prêtes à formater

health-check-crashed = La vérification { $check } a échoué

health-check-crashed-detail = Elle s’est arrêtée avant de pouvoir dire quoi que ce soit.

health-fix-verify = Sauvegarder maintenant, ce qui les vérifie

health-free-space = Espace libre

health-free-space-ok = { $volume } a de la place

health-free-space-low = { $volume } manque de place

health-free-space-full = { $volume } est presque plein

health-free-space-detail = { $free } libres sur { $capacity } ({ $percent } %)

health-fix-back-up-now = Sauvegarder maintenant

health-fix-library-folder = Ouvrir le dossier de la bibliothèque

health-fix-no-destination = Aucun disque de sauvegarde branché : ouvrir la page Stockage

health-fix-plug-in = Brancher { $volume } pour sauvegarder

health-fix-set-strategy = Définir une stratégie de sauvegarde

health-fix-show-volume = Le montrer dans la page Stockage

health-vault-integrity = Intégrité du catalogue

health-vault-ok = Le catalogue est sain

health-vault-ok-detail = { $file } a passé la vérification rapide de SQLite

health-library = La bibliothèque

health-library-closed = La bibliothèque ne s'ouvre pas

health-library-closed-detail = { $file } : { $error }

health-library-no-room = Le disque de la bibliothèque est presque plein

health-library-no-room-detail = { $free } libres sur { $capacity } sur { $volume } : la bibliothèque a besoin de place pour grandir.

health-library-ok = La bibliothèque s'ouvre et a de la place

health-library-ok-detail = { $file }

health-library-folder = Dossier de la bibliothèque

health-library-folder-none = Aucun dossier de bibliothèque choisi

health-library-folder-none-detail = Les imports qui copient les photos et les photos choisies n'ont nulle part où aller : ils demandent d'abord un dossier.

health-library-folder-ok = Un dossier de bibliothèque est choisi

health-library-folder-ok-detail = { $folder }

health-fix-choose-library-folder = Choisir un dossier…

health-met = Photos sauvegardées

health-met-ok = Chaque photo a ses sauvegardes

health-met-ok-detail = Photos vérifiées : { $count }

health-met-short = Photos sans toutes leurs sauvegardes : { $count }

health-met-short-detail = Copies vérifiées que { $name } demande par photo : { $copies }

health-seen = Disques de sauvegarde vus récemment

health-seen-ok = Chaque disque de sauvegarde a été vu récemment

health-seen-ok-detail = Dans les { $limit } derniers jours

health-seen-stale = { $volume } a été vu pour la dernière fois il y a { $days } jours

health-seen-stale-detail = Ses sauvegardes ne sont pas tenues à jour tant qu'il reste absent (plus de { $limit } jours).

health-strategy = Stratégie de sauvegarde

health-strategy-none = Aucune sauvegarde n'est configurée

health-strategy-none-detail = Chaque photo n'a qu'une copie, dans la bibliothèque : un disque qui lâche la perd.

health-strategy-set = Les sauvegardes suivent { $name }

health-strategy-set-detail = Copies vérifiées par photo : { $copies }

health-unverified = Copies vérifiées

health-unverified-detail = Une copie compte une fois relue et conforme à son empreinte.

health-unverified-some = Photos dont des copies de sauvegarde ne sont pas vérifiées : { $count }

health-vault-damaged = Le catalogue est endommagé

health-vault-damaged-detail = { $file } : { $first } ({ $count } erreurs). Restaurez un instantané du catalogue.

health-vault-unreadable = Le catalogue ne peut pas être vérifié

health-vault-unreadable-detail = { $file } : { $error }

init-library-folder = Dossier de la bibliothèque

init-library-folder-why = Les imports y copient les photos, et les photos choisies y vont.

look-load-failed = impossible de lire la photo : { $error }

look-render-failed = impossible de développer le raw : { $error }

gui-library-offline-on = Sur { $volume }, vu pour la dernière fois le { $date } sur { $node }

gui-library-offline-unknown = Sur un disque qui n’est pas branché

gui-library-missing-note = Manquante : son fichier n’est plus où il était, et aucune autre copie n’est connue

gui-library-plug-in = Branchez { $volume } pour ouvrir cette photo (vu pour la dernière fois le { $date } sur { $node })

gui-library-badge-offline = hors ligne
gui-library-badge-quick = rapide

gui-library-cannot-open = La bibliothèque ne peut pas être ouverte : { $error }

gui-library-badge-missing = manquante

gui-storage-intro = Les disques que Helios a vus. Nommez-les et dites à quoi sert chacun : Helios n’écrit que sur les disques de bibliothèque, d’archive, de sauvegarde et de travail.

gui-storage-kind-internal = Disque interne

gui-storage-kind-external = Disque externe

gui-storage-kind-removable = Carte ou clé

gui-storage-kind-share = Partage réseau

gui-storage-kind-remote = Stockage distant

gui-storage-role-library = Bibliothèque

gui-storage-role-archive = Archive

gui-storage-role-backup = Sauvegarde

gui-storage-role-ingest = Source

gui-storage-role-scratch = Travail

gui-storage-name-hint = Son nom ; Entrée l’enregistre

gui-storage-online = en ligne

gui-storage-offline = hors ligne

gui-storage-free = { $free } libres sur { $capacity }

gui-storage-backup-now = Sauvegarder maintenant

gui-storage-backup-off-site = L’une d’elles hors site

gui-storage-backup-on = Garder des copies de sauvegarde vérifiées de chaque photo

gui-storage-backup-on-connect = Quand un disque de sauvegarde est branché

gui-storage-backup-on-import = Après chaque import

gui-storage-dest-archive = Tout disque d’archive

gui-storage-dest-backup = Tout disque de sauvegarde

gui-storage-off-site = Hors site

gui-storage-off-site-hint = Gardé loin des autres disques (un autre bâtiment)

gui-storage-raid = RAID

gui-storage-raid-note = Un RAID compte pour une seule copie : il survit à un disque mort, pas à une suppression ni à un vol.

gui-storage-node = Dans { $node }

gui-storage-last-seen = Vu pour la dernière fois le { $date } sur { $node }

gui-storage-mark-explain = Ce disque n’est connu que par son nom de volume et sa taille : Helios pourrait le confondre avec un autre. Le marquer écrit un petit fichier, .helios/volume.json, à sa racine : rien d’autre ne change sur le disque.

gui-storage-mark-confirm = Écrire le fichier d’identité

gui-storage-mark = Marquer ce disque…

gui-storage-forget-hint = Retire ce que la carte des images sait des fichiers de ce volume hors de la Bibliothèque ; le volume reste connu

gui-import-window-delete-preset = Supprimer le préréglage

gui-import-window-preset = Préréglage

gui-import-window-preset-default = Par défaut

gui-import-window-preset-last-used = Dernier utilisé

gui-import-window-preset-name = Nom du préréglage

gui-import-window-preset-save = Enregistrer

gui-import-window-save-preset = Enregistrer comme préréglage…

gui-import-window-totals = { $photos ->
    [one] { $photos } photo, { $size }
   *[other] { $photos } photos, { $size }
}
gui-catalogue-days = il y a { $days } jours
gui-catalogue-here = à côté du catalogue
gui-catalogue-none = Aucun instantané pour l'instant.
gui-catalogue-on-disk = sur { $disk }
gui-catalogue-one-day = il y a 1 jour
gui-catalogue-restore = Restaurer…
gui-catalogue-restore-ask = Restaurer le catalogue depuis l'instantané d'{ $when } ? Les changements faits depuis sont perdus. Le catalogue actuel est mis de côté, et Helios se ferme puis redémarre sur l'instantané.
gui-catalogue-restore-confirm = Restaurer et redémarrer
gui-catalogue-snapshot-now = Instantané maintenant
gui-catalogue-today = aujourd'hui
gui-backup-wizard = Configurer les sauvegardes
gui-backup-wizard-about = Helios garde des copies vérifiées de chaque photo sur vos disques de sauvegarde : choisissez combien, où et quand.
gui-backup-wizard-step-copies = Copies
gui-backup-wizard-copies = Copies de chaque photo
gui-backup-wizard-copies-about = Chaque copie est sur un disque différent, en plus de la photo dans la bibliothèque. Deux copies survivent à la panne d'un disque.
gui-backup-wizard-step-destination = Disques
gui-backup-wizard-destination = Copier sur
gui-backup-wizard-destination-backup = Tout disque au rôle de sauvegarde
gui-backup-wizard-destination-archive = Tout disque au rôle d'archive
gui-backup-wizard-destination-help = Donnez son rôle à un disque dans Stockage › Volumes ; un disque par copie.
gui-backup-wizard-step-off-site = Hors site
gui-backup-wizard-off-site = Garder l'une des copies hors site
gui-backup-wizard-off-site-help = Un disque gardé ailleurs survit à un incendie ou à un vol : marquez-le hors site dans Stockage › Volumes.
gui-backup-wizard-step-when = Quand
gui-backup-wizard-when-about = Sauvegarder maintenant, sur la page Sauvegarde, fait des copies à tout moment.
gui-backup-wizard-on-import = Après chaque import
gui-backup-wizard-on-connect = Quand un disque de sauvegarde est branché
gui-backup-wizard-finish = Utiliser cette stratégie
gui-backup-wizard-open = Configurer les sauvegardes…

gui-compare-first = Comparer montre { $count } photos au plus : les { $count } premières sont comparées

gui-compare-rating = Note de la photo active

gui-compare-too-few = Sélectionnez deux photos ou plus pour les comparer

gui-workspace-library-compare = s’ouvre dans Comparer, les photos côte à côte

gui-library-compare = Comparer

gui-library-compare-tip = Les photos sélectionnées côte à côte (C)

gui-library-sort-captured = Date de prise de vue

gui-library-sort-imported = Date d’import

gui-library-sort-name = Nom de fichier

gui-library-sort-rating = Note

gui-library-sort-edited = Dernière retouche

gui-library-sort-size = Taille du fichier

gui-library-sort-id = Contenu

gui-library-sort-tip = Trier par

gui-library-sort-ascending = Croissant : la plus ancienne, la plus petite ou A d’abord

gui-library-sort-descending = Décroissant : la plus récente, la plus grande ou Z d’abord

gui-quit-title = Quitter Helios ?

gui-quit-jobs-detail = { $count } tâches en cours ; quitter les annule

gui-quit-verb = Quitter

gui-consent-allow = Autoriser

gui-consent-asks = { $plugin } vous demande l’autorisation de :

gui-consent-declined = Rien n’a changé : { $plugin } ne fonctionne pas sans ces autorisations. Vous pourrez les accorder plus tard dans Réglages > Extensions.

gui-consent-deny = Ne pas autoriser

gui-consent-from = Extension { $id }

gui-consent-later = Helios retient votre réponse ; Réglages > Extensions permet de la changer.

gui-consent-title = Autoriser { $plugin } ?

gui-settings-plugin-granted = Autorisé

gui-settings-plugin-no-capability = Elle ne demande aucune autorisation.

gui-settings-plugin-reason = Pourquoi

gui-settings-plugin-refused = Les réglages de cette extension ne sont pas affichés : { $problems }

gui-settings-plugin-settings = Réglages

gui-settings-plugins = Extensions

gui-settings-plugins-capabilities = Autorisations

gui-oauth-client-built-in = Le client de Helios

gui-oauth-client-environment = Celui de l’environnement

gui-oauth-client-none = Aucun dans cette version : utilisez le vôtre

gui-oauth-client-own = Votre propre client

gui-settings-accounts-clients = Utiliser mon propre client OAuth

gui-settings-client-helios = Utiliser le client de Helios

gui-settings-client-id = Votre ID client

gui-settings-client-secret = Votre code secret client

gui-settings-client-secret-help = Gardé dans le trousseau et jamais réaffiché : saisissez-le de nouveau pour le changer.

gui-settings-client-used = Client utilisé

gui-settings-clients-about = Helios se connecte à ces services avec son propre client OAuth. Pour utiliser le vôtre à la place (une application de bureau que vous avez enregistrée auprès du service), donnez son ID client et son code secret : ils restent dans le trousseau de cet ordinateur. Reconnectez-vous ensuite.

gui-settings-clients-none = Aucun service se connectant par OAuth n’est installé.

gui-settings-focus-import = Fenêtre d’import

gui-settings-focus-library = { $window } : bibliothèque { $library }

gui-settings-focus-main = Fenêtre principale

gui-settings-focus-workspace = { $window } : espace de travail { $workspace }, bibliothèque { $library }

gui-settings-focus-workspace-alone = { $window } : espace de travail { $workspace }

gui-settings-for-library = Cette bibliothèque

gui-settings-for-library-about = tant que cette bibliothèque est ouverte

gui-settings-for-workspace = Cet espace de travail

gui-settings-for-workspace-about = tant que cet espace de travail est ouvert

gui-health-owner-storage = Stockage

gui-health-owner-library = Bibliothèque

gui-health-owner-backup = Sauvegardes

gui-health-owner-gpu = GPU

gui-health-owner-plugins = Extensions

gui-health-owner-catalogue = Catalogue

gui-health-owner-other = Autres vérifications

gui-import-auto-ask = { $files } nouvelles photos ({ $size }) sur { $source } : plus qu’un import automatique n’en prend sans demander

gui-import-auto-confirm = Premier import automatique depuis { $source } : vérifiez les photos, puis Importer le confirme

gui-import-auto-confirmed = Import automatique confirmé : il se fera désormais tout seul

gui-import-auto-held = Les imports automatiques sont suspendus : { $source } n’est pas importé

gui-import-auto-open = Ouvrir

gui-import-auto-paused = Imports automatiques suspendus

gui-import-auto-resume = Reprendre

gui-import-auto-resumed = Imports automatiques repris

gui-settings-automatic-imports = Imports automatiques

gui-import-no-library-folder = Choisissez d'abord un dossier de bibliothèque : les imports y copient les photos (menu Helios › Configurer Helios…).

gui-init-library-folder-choose = Choisir…

gui-init-library-folder-help = Un dossier sur un disque qui a de la place pour vos photos ; il est créé s'il manque.

gui-init-wizard = Configurer Helios

gui-init-wizard-about = Quelques choix avant les premières photos : chacun se change plus tard.

gui-init-wizard-finish = Terminer

health-library-folder-unwritable = Le dossier de la bibliothèque n'est pas accessible en écriture

health-library-folder-unwritable-detail = { $folder } manque ou est en lecture seule : les imports ne peuvent pas y copier les photos.

library-folder-no-room = Ce disque a { $free } Mio libres : choisissez un dossier qui a de la place pour les photos.

library-folder-not-absolute = Donnez le chemin complet du dossier.

library-folder-not-writable = Helios ne peut pas écrire dans ce dossier : { $why }

gui-import-albums-reading = Lecture des albums…

gui-import-albums-more = Plus d’albums

gui-import-albums-none = Aucun album

gui-import-albums-failed = Impossible de lire les albums :

gui-import-album-reading = Lecture de l’album…

gui-import-album-failed = Impossible de lire l’album :

gui-import-album-choose = Choisissez un album sous le compte, dans Sources.
