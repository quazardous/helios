kind: fixed

Rendering a network where several modules read the same inputs, as chained masked modules do, no longer slows down exponentially with the chain's length: each shared step is planned once per render. Six chained masked modules render about three times faster and with a seventh of the memory.
