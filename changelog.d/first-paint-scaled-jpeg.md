kind: changed

Opening a raw whose embedded preview is large (a Nikon Z6 II's 6048 px) shows it sooner: the preview is decoded at 1/2, 1/4 or 1/8 of its size straight from its JPEG coefficients (DCT scaling, as libjpeg's scaled decode that darktable and Lightroom's quick previews rely on), at the smallest scale that still covers a full-screen viewport, instead of being decoded whole and then shrunk. Its colours are converted from the 8-bit pixels directly, with the same values bit for bit, without a floating-point copy first.
