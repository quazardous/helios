kind: added

darktable's drawn masks come over with a sidecar: a module blending
through circles, ellipses, gradients, paths, brushes and their groups
(with each member's inversion and opacity, the mask's inversion, blur,
contrast and brightness, and the module's opacity) gets the same mask as
Helios nodes. A mask Helios cannot reproduce exactly (a clone or AI
shape, a curved gradient, a parametric or raster mask, another blend
mode...) is never approximated: the module is kept aside, and the
import's warning says why.
