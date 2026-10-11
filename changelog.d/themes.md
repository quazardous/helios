kind: added

Themes. Helios ships Helios Dark (the default), Helios Light and Helios
High Contrast, chosen from the palette ("Theme: …"). A theme is a Design
Tokens file (`*.tokens.json`) of named colours and sizes; it can build on
another and change only a few of its values, and `theme.export` writes
the theme in use as one file to share. A theme with a mistake still
works: the faulty values fall back to the dark theme's, and Helios says
which.
