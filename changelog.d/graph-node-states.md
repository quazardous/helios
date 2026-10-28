kind: fixed

The graph view's node headers follow what the viewport renders: a node
reads done as soon as the tiles on screen are computed instead of idle
until a full-size render, and says cached when it was served from the
cache without computing.
