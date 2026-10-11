# Generated from Helios's declarations by `cargo xtask i18n extract`:
# do not edit. Translations live in i18n/<locale>/ under the same names.

param-blur-radius =
    .description = standard deviation, in full-resolution pixels
    .label = radius

param-decide-exposure-exposure =
    .description = exposure adjustment, in EV: positive brightens, negative darkens
    .label = exposure

param-demosaic-method =
    .description = auto: RCD on Bayer patterns, Markesteijn on X-Trans
    .label = method

param-demosaic-method-choice-auto = auto

param-demosaic-method-choice-bilinear = bilinear

param-demosaic-method-choice-markesteijn = markesteijn

param-demosaic-method-choice-rcd = rcd

param-exposure-black =
    .description = black level correction
    .label = black

param-exposure-compensate-bias =
    .description = take back the camera's exposure bias
    .label = compensate_bias

param-exposure-exposure =
    .description = exposure adjustment, in EV
    .label = exposure

param-highlights-clip =
    .description = clipping threshold, relative to the sensor's white
    .label = clip

param-mask-blend-opacity =
    .description = multiplies the mask
    .label = opacity

param-mask-blur-radius =
    .description = standard deviation, in full-resolution pixels
    .label = radius

param-mask-combine-group-invert =
    .description = invert the folded mask
    .label = group_invert

param-mask-combine-group-opacity =
    .description = multiplies the folded mask, after its inversion
    .label = group_opacity

param-mask-combine-invert =
    .description = 1 to invert a member before its opacity, by port
    .label = invert

param-mask-combine-opacity =
    .description = each member's opacity, by port (missing values are 1)
    .label = opacity

param-mask-combine-operator =
    .description = how the members fold together, in port order
    .label = operator

param-mask-combine-operator-choice-difference = difference

param-mask-combine-operator-choice-exclusion = exclusion

param-mask-combine-operator-choice-maximum = maximum

param-mask-combine-operator-choice-minimum = minimum

param-mask-combine-operator-choice-product = product

param-mask-combine-operator-choice-screen = screen

param-mask-combine-operator-choice-sum = sum

param-mask-feather-radius =
    .description = window radius, in full-resolution pixels
    .label = radius

param-mask-from-luma-high =
    .description = luminance where the mask is full
    .label = high

param-mask-from-luma-low =
    .description = luminance where the mask starts
    .label = low

param-mask-levels-brightness =
    .description = shifts the mask up or down
    .label = brightness

param-mask-levels-contrast =
    .description = steepens the mask around its middle
    .label = contrast

param-mask-levels-invert =
    .description = invert the mask, after contrast and brightness
    .label = invert

param-mask-levels-opacity =
    .description = multiplies the mask, last
    .label = opacity

param-mask-parametric-blue =
    .description = blue range: low, high, feather below, feather above
    .label = blue

param-mask-parametric-blue-invert =
    .description = invert this channel's selection
    .label = blue_invert

param-mask-parametric-chroma =
    .description = chroma range (Cz x 100): low, high, feather below, feather above
    .label = chroma

param-mask-parametric-chroma-invert =
    .description = invert this channel's selection
    .label = chroma_invert

param-mask-parametric-green =
    .description = green range: low, high, feather below, feather above
    .label = green

param-mask-parametric-green-invert =
    .description = invert this channel's selection
    .label = green_invert

param-mask-parametric-hue =
    .description = hue range, in turns: low, high, feather below, feather above
    .label = hue

param-mask-parametric-hue-invert =
    .description = invert this channel's selection
    .label = hue_invert

param-mask-parametric-invert =
    .description = invert the combined mask
    .label = invert

param-mask-parametric-lightness =
    .description = lightness range (Jz x 100): low, high, feather below, feather above
    .label = lightness

param-mask-parametric-lightness-invert =
    .description = invert this channel's selection
    .label = lightness_invert

param-mask-parametric-luminance =
    .description = luminance range: low, high, feather below, feather above
    .label = luminance

param-mask-parametric-luminance-invert =
    .description = invert this channel's selection
    .label = luminance_invert

param-mask-parametric-red =
    .description = red range: low, high, feather below, feather above
    .label = red

param-mask-parametric-red-invert =
    .description = invert this channel's selection
    .label = red_invert

param-mask-raster-invert =
    .description = invert the reused mask, before its opacity
    .label = invert

param-mask-raster-opacity =
    .description = multiplies the reused mask, after its inversion
    .label = opacity

param-sigmoid-black-target =
    .description = display black, in %
    .label = black_target

param-sigmoid-contrast =
    .description = contrast around middle grey
    .label = contrast

param-sigmoid-preserve-hue =
    .description = how much of the hue to keep, in %
    .label = preserve_hue

param-sigmoid-skew =
    .description = skews the contrast toward shadows or highlights
    .label = skew

param-sigmoid-white-target =
    .description = display white, in %
    .label = white_target

param-vkdt-exposure-exposure =
    .description = the exposure correction value in stops
    .label = exposure

param-vkdt-usm-amount =
    .description = how much to increase sharpness
    .label = amount

param-vkdt-usm-thrs =
    .description = the threshold to not boost noise
    .label = thrs

param-whitebalance-blue =
    .description = blue coefficient (user mode)
    .label = blue

param-whitebalance-green =
    .description = green coefficient (user mode)
    .label = green

param-whitebalance-mode =
    .description = the camera's coefficients, or the user's
    .label = mode

param-whitebalance-mode-choice-as-shot = as_shot

param-whitebalance-mode-choice-user = user

param-whitebalance-red =
    .description = red coefficient (user mode)
    .label = red
