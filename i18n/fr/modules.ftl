# Traduction de ../en/modules.ftl, sous les mêmes clés.

param-blur-radius =
    .description = écart type, en pixels de pleine résolution
    .label = rayon

param-decide-exposure-exposure =
    .description = réglage d’exposition, en IL : positif éclaircit, négatif assombrit
    .label = exposition

param-demosaic-method =
    .description = auto : RCD sur les matrices de Bayer, Markesteijn sur les X-Trans
    .label = méthode

param-demosaic-method-choice-auto = auto

param-demosaic-method-choice-bilinear = bilinéaire

param-demosaic-method-choice-markesteijn = markesteijn

param-demosaic-method-choice-rcd = rcd

param-exposure-black =
    .description = correction du niveau de noir
    .label = noir

param-exposure-compensate-bias =
    .description = annuler la correction d’exposition de l’appareil
    .label = compenser la correction

param-exposure-exposure =
    .description = réglage d’exposition, en IL
    .label = exposition

param-highlights-clip =
    .description = seuil d’écrêtage, relatif au blanc du capteur
    .label = écrêtage

param-mask-blend-opacity =
    .description = multiplie le masque
    .label = opacité

param-mask-blur-radius =
    .description = écart type, en pixels de pleine résolution
    .label = rayon

param-mask-brush-invert =
    .description = inverse la forme, avant son opacité
    .label = inverser

param-mask-brush-opacity =
    .description = multiplie la forme, après son inversion
    .label = opacité

param-mask-brush-points =
    .description = jusqu’à 256 points, 9 valeurs chacun : le coin, la poignée avant, la poignée après (x, y chacune, en pixels de l’image), le rayon (en pixels de l’image), la dureté et la densité (de 0 à 1)
    .label = points

param-mask-circle-feather =
    .description = largeur du fondu au-delà du rayon, en pixels de l’image
    .label = fondu

param-mask-circle-invert =
    .description = inverse la forme, avant son opacité
    .label = inverser

param-mask-circle-opacity =
    .description = multiplie la forme, après son inversion
    .label = opacité

param-mask-circle-radius =
    .description = où le fondu commence, en pixels de l’image
    .label = rayon

param-mask-circle-x =
    .description = centre, en pixels de l’image depuis la gauche
    .label = x

param-mask-circle-y =
    .description = centre, en pixels de l’image depuis le haut
    .label = y

param-mask-combine-group-invert =
    .description = inverse le masque combiné
    .label = inverser le groupe

param-mask-combine-group-opacity =
    .description = multiplie le masque combiné, après son inversion
    .label = opacité du groupe

param-mask-combine-invert =
    .description = 1 pour inverser un membre avant son opacité, par port
    .label = inverser

param-mask-combine-opacity =
    .description = l’opacité de chaque membre, par port (1 quand elle manque)
    .label = opacité

param-mask-combine-operator =
    .description = comment les membres se combinent, dans l’ordre des ports
    .label = opérateur

param-mask-combine-operator-choice-difference = différence

param-mask-combine-operator-choice-exclusion = exclusion

param-mask-combine-operator-choice-maximum = maximum

param-mask-combine-operator-choice-minimum = minimum

param-mask-combine-operator-choice-product = produit

param-mask-combine-operator-choice-screen = superposition

param-mask-combine-operator-choice-sum = somme

param-mask-ellipse-feather =
    .description = largeur du fondu au-delà du bord, en pixels de l’image (le long du grand axe si proportionnel)
    .label = fondu

param-mask-ellipse-invert =
    .description = inverse la forme, avant son opacité
    .label = inverser

param-mask-ellipse-opacity =
    .description = multiplie la forme, après son inversion
    .label = opacité

param-mask-ellipse-proportional =
    .description = le fondu suit chaque demi-axe au lieu de garder une seule largeur
    .label = proportionnel

param-mask-ellipse-radius-x =
    .description = demi-axe le long de l’axe x tourné, en pixels de l’image
    .label = rayon_x

param-mask-ellipse-radius-y =
    .description = demi-axe le long de l’axe y tourné, en pixels de l’image
    .label = rayon_y

param-mask-ellipse-rotation =
    .description = degrés de l’axe x vers l’axe y (sens horaire à l’écran)
    .label = rotation

param-mask-ellipse-x =
    .description = centre, en pixels de l’image depuis la gauche
    .label = x

param-mask-ellipse-y =
    .description = centre, en pixels de l’image depuis le haut
    .label = y

param-mask-feather-radius =
    .description = rayon de la fenêtre, en pixels de pleine résolution
    .label = rayon

param-mask-from-luma-high =
    .description = luminance où le masque est plein
    .label = haut

param-mask-from-luma-low =
    .description = luminance où le masque commence
    .label = bas

param-mask-gradient-feather =
    .description = largeur de la transition, en pixels de l’image
    .label = fondu

param-mask-gradient-invert =
    .description = inverse la forme, avant son opacité
    .label = inverser

param-mask-gradient-opacity =
    .description = multiplie la forme, après son inversion
    .label = opacité

param-mask-gradient-profile =
    .description = la transition : linéaire, ou sigmoïde (une fonction d’erreur)
    .label = profil

param-mask-gradient-profile-choice-linear = linéaire

param-mask-gradient-profile-choice-sigmoid = sigmoïde

param-mask-gradient-rotation =
    .description = degrés : 0 est plein au-dessus de la ligne, 90 plein à sa gauche
    .label = rotation

param-mask-gradient-x =
    .description = un point de la ligne, en pixels de l’image depuis la gauche
    .label = x

param-mask-gradient-y =
    .description = un point de la ligne, en pixels de l’image depuis le haut
    .label = y

param-mask-levels-brightness =
    .description = décale le masque vers le haut ou le bas
    .label = luminosité

param-mask-levels-contrast =
    .description = raidit le masque autour de son milieu
    .label = contraste

param-mask-levels-invert =
    .description = inverse le masque, après le contraste et la luminosité
    .label = inverser

param-mask-levels-opacity =
    .description = multiplie le masque, en dernier
    .label = opacité

param-mask-parametric-blue =
    .description = plage du bleu : bas, haut, adoucissement en dessous, adoucissement au-dessus
    .label = bleu

param-mask-parametric-blue-invert =
    .description = inverse la sélection de ce canal
    .label = inverser le bleu

param-mask-parametric-chroma =
    .description = plage de chroma (Cz x 100) : bas, haut, adoucissement en dessous, adoucissement au-dessus
    .label = chroma

param-mask-parametric-chroma-invert =
    .description = inverse la sélection de ce canal
    .label = inverser la chroma

param-mask-parametric-green =
    .description = plage du vert : bas, haut, adoucissement en dessous, adoucissement au-dessus
    .label = vert

param-mask-parametric-green-invert =
    .description = inverse la sélection de ce canal
    .label = inverser le vert

param-mask-parametric-hue =
    .description = plage de teinte, en tours : bas, haut, adoucissement en dessous, adoucissement au-dessus
    .label = teinte

param-mask-parametric-hue-invert =
    .description = inverse la sélection de ce canal
    .label = inverser la teinte

param-mask-parametric-invert =
    .description = inverse le masque combiné
    .label = inverser

param-mask-parametric-lightness =
    .description = plage de clarté (Jz x 100) : bas, haut, adoucissement en dessous, adoucissement au-dessus
    .label = clarté

param-mask-parametric-lightness-invert =
    .description = inverse la sélection de ce canal
    .label = inverser la clarté

param-mask-parametric-luminance =
    .description = plage de luminance : bas, haut, adoucissement en dessous, adoucissement au-dessus
    .label = luminance

param-mask-parametric-luminance-invert =
    .description = inverse la sélection de ce canal
    .label = inverser la luminance

param-mask-parametric-red =
    .description = plage du rouge : bas, haut, adoucissement en dessous, adoucissement au-dessus
    .label = rouge

param-mask-parametric-red-invert =
    .description = inverse la sélection de ce canal
    .label = inverser le rouge

param-mask-path-feather =
    .description = largeur du fondu hors du chemin, en pixels de l’image
    .label = fondu

param-mask-path-invert =
    .description = inverse la forme, avant son opacité
    .label = inverser

param-mask-path-opacity =
    .description = multiplie la forme, après son inversion
    .label = opacité

param-mask-path-points =
    .description = jusqu’à 64 points, 6 valeurs chacun : le coin, la poignée avant, la poignée après (x, y chacune, en pixels de l’image) ; moins de 3 points ne dessinent rien
    .label = points

param-mask-raster-invert =
    .description = inverse le masque réutilisé, avant son opacité
    .label = inverser

param-mask-raster-opacity =
    .description = multiplie le masque réutilisé, après son inversion
    .label = opacité

param-sigmoid-black-target =
    .description = noir de l’affichage, en %
    .label = cible du noir

param-sigmoid-contrast =
    .description = contraste autour du gris moyen
    .label = contraste

param-sigmoid-preserve-hue =
    .description = part de la teinte à garder, en %
    .label = préserver la teinte

param-sigmoid-skew =
    .description = penche le contraste vers les ombres ou les hautes lumières
    .label = inclinaison

param-sigmoid-white-target =
    .description = blanc de l’affichage, en %
    .label = cible du blanc

param-vkdt-contrast-detail =
    .description = le renforcement du détail
    .label = détail

param-vkdt-contrast-edges =
    .description = le seuil de contours du filtre guidé
    .label = contours

param-vkdt-contrast-radius =
    .description = le rayon du filtre guidé, une fraction de la largeur de l’image
    .label = rayon

param-vkdt-contrast-thrs =
    .description = le seuil du renforcement du détail
    .label = seuil

param-vkdt-exposure-exposure =
    .description = la correction d’exposition, en diaphs
    .label = exposition

param-vkdt-usm-amount =
    .description = à quel point augmenter la netteté
    .label = quantité

param-vkdt-usm-thrs =
    .description = le seuil pour ne pas renforcer le bruit
    .label = seuil

param-whitebalance-blue =
    .description = coefficient du bleu (mode utilisateur)
    .label = bleu

param-whitebalance-green =
    .description = coefficient du vert (mode utilisateur)
    .label = vert

param-whitebalance-mode =
    .description = les coefficients de l’appareil, ou ceux de l’utilisateur
    .label = mode

param-whitebalance-mode-choice-as-shot = tel que pris

param-whitebalance-mode-choice-user = utilisateur

param-whitebalance-red =
    .description = coefficient du rouge (mode utilisateur)
    .label = rouge
