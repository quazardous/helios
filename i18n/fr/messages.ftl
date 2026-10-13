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

export-png-bits = le PNG a 8 ou 16 bits, pas { $bits }

export-write-failed = impossible d’écrire { $path } : { $error }

gui-about-crashes = Rapports de plantage : { $path }

gui-about-logs = Fichier journal : { $path }

gui-confirmed-by-policy = { $verb } : confirmé par la règle

gui-copied = copié { $text }

gui-edit-does-not-fit = Une partie de la retouche enregistrée ne correspond pas à ce réseau : { $part }

gui-edit-not-saved = La retouche n’a pas été enregistrée : { $error }

gui-gesture-in-progress = { $gesture } est en cours

gui-job-batch-done = Lot terminé

gui-job-batch-failed = Échec du lot

gui-job-export-done = Export terminé

gui-job-export-failed = Échec de l’export

gui-library-not-saved = Non enregistré, la bibliothèque montre ce que contient le coffre : { $error }

gui-masks-group-full = le groupe « { $group } » contient déjà { $count } membres

gui-masks-group-locked = le groupe « { $group } » est verrouillé : ses membres restent

gui-masks-hide = Masquer { $name } : son opacité éteinte

gui-masks-into-itself = un groupe ne peut pas aller en lui-même

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

gui-masks-not-a-group = « { $name } » n’est pas un groupe : seul un groupe accueille des membres

gui-masks-nothing-beside-root = rien ne va à côté du masque du module : déposez dedans

gui-masks-one-at-a-time = les membres se déplacent un par un

gui-masks-root-stays = le masque du module reste la racine

gui-masks-show = Afficher { $name } : son opacité à fond

gui-masks-unlock = Déverrouiller { $name }

gui-no-dialogs = aucune boîte de dialogue pour montrer ceci

gui-open-workspace-directory = dossier

gui-open-workspace-title = Ouvrir un espace de travail

gui-open-workspace-verb = Ouvrir

gui-opening = ouverture de { $path }

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

gui-recording-saved = enregistrement sauvé dans { $path }

gui-recording-stopped = enregistrement arrêté : { $error }

gui-scene-saved = viewport.scene sauvé dans { $path }

gui-snapshot-failed = capture échouée : { $error }

gui-snapshot-saved = { $target } capturé dans { $path }

gui-theme-exported = thème exporté vers { $path }

gui-theme-problem = thème { $name } : { $problem }

gui-theme-unknown = thème { $name } : pas de thème « { $name } » : { $default } est utilisé à la place

gui-vault-unreadable = Impossible de lire le coffre : { $error }

gui-window-frame-error = cadre de la fenêtre : { $error }

gui-library-file = { $size } · modifié le { $day }

gui-param-default = { $path } · par défaut { $value }

gui-param-held = { $label } : maintenu à { $value }

gui-param-hint = Glisser ou flèches pour changer, la molette une fois le focus pris ou avec Alt (Maj : glissé plus fin, pas plus grands) ; double clic pour réinitialiser ; Entrée pour saisir une valeur

gui-param-not-a-number = { $label } : « { $typed } » n’est pas un nombre

gui-param-past-range = { $label } : au-delà de sa plage habituelle

gui-size-bytes = { $size } o

gui-size-gib = { $size } Gio

gui-size-kib = { $size } Kio

gui-size-mib = { $size } Mio

node-batch-only = le point d’accès des lots ne répond qu’à load-source, export et cancel

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
