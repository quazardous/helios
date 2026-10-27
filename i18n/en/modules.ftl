# Generated from Helios's declarations and code by `cargo xtask i18n extract`:
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

param-mask-brush-invert =
    .description = invert the shape, before its opacity
    .label = invert

param-mask-brush-opacity =
    .description = multiplies the shape, after its inversion
    .label = opacity

param-mask-brush-points =
    .description = up to 256 points, 9 values each: the corner, the handle before it, the handle after it (x, y each, image pixels), the radius (image pixels), the hardness and the density (0 to 1)
    .label = points

param-mask-circle-feather =
    .description = falloff width past the radius, image pixels
    .label = feather

param-mask-circle-invert =
    .description = invert the shape, before its opacity
    .label = invert

param-mask-circle-opacity =
    .description = multiplies the shape, after its inversion
    .label = opacity

param-mask-circle-radius =
    .description = where the falloff starts, image pixels
    .label = radius

param-mask-circle-x =
    .description = centre, image pixels from the left
    .label = x

param-mask-circle-y =
    .description = centre, image pixels from the top
    .label = y

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

param-mask-ellipse-feather =
    .description = falloff width past the edge, image pixels (along the longer axis when proportional)
    .label = feather

param-mask-ellipse-invert =
    .description = invert the shape, before its opacity
    .label = invert

param-mask-ellipse-opacity =
    .description = multiplies the shape, after its inversion
    .label = opacity

param-mask-ellipse-proportional =
    .description = the falloff scales with each half-axis instead of keeping one width
    .label = proportional

param-mask-ellipse-radius-x =
    .description = half-axis along the rotated x axis, image pixels
    .label = radius_x

param-mask-ellipse-radius-y =
    .description = half-axis along the rotated y axis, image pixels
    .label = radius_y

param-mask-ellipse-rotation =
    .description = degrees from the x axis toward the y axis (clockwise on screen)
    .label = rotation

param-mask-ellipse-x =
    .description = centre, image pixels from the left
    .label = x

param-mask-ellipse-y =
    .description = centre, image pixels from the top
    .label = y

param-mask-feather-radius =
    .description = window radius, in full-resolution pixels
    .label = radius

param-mask-from-luma-high =
    .description = luminance where the mask is full
    .label = high

param-mask-from-luma-low =
    .description = luminance where the mask starts
    .label = low

param-mask-gradient-feather =
    .description = width of the transition, image pixels
    .label = feather

param-mask-gradient-invert =
    .description = invert the shape, before its opacity
    .label = invert

param-mask-gradient-opacity =
    .description = multiplies the shape, after its inversion
    .label = opacity

param-mask-gradient-profile =
    .description = the transition: linear, or sigmoid (an error function)
    .label = profile

param-mask-gradient-profile-choice-linear = linear

param-mask-gradient-profile-choice-sigmoid = sigmoid

param-mask-gradient-rotation =
    .description = degrees: 0 is full above the line, 90 full left of it
    .label = rotation

param-mask-gradient-x =
    .description = a point of the line, image pixels from the left
    .label = x

param-mask-gradient-y =
    .description = a point of the line, image pixels from the top
    .label = y

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

param-mask-object-area =
    .description = the region of the image the mask covers: x, y, width, height in image pixels
    .label = area

param-mask-object-confidence =
    .description = how sure the model is, from 0 to 1
    .label = confidence

param-mask-object-kind =
    .description = what to find: the main subject, the sky, a person, or what the prompt names
    .label = kind

param-mask-object-kind-choice-person = person

param-mask-object-kind-choice-prompt = prompt

param-mask-object-kind-choice-sky = sky

param-mask-object-kind-choice-subject = subject

param-mask-object-mask =
    .description = the mask found: an 8-bit grayscale PNG, base64
    .label = mask

param-mask-object-prompt =
    .description = what to find, in words, when the kind is prompt
    .label = prompt

param-mask-object-resolver =
    .description = who finds it: a resolver's name, empty for the default one
    .label = resolver

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

param-mask-path-feather =
    .description = falloff width outside the path, image pixels
    .label = feather

param-mask-path-invert =
    .description = invert the shape, before its opacity
    .label = invert

param-mask-path-opacity =
    .description = multiplies the shape, after its inversion
    .label = opacity

param-mask-path-points =
    .description = up to 64 points, 6 values each: the corner, the handle before it, the handle after it (x, y each, image pixels); fewer than 3 points draw nothing
    .label = points

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

param-vkdt-contrast-detail =
    .description = the amount of detail enhancement
    .label = detail

param-vkdt-contrast-edges =
    .description = the guided filter's edge threshold
    .label = edges

param-vkdt-contrast-radius =
    .description = the guided filter's radius, a fraction of the image's width
    .label = radius

param-vkdt-contrast-thrs =
    .description = the threshold for detail enhancement
    .label = thrs

param-vkdt-exposure-exposure =
    .description = the exposure correction value in stops
    .label = exposure

param-vkdt-grade-gain =
    .description = multiply, applied first: red, green, blue, and all three
    .label = gain

param-vkdt-grade-gamma =
    .description = power for the midtones, above 1 brightens: red, green, blue, and all three
    .label = gamma

param-vkdt-grade-hi-pivot =
    .description = where midtones turn into highlights (zoned log)
    .label = hi_pivot

param-vkdt-grade-lift =
    .description = blend towards a colour in the shadows: red, green, blue, and all three
    .label = lift

param-vkdt-grade-mode =
    .description = apply to all tones alike, or by zone
    .label = mode

param-vkdt-grade-mode-choice-classic = classic

param-vkdt-grade-mode-choice-zoned-log = zoned_log

param-vkdt-grade-offset =
    .description = add, applied last: red, green, blue, and all three
    .label = offset

param-vkdt-grade-sh-pivot =
    .description = where shadows turn into midtones (zoned log)
    .label = sh_pivot

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
