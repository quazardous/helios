kind: fixed

A render that several parts of the window share (the viewport, the preview) is no longer lost when one of them gives up: it runs until every one of them has, and a render given up is never kept as an answer, so asking again computes it again.
