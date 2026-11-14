kind: fixed

Library thumbnails of phone photos (a Pixel's Ultra HDR or Motion Photo JPEG, say) show the photo in colour again: the thumbnail read took the grey gain map stored beside the picture, which showed the photo in black and white, or as a blank white cell. Colour streams are now read before grey ones, and the grey thumbnails already cached for such files are read again.
