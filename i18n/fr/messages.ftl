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

xmp-not-applied = { $count ->
    [one] { $count } élément d’historique sur { $items } non appliqué, conservé pour plus tard : { $groups }
   *[other] { $count } éléments d’historique sur { $items } non appliqués, conservés pour plus tard : { $groups }
}

xmp-not-applied-group = { $labels } ({ $reason })

xmp-reason-blending = fusion non prise en charge

xmp-reason-extra-instance = une seconde instance d’un module à instance unique

xmp-reason-unknown-blend-version = paramètres de fusion v{ $version } non pris en charge

xmp-reason-unknown-module = pas de module Helios

xmp-reason-unreadable = illisible : { $error }

xmp-reason-unsupported-version = version non prise en charge (lit { $known })
