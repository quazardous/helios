kind: added

A network can start from files: camera raws (Bayer and X-Trans sensors,
through rawler) and JPEG, PNG or TIFF pictures. A raw comes in as its sensor
data, untouched, with its camera's description (maker, model, exposure,
levels, white balance, color matrix) handed to the modules that read it; a
picture comes in converted to the working color space. The preview a raw
carries can be read without decoding the raw, for fast culling.
