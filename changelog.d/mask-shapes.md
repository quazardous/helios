kind: added

Masks can be drawn: circles, ellipses, linear gradients and paths (polygons
or Bézier curves), each with a feather, an opacity and an inversion,
combined with the other mask members by every group operator. Shapes are
placed in image pixels, so they stay put at every zoom and crop, their
edges are anti-aliased, and parts of the image away from a shape cost
almost nothing to render.
