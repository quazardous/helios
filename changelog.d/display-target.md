kind: changed

The viewport's display transform encodes for the target the window draws
for, instead of always for sRGB: on a target wider than sRGB (Rec.2020,
extended sRGB) colours outside the sRGB gamut are no longer clipped.
