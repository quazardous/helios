kind: changed

Google Photos: an import and the import window's thumbnails share one request budget per account (120 a minute, two at a time), so running both together never asks more of Google than it allows; downloads go before thumbnails when both wait. A connector declares it in its manifest's `[limits]`, now with `per_second` and `parallel`.
