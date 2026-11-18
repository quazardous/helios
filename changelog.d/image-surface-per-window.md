kind: added

With Wide-gamut preview on, a detached viewer now gets an image surface of its own on Wayland, drawn for the screen it is on (Rec.2020 where the compositor manages colour), instead of following the main window's. Detached panels' windows are built transparent there for it. On X11, Windows and macOS the viewer stays sRGB and the display report says why.
